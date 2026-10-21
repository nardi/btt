//! The price model, which estimates the lowest single fare for one adult in second class from
//! the operator, the kind of train and the distance of a run, and learns from every live price
//! the app sees.
//!
//! The model is a Bayesian linear regression on the log price with one-hot features. The log
//! price is a level per country, an offset per operator and a shape that all countries share.
//! `docs/price-model.md` describes the model and the reasons for its form.

mod config;
mod posterior;

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use btt_model::{EuroCents, Price, PriceKind, PriceObservation, PriceSource};
use chrono::NaiveDate;
use smol_str::SmolStr;

use crate::FareDataError;
use crate::price_book::{RideFacts, TicketPriceSource};
use config::ModelConfig;
pub use config::TrainKind;
use posterior::Posterior;

/// Live prices below this many euro are left out, because their logarithm says nothing about
/// a fare, such as the zero of free travel.
const LOWEST_OBSERVED_EURO: f64 = 0.5;

/// A term of the regression.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Feature {
    /// The global intercept, the log price at the reference distance.
    Intercept,
    /// The global slope on the log distance.
    Slope,
    /// The offset of a country's intercept.
    Country(SmolStr),
    /// The offset of a country's slope.
    CountrySlope(SmolStr),
    /// The offset of an operator.
    Operator(SmolStr),
    /// The offset of an operator's slope, for an operator with a tariff of its own.
    OperatorSlope(SmolStr),
    /// The base fare of an operator's tariff, as a term that fades with the distance.
    OperatorBase(SmolStr),
    /// The bend of an operator's tariff over the distance, such as a taper on long trips.
    OperatorCurve(SmolStr),
    /// The offset of a kind of train, shared by all countries.
    Kind(TrainKind),
    /// The offset of the slope of a kind of train, shared by all countries.
    KindSlope(TrainKind),
    /// The offset of a kind of train in one country.
    CountryKind(SmolStr, TrainKind),
    /// The offset of a category of one operator, with the category in upper case.
    Category(SmolStr, SmolStr),
    /// The surcharge of a run that crosses a border.
    Border,
    /// The base fare that all tariffs share, as a term that fades with the distance.
    Base,
    /// The base fare of a country's tariffs.
    CountryBase(SmolStr),
    /// The bend of a country's distance curve.
    CountryCurve(SmolStr),
    /// The offset of a category in one country, shared by the operators that run it.
    CountryCategory(SmolStr, SmolStr),
    /// The border surcharge as a base fare that fades with the distance.
    BorderBase,
    /// The border surcharge of one operator.
    BorderOperator(SmolStr),
}

/// What the model reads from a run of trains.
struct RunSummary<'a> {
    /// The operator of the longest ride. Empty when not known.
    operator: &'a str,
    /// The category of the longest ride, in upper case.
    category: SmolStr,
    /// The country of a run inside one country. For a run across a border, the home country of
    /// its operator, or else the country its longest ride leaves from.
    country: Option<SmolStr>,
    /// The kind with the highest rank among the rides.
    kind: TrainKind,
    /// The total length of the rides.
    kilometres: f64,
    /// Whether the rides touch more than one country.
    crosses_border: bool,
}

/// A group of runs that shares an estimate of the noise.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum NoiseGroup {
    /// The runs of one kind of train in every country.
    Kind(TrainKind),
    /// The runs of one kind of train in one country, empty when not known.
    CountryKind(SmolStr, TrainKind),
    /// The runs of one operator.
    Operator(SmolStr),
}

/// The running estimate of the noise of one group of runs.
#[derive(Debug, Clone, Copy, Default)]
struct NoiseEstimate {
    /// The sum of the squared residuals that the noise explains.
    squared_residuals: f64,
    /// The number of observations behind the sum.
    weight: f64,
}

impl NoiseEstimate {
    /// Returns the variance of the log price, a mean of a prior variance that counts as
    /// `prior_weight` observations and the residuals.
    fn variance(self, prior_variance: f64, prior_weight: f64) -> f64 {
        (prior_weight * prior_variance + self.squared_residuals) / (prior_weight + self.weight)
    }
}

/// An estimate of the lowest fare of a run, in euro.
#[derive(Debug, Clone, PartialEq)]
pub struct PriceEstimate {
    /// The median of the fare.
    pub median: f64,
    /// The low end of the range that holds the interval share of fares.
    pub low: f64,
    /// The high end of that range.
    pub high: f64,
    /// The kind of train the run counts as.
    pub kind: TrainKind,
    /// The country whose level the run takes, if known.
    pub country: Option<SmolStr>,
    /// The number of live prices the model has learned from for the operator of the run.
    pub operator_observations: u32,
    /// The number of live prices the model has learned from for the country of the run.
    pub country_observations: u32,
    /// The trains of the run that a live price for the same day also covers, such as `ICE 848`.
    /// The estimate leans on that price. Empty when no such price is known.
    pub shared_trains: Vec<SmolStr>,
}

/// One ride of a run that a live price covered, kept to compare with other runs.
#[derive(Debug, Clone)]
struct StoredRide {
    /// The operator as the app names it. Empty when not known.
    operator: SmolStr,
    /// Category of the train.
    category: SmolStr,
    /// Country of the boarding station, if known.
    from_country: Option<SmolStr>,
    /// Country of the alighting station, if known.
    to_country: Option<SmolStr>,
    /// Length of the ride along its line, in kilometres.
    kilometres: f64,
    /// Whether the train has berths.
    offers_berths: bool,
    /// The train as category and number. Empty when not known.
    train: SmolStr,
}

impl StoredRide {
    /// Returns the ride as the facts that the model estimates from.
    fn facts(&self) -> RideFacts<'_> {
        RideFacts {
            from_country: self.from_country.as_deref(),
            to_country: self.to_country.as_deref(),
            operator: &self.operator,
            category: &self.category,
            distance_kilometres: self.kilometres,
            offers_berths: self.offers_berths,
            train: &self.train,
            date: None,
        }
    }
}

/// A run that a live price covered, kept so that runs on the same trains can lean on its price.
#[derive(Debug, Clone)]
struct LiveRun {
    /// The day of the first departure.
    date: NaiveDate,
    /// The rides of the run.
    rides: Vec<StoredRide>,
    /// The log of the price in euro.
    log_price: f64,
}

/// What a live price of another run changes in the estimate of a run that shares trains with it.
struct SharedAdjustment {
    /// The amount to add to the log price.
    shift: f64,
    /// The amount to take off the noise variance, because part of the noise is known.
    noise_reduction: f64,
    /// The trains that both runs ride.
    trains: Vec<SmolStr>,
}

/// An estimate of the log price of a run before the live prices of other runs are taken in.
struct LogEstimate<'a> {
    /// What the model reads from the run.
    summary: RunSummary<'a>,
    /// The mean of the log price.
    mean: f64,
    /// The variance of the mean, from the weights.
    parameter_variance: f64,
    /// The variance of a single fare around the mean.
    noise_variance: f64,
}

/// The price model with its posterior.
#[derive(Debug, Clone)]
pub struct PriceModel {
    /// The priors and tables.
    config: ModelConfig,
    /// The home country of each operator.
    operator_countries: HashMap<SmolStr, SmolStr>,
    /// The index of each term that an observation has involved. Other terms are at their prior.
    feature_indices: HashMap<Feature, usize>,
    /// The posterior of the weights of the terms in `feature_indices`.
    posterior: Posterior,
    /// The number of observations that involved each term, by index.
    observation_counts: Vec<u32>,
    /// The noise of each kind of train in each country. Countries differ a lot here, from the
    /// fixed fares of the Netherlands to the saver and zone fares of Switzerland.
    noise: HashMap<NoiseGroup, NoiseEstimate>,
    /// The day the posterior holds for, which is the day of the latest observation or later.
    current_day: Option<NaiveDate>,
    /// The number of standard deviations from the median to each end of the price range.
    interval_half_width: f64,
    /// The latest runs with named trains that a live price covered, oldest first.
    live_runs: Vec<LiveRun>,
}

impl PriceModel {
    /// Builds the model at its prior from the texts of `price_model.toml` and `operators.toml`.
    pub(crate) fn from_files(
        config_text: &str,
        operators_text: &str,
    ) -> Result<Self, FareDataError> {
        let config = ModelConfig::from_toml(config_text)?;
        let interval_half_width = standard_normal_quantile(0.5 + config.interval_share / 2.0);
        Ok(Self {
            operator_countries: config::operator_countries(operators_text)?,
            config,
            feature_indices: HashMap::new(),
            posterior: Posterior::new(),
            observation_counts: Vec::new(),
            noise: HashMap::new(),
            current_day: None,
            interval_half_width,
            live_runs: Vec::new(),
        })
    }

    /// Builds the model at its prior from the compiled-in files, without the seed observations.
    ///
    /// # Errors
    ///
    /// Returns an error when a file does not parse or holds an impossible value.
    pub fn prior() -> Result<Self, FareDataError> {
        Self::from_files(crate::PRICE_MODEL_TOML, crate::OPERATORS_TOML)
    }

    /// Builds the model at its prior from the text of a `price_model.toml` and the compiled-in
    /// operators, without the seed observations. The examples compare configurations with it.
    ///
    /// # Errors
    ///
    /// Returns an error when the text does not parse or holds an impossible value.
    pub fn with_config(config_text: &str) -> Result<Self, FareDataError> {
        Self::from_files(config_text, crate::OPERATORS_TOML)
    }

    /// Builds the model from the compiled-in files and learns from the seed observations.
    ///
    /// # Errors
    ///
    /// Returns an error when a file does not parse or holds an impossible value.
    pub fn bundled() -> Result<Self, FareDataError> {
        let mut model = Self::from_files(crate::PRICE_MODEL_TOML, crate::OPERATORS_TOML)?;
        for observation in seed_observations(crate::PRICE_SEED_JSONL)? {
            model.observe(&observation);
        }
        Ok(model)
    }

    /// Returns the number of live prices the model has learned from.
    pub fn observation_count(&self) -> u32 {
        self.feature_indices
            .get(&Feature::Intercept)
            .map_or(0, |&index| self.observation_counts[index])
    }

    /// Reads what the model needs from a run of rides, or `None` for an empty run.
    fn run_summary<'a>(&self, rides: &[RideFacts<'a>]) -> Option<RunSummary<'a>> {
        let longest = rides
            .iter()
            .max_by(|a, b| a.distance_kilometres.total_cmp(&b.distance_kilometres))?;
        let kind = rides
            .iter()
            .map(|ride| {
                self.config
                    .kind_of(ride.operator, ride.category, ride.offers_berths)
            })
            .max_by_key(|kind| kind.rank())?;
        let mut countries = rides
            .iter()
            .flat_map(|ride| [ride.from_country, ride.to_country])
            .flatten();
        let first_country = countries.next();
        let crosses_border = countries.any(|country| Some(country) != first_country);
        // A run inside one country costs what that country's trains cost, whoever runs them,
        // such as a German ICE between Basel and Olten. A run across a border takes the level
        // of its operator's home country.
        let country = if crosses_border {
            self.operator_countries
                .get(longest.operator)
                .cloned()
                .or_else(|| longest.from_country.map(SmolStr::from))
        } else {
            first_country.map(SmolStr::from)
        };
        Some(RunSummary {
            operator: longest.operator,
            category: longest.category.to_uppercase().into(),
            country,
            kind,
            kilometres: rides.iter().map(|ride| ride.distance_kilometres).sum(),
            crosses_border,
        })
    }

    /// Returns the prior mean and standard deviation of a term.
    fn prior_of(&self, feature: &Feature) -> (f64, f64) {
        let config = &self.config;
        let spread = &config.spread;
        // The log price at the reference distance, for a price per kilometre and a shift.
        let level = |euro_per_kilometre: f64, shift: f64| {
            (euro_per_kilometre * (config.reference_kilometres + shift)).ln()
        };
        let global_level = level(
            config.global.euro_per_kilometre,
            config.default_shift_kilometres,
        );
        match feature {
            Feature::Intercept => (global_level, config.global.intercept_standard_deviation),
            Feature::Slope => (config.global.slope, config.global.slope_standard_deviation),
            Feature::Country(code) => {
                config
                    .country(code)
                    .map_or((0.0, spread.country), |country| {
                        let shift = config.shift_kilometres(Some(code));
                        (
                            level(country.euro_per_kilometre, shift) - global_level,
                            country.standard_deviation.unwrap_or(spread.country),
                        )
                    })
            }
            Feature::CountrySlope(code) => {
                let listed = config
                    .country(code)
                    .and_then(|country| country.standard_deviation);
                (
                    0.0,
                    listed.map_or(spread.country_slope, |sd| sd.min(spread.country_slope)),
                )
            }
            Feature::Operator(id) => (
                config.operator(id).map_or(0.0, |operator| operator.offset),
                spread.operator,
            ),
            Feature::OperatorSlope(_) => (0.0, spread.operator_slope),
            Feature::OperatorBase(_) => (0.0, spread.operator_base),
            Feature::OperatorCurve(_) => (0.0, spread.operator_curve),
            Feature::Kind(kind) => (config.kind_offsets.of(*kind), spread.kind),
            Feature::KindSlope(_) => (0.0, spread.kind_slope),
            Feature::CountryKind(..) => (0.0, spread.country_kind),
            Feature::Category(..) => (0.0, spread.category),
            Feature::Border => (0.0, spread.border),
            Feature::Base => (0.0, spread.base),
            Feature::CountryBase(_) => (0.0, spread.country_base),
            Feature::CountryCurve(_) => (0.0, spread.country_curve),
            Feature::CountryCategory(..) => (0.0, spread.country_category),
            Feature::BorderBase => (0.0, spread.border_base),
            Feature::BorderOperator(_) => (0.0, spread.border_operator),
        }
    }

    /// Returns the terms of a run with their values.
    fn features_of(&self, summary: &RunSummary<'_>) -> Vec<(Feature, f64)> {
        let shift = self.config.shift_kilometres(summary.country.as_deref());
        let distance = ((summary.kilometres.max(0.0) + shift)
            / (self.config.reference_kilometres + shift))
            .ln();
        // The log price of `base + rate × km` lies above that of `rate × km` by about
        // `base / (rate × km)`, so a term in the inverse of the distance stands in for a base
        // fare. It is 1 at 5 km and fades to 0.1 at 95 km.
        let base = base_term(summary.kilometres);
        let mut features = vec![
            (Feature::Intercept, 1.0),
            (Feature::Slope, distance),
            (Feature::Base, base),
            (Feature::Kind(summary.kind), 1.0),
            (Feature::KindSlope(summary.kind), distance),
        ];
        if let Some(country) = &summary.country {
            features.push((Feature::Country(country.clone()), 1.0));
            features.push((Feature::CountrySlope(country.clone()), distance));
            features.push((Feature::CountryBase(country.clone()), base));
            features.push((Feature::CountryCurve(country.clone()), distance * distance));
            features.push((Feature::CountryKind(country.clone(), summary.kind), 1.0));
            features.push((
                Feature::CountryCategory(country.clone(), summary.category.clone()),
                1.0,
            ));
        }
        if !summary.operator.is_empty() {
            features.push((Feature::Operator(summary.operator.into()), 1.0));
            features.push((Feature::OperatorSlope(summary.operator.into()), distance));
            features.push((Feature::OperatorBase(summary.operator.into()), base));
            features.push((
                Feature::OperatorCurve(summary.operator.into()),
                distance * distance,
            ));
            features.push((
                Feature::Category(summary.operator.into(), summary.category.clone()),
                1.0,
            ));
        }
        if summary.crosses_border {
            features.push((Feature::Border, 1.0));
            features.push((Feature::BorderBase, base));
            if !summary.operator.is_empty() {
                features.push((Feature::BorderOperator(summary.operator.into()), 1.0));
            }
        }
        // A term with a spread of 0 is left out of the model.
        features.retain(|(feature, _)| self.prior_of(feature).1 > 0.0);
        features
    }

    /// Returns the noise variance of a run's log price. The estimate of each group shrinks
    /// towards the one of the wider group above it: the operator towards its kind of train in
    /// its country, that towards the kind of train in every country, and that towards the
    /// prior. A level with a weight of 0 is left out.
    fn noise_variance(&self, summary: &RunSummary<'_>) -> f64 {
        let prior = &self.config.noise;
        let estimate = |group: NoiseGroup| self.noise.get(&group).copied().unwrap_or_default();
        let mut variance = prior.standard_deviation * prior.standard_deviation;
        let mut weight = prior.prior_weight;
        if prior.kind_weight > 0.0 {
            variance = estimate(NoiseGroup::Kind(summary.kind)).variance(variance, weight);
            weight = prior.kind_weight;
        }
        variance = estimate(NoiseGroup::CountryKind(
            summary.country.clone().unwrap_or_default(),
            summary.kind,
        ))
        .variance(variance, weight);
        if prior.operator_weight > 0.0 && !summary.operator.is_empty() {
            variance = estimate(NoiseGroup::Operator(summary.operator.into()))
                .variance(variance, prior.operator_weight);
        }
        variance
    }

    /// Returns the number of observations behind a term.
    fn count_of(&self, feature: &Feature) -> u32 {
        self.feature_indices
            .get(feature)
            .map_or(0, |&index| self.observation_counts[index])
    }

    /// Estimates the log price of a run from the weights alone, without the live prices of other
    /// runs, or returns `None` for an empty run.
    fn log_estimate<'a>(&self, rides: &[RideFacts<'a>]) -> Option<LogEstimate<'a>> {
        let summary = self.run_summary(rides)?;
        let mut known = Vec::new();
        let mut prior_mean = 0.0;
        let mut prior_variance = 0.0;
        for (feature, value) in self.features_of(&summary) {
            if let Some(&index) = self.feature_indices.get(&feature) {
                known.push((index, value));
            } else {
                // No observation has involved the term, so it is at its prior and independent
                // of the others.
                let (mean, standard_deviation) = self.prior_of(&feature);
                prior_mean += value * mean;
                prior_variance += value * value * standard_deviation * standard_deviation;
            }
        }
        let (known_mean, known_variance) = self.posterior.moments(&known);
        let noise_variance = self.noise_variance(&summary);
        Some(LogEstimate {
            mean: known_mean + prior_mean,
            parameter_variance: known_variance + prior_variance,
            noise_variance,
            summary,
        })
    }

    /// Returns what the live price of another run on the same day changes in the estimate of a
    /// run, when the two ride some of the same trains. Of all such runs, the one that shares the
    /// most distance counts.
    ///
    /// Fares of a train and a day move together, because the saver fares of a train sell out
    /// together. The noise of a fare around the model therefore has a part that two runs on the
    /// same trains have in common. The part grows with the share of their distance that rides the
    /// same trains, and the covariance of the two noises is that share of the common part of
    /// their variances. The other run's residual, the log of its live price over what the model
    /// now predicts for it, then tells the mean of this run's noise. It also takes away the part
    /// of the variance that the covariance explains.
    fn shared_adjustment(
        &self,
        rides: &[RideFacts<'_>],
        estimate: &LogEstimate<'_>,
    ) -> Option<SharedAdjustment> {
        let common_share = self.config.shared_trains.common_share;
        let date = rides.first()?.date?;
        let own_kilometres = estimate.summary.kilometres;
        if common_share <= 0.0 || own_kilometres <= 0.0 {
            return None;
        }
        let mut best: Option<(f64, &LiveRun, Vec<SmolStr>)> = None;
        for run in self.live_runs.iter().filter(|run| run.date == date) {
            let run_kilometres: f64 = run.rides.iter().map(|ride| ride.kilometres).sum();
            if run_kilometres <= 0.0 {
                continue;
            }
            let mut overlap = 0.0;
            let mut trains = Vec::new();
            for ride in rides.iter().filter(|ride| !ride.train.is_empty()) {
                if let Some(other) = run.rides.iter().find(|other| other.train == ride.train) {
                    overlap += ride.distance_kilometres * other.kilometres;
                    trains.push(SmolStr::from(ride.train));
                }
            }
            let share = overlap / (run_kilometres * own_kilometres);
            // A later run wins a tie, because the live runs are in the order they were learned.
            if share > 0.0 && best.as_ref().is_none_or(|&(most, ..)| share >= most) {
                best = Some((share, run, trains));
            }
        }
        let (share, run, trains) = best?;
        let other_facts: Vec<RideFacts<'_>> = run.rides.iter().map(StoredRide::facts).collect();
        let other = self.log_estimate(&other_facts)?;
        let residual = run.log_price - other.mean;
        let covariance =
            share.min(1.0) * common_share * (estimate.noise_variance * other.noise_variance).sqrt();
        let gain = covariance / (other.noise_variance + other.parameter_variance);
        Some(SharedAdjustment {
            shift: gain * residual,
            noise_reduction: gain * covariance,
            trains,
        })
    }

    /// Estimates the lowest fare of a run, or returns `None` for an empty run.
    pub fn estimate(&self, rides: &[RideFacts<'_>]) -> Option<PriceEstimate> {
        let log = self.log_estimate(rides)?;
        let adjustment = self.shared_adjustment(rides, &log);
        let (shift, noise_reduction, shared_trains) =
            adjustment.map_or((0.0, 0.0, Vec::new()), |adjustment| {
                (
                    adjustment.shift,
                    adjustment.noise_reduction,
                    adjustment.trains,
                )
            });
        let mean = log.mean + shift;
        let noise_variance = log.noise_variance - noise_reduction;
        let spread = (log.parameter_variance + noise_variance).sqrt() * self.interval_half_width;
        let summary = log.summary;
        Some(PriceEstimate {
            median: mean.exp(),
            low: (mean - spread).exp(),
            high: (mean + spread).exp(),
            kind: summary.kind,
            operator_observations: self.count_of(&Feature::Operator(summary.operator.into())),
            country_observations: summary.country.as_ref().map_or(0, |country| {
                self.count_of(&Feature::Country(country.clone()))
            }),
            country: summary.country,
            shared_trains,
        })
    }

    /// Prices a run as a range, or returns `None` for an empty run.
    pub fn price(&self, rides: &[RideFacts<'_>]) -> Option<Price> {
        let estimate = self.estimate(rides)?;
        let kilometres: f64 = rides.iter().map(|ride| ride.distance_kilometres).sum();
        let operator = rides
            .iter()
            .max_by(|a, b| a.distance_kilometres.total_cmp(&b.distance_kilometres))
            .map(|ride| ride.operator)
            .unwrap_or_default();
        let basis = if estimate.operator_observations > 0 {
            format!(
                "It has learned from {} of {operator}.",
                live_price_count_text(estimate.operator_observations)
            )
        } else if estimate.country_observations > 0 {
            format!(
                "It has no live prices of this operator yet and learned from {} in its country.",
                live_price_count_text(estimate.country_observations)
            )
        } else {
            "It has no live prices for this country yet, so the range is wide.".to_owned()
        };
        let basis = if estimate.shared_trains.is_empty() {
            basis
        } else {
            format!(
                "{basis} It also leans on a live price for {} on the same day.",
                estimate.shared_trains.join(" and ")
            )
        };
        Some(Price {
            low: EuroCents::from_euro(estimate.low),
            high: EuroCents::from_euro(estimate.high),
            kind: PriceKind::Range,
            source: PriceSource {
                name: "Price model".into(),
                note: format!(
                    "An estimate of the lowest second-class fare for {kilometres:.0} km on {} \
                     trains, from a model that learns from the live prices the app sees. {basis} \
                     About {:.0} in 10 fares fall inside the range.",
                    estimate.kind.text(),
                    self.config.interval_share * 10.0,
                )
                .into(),
                as_of: None,
                page: None,
            },
            original: None,
        })
    }

    /// Returns the index of a term, adding it at its prior when no observation has involved it.
    fn index_of(&mut self, feature: Feature) -> usize {
        if let Some(&index) = self.feature_indices.get(&feature) {
            return index;
        }
        let (mean, standard_deviation) = self.prior_of(&feature);
        let index = self.posterior.add_weight(mean, standard_deviation);
        self.observation_counts.push(0);
        self.feature_indices.insert(feature, index);
        index
    }

    /// Forgets old observations up to a day: the posterior moves towards the prior by the share
    /// of the half life that passed since the day it holds for. A day before that changes
    /// nothing.
    pub fn advance_to(&mut self, day: NaiveDate) {
        let Some(current_day) = self.current_day else {
            self.current_day = Some(day);
            return;
        };
        if day <= current_day {
            return;
        }
        // Whole days fit in an `f64` exactly.
        #[expect(clippy::cast_precision_loss)]
        let days = (day - current_day).num_days() as f64;
        let data_weight = 0.5_f64.powf(days / self.config.half_life_days);
        self.posterior.decay(data_weight);
        for noise in self.noise.values_mut() {
            noise.squared_residuals *= data_weight;
            noise.weight *= data_weight;
        }
        self.current_day = Some(day);
        // A run that left before this day can no longer inform the estimate of a run to buy.
        self.live_runs.retain(|run| run.date >= day);
    }

    /// Learns from a live price. Returns whether the model used it. It leaves out prices that
    /// are close to zero and runs without rides.
    pub fn observe(&mut self, observation: &PriceObservation) -> bool {
        let euro = observation.price.euro();
        let rides: Vec<RideFacts<'_>> = observation
            .run
            .rides
            .iter()
            .map(RideFacts::of_description)
            .collect();
        if euro < LOWEST_OBSERVED_EURO {
            return false;
        }
        let Some(summary) = self.run_summary(&rides) else {
            return false;
        };
        let groups = noise_groups(&summary);
        let noise_variance = self.noise_variance(&summary);
        let features = self.features_of(&summary);
        self.advance_to(observation.looked_up_on);
        let sparse: Vec<(usize, f64)> = features
            .into_iter()
            .map(|(feature, value)| (self.index_of(feature), value))
            .collect();
        let log_price = euro.ln();
        let (predicted, parameter_variance) = self.posterior.moments(&sparse);
        let residual = log_price - predicted;
        // The largest squared residual that counts in full.
        let limit = self.config.outlier_limit.powi(2) * (noise_variance + parameter_variance);
        let squared_residual = residual * residual;
        // An outlier counts as if its noise were large enough to put it at the limit.
        let effective_variance = if squared_residual > limit {
            noise_variance * squared_residual / limit
        } else {
            noise_variance
        };
        self.posterior
            .observe(&sparse, log_price, effective_variance);
        // The noise learns from the residual after the update, which no longer carries the
        // error of a prior that the data have since corrected. That residual is smaller than the
        // noise by the share of the observation the weights took up, its leverage, so it is
        // scaled back up.
        let (updated, updated_variance) = self.posterior.moments(&sparse);
        let leverage = (updated_variance / effective_variance).min(0.95);
        let noise_part = (log_price - updated).powi(2) / (1.0 - leverage);
        for group in groups {
            let noise = self.noise.entry(group).or_default();
            noise.squared_residuals += noise_part.min(limit);
            noise.weight += 1.0;
        }
        for &(index, _) in &sparse {
            self.observation_counts[index] += 1;
        }
        self.keep_live_run(observation, log_price);
        true
    }

    /// Keeps the run of a live price to compare with other runs, when the app named its trains.
    /// A price for the same trains on the same day replaces the earlier one. The model keeps
    /// the latest `kept_runs` runs.
    fn keep_live_run(&mut self, observation: &PriceObservation, log_price: f64) {
        let rides: Vec<StoredRide> = observation
            .run
            .rides
            .iter()
            .map(|ride| StoredRide {
                operator: ride.operator.clone(),
                category: ride.category.clone(),
                from_country: ride.from_country.clone(),
                to_country: ride.to_country.clone(),
                kilometres: f64::from(ride.distance_metres) / 1000.0,
                offers_berths: ride.offers_berths,
                train: ride.train.clone(),
            })
            .collect();
        if rides.iter().all(|ride| ride.train.is_empty()) {
            return;
        }
        let date = observation.run.local_departure.date();
        let same_trains = |run: &LiveRun| {
            run.date == date
                && run.rides.len() == rides.len()
                && run
                    .rides
                    .iter()
                    .zip(&rides)
                    .all(|(a, b)| a.train == b.train)
        };
        self.live_runs.retain(|run| !same_trains(run));
        self.live_runs.push(LiveRun {
            date,
            rides,
            log_price,
        });
        let kept = self.config.shared_trains.kept_runs;
        if self.live_runs.len() > kept {
            self.live_runs.drain(..self.live_runs.len() - kept);
        }
    }
}

/// Returns the value of the base fare terms for a distance in kilometres, which is 1 at 5 km
/// and fades to 0.1 at 95 km.
fn base_term(kilometres: f64) -> f64 {
    10.0 / (kilometres.max(0.0) + 5.0)
}

/// Returns the groups whose noise estimates a run feeds.
fn noise_groups(summary: &RunSummary<'_>) -> Vec<NoiseGroup> {
    let mut groups = vec![
        NoiseGroup::Kind(summary.kind),
        NoiseGroup::CountryKind(summary.country.clone().unwrap_or_default(), summary.kind),
    ];
    if !summary.operator.is_empty() {
        groups.push(NoiseGroup::Operator(summary.operator.into()));
    }
    groups
}

/// Returns a number of live prices in words, such as "1 live price" or "12 live prices".
fn live_price_count_text(count: u32) -> String {
    if count == 1 {
        "1 live price".to_owned()
    } else {
        format!("{count} live prices")
    }
}

/// Reads the observations of the seed run from the text of `price_seed.jsonl`, one JSON
/// document per line, ordered by the day of the lookup.
///
/// # Errors
///
/// Returns an error for a line that is not an observation.
pub fn seed_observations(text: &str) -> Result<Vec<PriceObservation>, FareDataError> {
    let mut observations = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<PriceObservation>(line).map_err(|error| {
                FareDataError::InvalidValue {
                    file: "price_seed.jsonl",
                    message: error.to_string(),
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    observations.sort_by_key(|observation| observation.looked_up_on);
    Ok(observations)
}

/// Returns the value below which a standard normal variable falls with probability `p`, by the
/// rational approximation of Peter Acklam, which is accurate to about 1e-9.
fn standard_normal_quantile(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    // Below this probability, and above one minus it, the tail formula holds.
    const TAIL: f64 = 0.024_25;
    let tail = |q: f64| {
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    if p < TAIL {
        tail((-2.0 * p.ln()).sqrt())
    } else if p > 1.0 - TAIL {
        -tail((-2.0 * (1.0 - p).ln()).sqrt())
    } else {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    }
}

/// The price model shared between the price book, which reads it, and the code that feeds it
/// live prices.
#[derive(Debug, Clone)]
pub struct SharedPriceModel(Arc<RwLock<PriceModel>>);

impl SharedPriceModel {
    /// Shares a model.
    pub fn new(model: PriceModel) -> Self {
        Self(Arc::new(RwLock::new(model)))
    }

    /// Locks the model for reading. Every write leaves the model consistent, so a poisoned
    /// lock is used as is.
    pub fn read(&self) -> RwLockReadGuard<'_, PriceModel> {
        self.0.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Locks the model for writing.
    pub fn write(&self) -> RwLockWriteGuard<'_, PriceModel> {
        self.0.write().unwrap_or_else(PoisonError::into_inner)
    }
}

impl TicketPriceSource for SharedPriceModel {
    fn price_run(&self, run: &[RideFacts<'_>]) -> Option<Price> {
        self.read().price(run)
    }
}

impl PriceModel {
    /// Returns the posterior mean weight of a term, at its prior when no observation has
    /// involved it.
    fn mean_of(&self, feature: &Feature) -> f64 {
        self.feature_indices.get(feature).map_or_else(
            || self.prior_of(feature).0,
            |&index| self.posterior.moments(&[(index, 1.0)]).0,
        )
    }

    /// Returns the log price of one class of rides from the posterior mean of the model.
    ///
    /// The constant gathers every offset of the class, the distance gathers every slope, and
    /// the curve and the base hold the terms of an operator's own tariff. The border surcharge
    /// is left out, because the search prices a ride by the station it boards at.
    pub fn route_row(
        &self,
        operator: &str,
        category: &str,
        offers_berths: bool,
        country: Option<&str>,
    ) -> crate::price_table::PriceRow {
        let kind = self.config.kind_of(operator, category, offers_berths);
        let category: SmolStr = category.to_uppercase().into();
        let mut constant = self.mean_of(&Feature::Intercept) + self.mean_of(&Feature::Kind(kind));
        let mut distance = self.mean_of(&Feature::Slope) + self.mean_of(&Feature::KindSlope(kind));
        let mut curve = 0.0;
        let mut base = self.mean_of(&Feature::Base);
        if let Some(code) = country {
            constant += self.mean_of(&Feature::Country(code.into()))
                + self.mean_of(&Feature::CountryKind(code.into(), kind))
                + self.mean_of(&Feature::CountryCategory(code.into(), category.clone()));
            distance += self.mean_of(&Feature::CountrySlope(code.into()));
            curve += self.mean_of(&Feature::CountryCurve(code.into()));
            base += self.mean_of(&Feature::CountryBase(code.into()));
        }
        // The model gives an operator without a name no terms of its own.
        if !operator.is_empty() {
            constant += self.mean_of(&Feature::Operator(operator.into()))
                + self.mean_of(&Feature::Category(operator.into(), category));
            distance += self.mean_of(&Feature::OperatorSlope(operator.into()));
            curve += self.mean_of(&Feature::OperatorCurve(operator.into()));
            base += self.mean_of(&Feature::OperatorBase(operator.into()));
        }
        crate::price_table::PriceRow {
            constant: row_f32(constant),
            distance: row_f32(distance),
            curve: row_f32(curve),
            base: row_f32(base),
            shift: row_f32(self.config.shift_kilometres(country)),
        }
    }

    /// Returns the distance the log price divides by, in kilometres.
    pub fn reference_kilometres(&self) -> f32 {
        row_f32(self.config.reference_kilometres)
    }
}

/// Narrows a posterior weight to the `f32` the price rows hold. The narrowing loses no
/// weight that matters, because the model's prices are estimates with a wide range.
#[expect(clippy::cast_possible_truncation)]
fn row_f32(value: f64) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests;
