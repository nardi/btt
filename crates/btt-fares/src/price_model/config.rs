//! The priors and tables of the price model, read from `price_model.toml` and
//! `operators.toml`.

use std::collections::HashMap;

use serde::Deserialize;
use smol_str::SmolStr;

use crate::FareDataError;

/// A coarse class of train that operators share, which the model pools across countries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainKind {
    /// Trains on high-speed lines, such as ICE, TGV and Frecciarossa.
    HighSpeed,
    /// Other long-distance trains, such as IC, EC and Railjet.
    LongDistance,
    /// Regional and suburban trains.
    Regional,
    /// Trains with couchettes or sleepers.
    Night,
    /// Buses, such as rail replacement buses.
    Bus,
}

impl TrainKind {
    /// Returns how strongly the kind sets the price of a run that mixes kinds. A run takes the
    /// kind with the highest rank among its trains.
    pub(crate) fn rank(self) -> u8 {
        match self {
            Self::Bus => 0,
            Self::Regional => 1,
            Self::LongDistance => 2,
            Self::HighSpeed => 3,
            Self::Night => 4,
        }
    }

    /// Returns the name of the kind for the note of a price.
    pub(crate) fn text(self) -> &'static str {
        match self {
            Self::HighSpeed => "high-speed",
            Self::LongDistance => "long-distance",
            Self::Regional => "regional",
            Self::Night => "night",
            Self::Bus => "bus",
        }
    }
}

/// The level that every country starts from.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GlobalLevel {
    /// Price per kilometre at a slope of 1, in euro.
    pub euro_per_kilometre: f64,
    /// Prior mean of the slope on the log distance.
    pub slope: f64,
    /// Prior standard deviation of the global intercept.
    pub intercept_standard_deviation: f64,
    /// Prior standard deviation of the global slope.
    pub slope_standard_deviation: f64,
}

/// Prior standard deviations of the offsets on the log price.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Spread {
    /// Of a country's intercept.
    pub country: f64,
    /// Of a country's slope.
    pub country_slope: f64,
    /// Of an operator's offset.
    pub operator: f64,
    /// Of an operator's slope.
    pub operator_slope: f64,
    /// Of the base fare term of an operator.
    pub operator_base: f64,
    /// Of the bend of an operator's tariff.
    pub operator_curve: f64,
    /// Of a kind's intercept.
    pub kind: f64,
    /// Of a kind's slope.
    pub kind_slope: f64,
    /// Of the offset of a kind in one country.
    pub country_kind: f64,
    /// Of the offset of a category of one operator.
    pub category: f64,
    /// Of the surcharge of a run that crosses a border.
    pub border: f64,
}

/// The prior of the noise of a single fare around the model.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct NoisePrior {
    /// Standard deviation of the log price.
    pub standard_deviation: f64,
    /// Number of observations the prior counts as.
    pub prior_weight: f64,
}

/// Prior means of the kind offsets.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct KindOffsets {
    /// Of high-speed trains.
    pub high_speed: f64,
    /// Of long-distance trains.
    pub long_distance: f64,
    /// Of regional trains.
    pub regional: f64,
    /// Of night trains.
    pub night: f64,
    /// Of buses.
    pub bus: f64,
}

impl KindOffsets {
    /// Returns the prior mean of the offset of a kind.
    pub(crate) fn of(&self, kind: TrainKind) -> f64 {
        match kind {
            TrainKind::HighSpeed => self.high_speed,
            TrainKind::LongDistance => self.long_distance,
            TrainKind::Regional => self.regional,
            TrainKind::Night => self.night,
            TrainKind::Bus => self.bus,
        }
    }
}

/// The price level of one country.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CountryLevel {
    /// Two-letter code of the country.
    pub code: SmolStr,
    /// Price per kilometre at a slope of 1, in euro.
    pub euro_per_kilometre: f64,
    /// The shift of the distance, which stands in for a base fee.
    pub shift_kilometres: Option<f64>,
    /// Prior standard deviation of the country's terms, when it is surer than the default.
    pub standard_deviation: Option<f64>,
}

/// An operator that prices differently from the rest of its country.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OperatorPrior {
    /// The id of the operator in `operators.toml`.
    pub id: SmolStr,
    /// Prior mean of its offset on the log price.
    #[serde(default)]
    pub offset: f64,
    /// The kind of its trains when no category rule matches.
    pub kind: Option<TrainKind>,
}

/// A rule that gives the kind of trains by their category.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct KindRule {
    /// The kind of the trains the rule matches.
    pub kind: TrainKind,
    /// The categories the rule matches, compared without regard to case.
    pub categories: Vec<SmolStr>,
    /// The operators the rule is for. Empty means every operator.
    #[serde(default)]
    pub operators: Vec<SmolStr>,
}

/// The contents of `price_model.toml`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ModelConfig {
    /// Days after which an observation counts half.
    pub half_life_days: f64,
    /// Share of fares that the price range should hold.
    pub interval_share: f64,
    /// Residuals beyond this many standard deviations count less.
    pub outlier_limit: f64,
    /// The shift for a country without one of its own.
    pub default_shift_kilometres: f64,
    /// The distance at which the intercepts hold.
    pub reference_kilometres: f64,
    /// The prior of the noise.
    pub noise: NoisePrior,
    /// The level that every country starts from.
    pub global: GlobalLevel,
    /// Prior standard deviations of the offsets.
    pub spread: Spread,
    /// Prior means of the kind offsets.
    pub kind_offsets: KindOffsets,
    /// Price levels of countries.
    #[serde(rename = "country")]
    pub countries: Vec<CountryLevel>,
    /// Operators with a prior of their own.
    #[serde(rename = "operator")]
    pub operators: Vec<OperatorPrior>,
    /// Rules for the kind of a train.
    #[serde(rename = "kind_rule")]
    pub kind_rules: Vec<KindRule>,
}

impl ModelConfig {
    /// Reads the configuration from the text of `price_model.toml`.
    pub(crate) fn from_toml(text: &str) -> Result<Self, FareDataError> {
        let mut config: Self = toml::from_str(text).map_err(|source| FareDataError::Invalid {
            file: "price_model.toml",
            source,
        })?;
        // Categories are compared in lower case, so the rules keep them that way.
        for rule in &mut config.kind_rules {
            for category in &mut rule.categories {
                *category = category.to_lowercase().into();
            }
        }
        let shares_valid = config.interval_share > 0.0 && config.interval_share < 1.0;
        let positive = [
            config.half_life_days,
            config.outlier_limit,
            config.reference_kilometres,
            config.noise.standard_deviation,
            config.global.euro_per_kilometre,
            config.global.intercept_standard_deviation,
            config.global.slope_standard_deviation,
        ]
        .into_iter()
        .chain(
            config
                .countries
                .iter()
                .map(|country| country.euro_per_kilometre),
        )
        .all(|value| value > 0.0);
        if !shares_valid || !positive {
            return Err(FareDataError::InvalidValue {
                file: "price_model.toml",
                message: "the interval share must lie between 0 and 1, and half lives, prices, \
                          distances and standard deviations must be positive"
                    .to_owned(),
            });
        }
        Ok(config)
    }

    /// Returns the level of a country, if the file lists it.
    pub(crate) fn country(&self, code: &str) -> Option<&CountryLevel> {
        self.countries.iter().find(|country| country.code == code)
    }

    /// Returns the prior of an operator, if the file lists it.
    pub(crate) fn operator(&self, id: &str) -> Option<&OperatorPrior> {
        self.operators.iter().find(|operator| operator.id == id)
    }

    /// Returns the shift of the distance in a country.
    pub(crate) fn shift_kilometres(&self, country: Option<&str>) -> f64 {
        country
            .and_then(|code| self.country(code))
            .and_then(|country| country.shift_kilometres)
            .unwrap_or(self.default_shift_kilometres)
    }

    /// Returns the kind of a train from its operator, its category and whether it has berths.
    pub(crate) fn kind_of(&self, operator: &str, category: &str, offers_berths: bool) -> TrainKind {
        if offers_berths {
            return TrainKind::Night;
        }
        let category = category.to_lowercase();
        self.kind_rules
            .iter()
            .find(|rule| {
                (rule.operators.is_empty() || rule.operators.iter().any(|id| id == operator))
                    && rule.categories.iter().any(|listed| *listed == category)
            })
            .map(|rule| rule.kind)
            .or_else(|| self.operator(operator).and_then(|prior| prior.kind))
            .unwrap_or(TrainKind::Regional)
    }
}

/// One operator of `operators.toml`, with only the fields the model reads.
#[derive(Deserialize)]
struct OperatorEntry {
    /// The name the app uses for the operator.
    id: SmolStr,
    /// The country of the operator's home network.
    country: SmolStr,
}

/// The operators of `operators.toml`.
#[derive(Deserialize)]
struct OperatorsFile {
    /// The rules in order. One operator can have several.
    operator: Vec<OperatorEntry>,
}

/// Reads the home country of each operator from the text of `operators.toml`.
pub(crate) fn operator_countries(text: &str) -> Result<HashMap<SmolStr, SmolStr>, FareDataError> {
    let file: OperatorsFile = toml::from_str(text).map_err(|source| FareDataError::Invalid {
        file: "operators.toml",
        source,
    })?;
    Ok(file
        .operator
        .into_iter()
        .map(|entry| (entry.id, entry.country))
        .collect())
}
