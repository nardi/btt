//! The prices of night trains: the ticket without a pass and the accommodation with and without
//! a pass, fitted to the offers of the ÖBB shop.
//!
//! The shop lists, for one night train, the price of every kind of accommodation. Without a
//! pass, the price holds the ticket and the accommodation together. A pass holder asks for the
//! same train with the Interrail card and pays the accommodation only, with the reservation. The
//! two are not a fixed difference apart, so each is fitted by itself.
//!
//! `docs/night-train-fares.md` describes the data and the fit.

use std::collections::HashMap;

use btt_model::{EuroCents, Price, PriceKind, PriceSource};
use nalgebra::{DMatrix, DVector};
use serde::Deserialize;

use crate::FareDataError;
use crate::price_book::{RideFacts, TicketPriceSource};

/// The factor from the straight line between the end stations of a night train to the length of
/// its line. The planner measures rides along the line, and a night train runs about a third
/// longer than the straight line, such as 733 km for Amsterdam to Basel against 560 km.
const LINE_PER_STRAIGHT: f64 = 1.3;

/// The distance in kilometres at which the fitted level holds.
const REFERENCE_KILOMETRES: f64 = 600.0;

/// The categories of the timetable that stand for the night trains that the shop sells, in
/// lower case.
const NIGHT_CATEGORIES: &[&str] = &["nj", "nightjet", "en", "euronight"];

/// The share of the prices that the range of an estimate holds, as a number of standard
/// deviations of the log price: 1.28 holds 80 percent.
const RANGE_STANDARD_DEVIATIONS: f64 = 1.28;

/// The smallest standard deviation of the log price that a fit reports, so a range never
/// collapses to a point.
const SMALLEST_SPREAD: f64 = 0.1;

/// A kind of accommodation on a night train.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Accommodation {
    /// A seat in a seating car.
    Seat,
    /// A berth in a couchette compartment of four or six.
    Couchette,
    /// A berth in a sleeper compartment of one to three.
    Berth,
}

/// What a price holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PriceBasis {
    /// The lowest price without a pass, which holds the ticket and the accommodation.
    WithoutPass,
    /// The lowest price for a holder of an Interrail pass, which holds the accommodation and the
    /// reservation, and no ticket.
    WithPass,
    /// The lowest price without a pass, less the price of a seat of the same tariff on the same
    /// train.
    /// Added to the price of a ticket that holds a seat, it makes the price of the
    /// accommodation. A seat has none.
    ExtraOverSeat,
}

/// What one compartment of a train costs by tariff, as the shop lists it.
#[derive(Debug, Clone, Deserialize)]
struct Compartment {
    /// The kind of accommodation.
    #[serde(rename = "type")]
    kind: String,
    /// The price of each tariff without a pass, in euro.
    #[serde(default)]
    prices: HashMap<String, f64>,
    /// The price of each tariff for a holder of a pass, in euro.
    #[serde(default)]
    pass: HashMap<String, f64>,
}

/// A station of a night train.
#[derive(Debug, Clone, Deserialize)]
struct TrainStation {
    /// The name of the station.
    name: String,
}

/// One night train with its offers on a day, one line of `night_fares.jsonl`.
#[derive(Debug, Clone, Deserialize)]
struct NightTrainOffers {
    /// The first station.
    from: TrainStation,
    /// The last station.
    to: TrainStation,
    /// The day the offers were collected, as `YYYY-MM-DD`.
    collected_on: String,
    /// The train, such as `NJ 466`.
    train: String,
    /// The distance in a straight line between the end stations, in kilometres.
    straight_kilometres: f64,
    /// The compartments of the train.
    compartments: Vec<Compartment>,
}

impl NightTrainOffers {
    /// Returns the lowest price of an accommodation among the tariffs and the compartments.
    fn lowest(&self, accommodation: Accommodation, with_pass: bool) -> Option<f64> {
        let wanted = match accommodation {
            Accommodation::Seat => "SEAT",
            Accommodation::Couchette => "COUCHETTE",
            Accommodation::Berth => "BERTH",
        };
        self.compartments
            .iter()
            .filter(|compartment| compartment.kind == wanted)
            .flat_map(|compartment| {
                if with_pass {
                    compartment.pass.values()
                } else {
                    compartment.prices.values()
                }
            })
            .copied()
            .reduce(f64::min)
    }

    /// Returns the tariffs that the train sells without a pass.
    fn tariffs(&self) -> Vec<String> {
        let mut tariffs: Vec<String> = self
            .compartments
            .iter()
            .flat_map(|compartment| compartment.prices.keys().cloned())
            .collect();
        tariffs.sort_unstable();
        tariffs.dedup();
        tariffs
    }

    /// Returns the lowest price of an accommodation in a tariff without a pass.
    fn lowest_of_tariff(&self, accommodation: Accommodation, tariff: &str) -> Option<f64> {
        let wanted = match accommodation {
            Accommodation::Seat => "SEAT",
            Accommodation::Couchette => "COUCHETTE",
            Accommodation::Berth => "BERTH",
        };
        self.compartments
            .iter()
            .filter(|compartment| compartment.kind == wanted)
            .filter_map(|compartment| compartment.prices.get(tariff))
            .copied()
            .reduce(f64::min)
    }

    /// Returns the price of the measure of the train, or `None` when the train has none.
    fn price(&self, accommodation: Accommodation, basis: PriceBasis) -> Option<f64> {
        match basis {
            PriceBasis::WithoutPass => self.lowest(accommodation, false),
            PriceBasis::WithPass => self.lowest(accommodation, true),
            PriceBasis::ExtraOverSeat => {
                if accommodation == Accommodation::Seat {
                    return None;
                }
                // The extra is taken within one tariff, because a seat and a bed of different
                // tariffs differ by more than the bed.
                self.tariffs()
                    .into_iter()
                    .filter_map(|tariff| {
                        let bed = self.lowest_of_tariff(accommodation, &tariff)?;
                        let seat = self.lowest_of_tariff(Accommodation::Seat, &tariff)?;
                        (bed > seat).then_some(bed - seat)
                    })
                    .reduce(f64::min)
            }
        }
    }
}

/// A ridge regression of the log price on the log distance and the kind of train.
#[derive(Debug, Clone)]
struct Fit {
    /// The weights: the log price at the reference distance, the slope on the log distance and
    /// the offset of a EuroNight.
    weights: [f64; 3],
    /// The standard deviation of the log price around the fit.
    spread: f64,
    /// The number of trains the fit uses.
    count: usize,
}

/// The ridge penalty on the slope and the offset of a EuroNight.
const RIDGE: f64 = 1.0;

impl Fit {
    /// Returns the features of a train.
    fn features(kilometres: f64, euronight: bool) -> [f64; 3] {
        [
            1.0,
            (kilometres / REFERENCE_KILOMETRES).ln(),
            f64::from(u8::from(euronight)),
        ]
    }

    /// Fits observations of a log price, or returns `None` with fewer than five.
    fn new(observations: &[([f64; 3], f64)]) -> Option<Self> {
        if observations.len() < 5 {
            return None;
        }
        let mut gram = DMatrix::<f64>::zeros(3, 3);
        let mut moment = DVector::<f64>::zeros(3);
        for (features, log_price) in observations {
            let x = DVector::from_row_slice(features);
            gram += &x * x.transpose();
            moment += &x * *log_price;
        }
        // The level is not penalized, the slope and the offset are.
        gram[(1, 1)] += RIDGE;
        gram[(2, 2)] += RIDGE;
        let solved = gram.lu().solve(&moment)?;
        let weights = [solved[0], solved[1], solved[2]];
        let squares: f64 = observations
            .iter()
            .map(|(features, log_price)| {
                let predicted: f64 = features.iter().zip(&weights).map(|(x, w)| x * w).sum();
                (log_price - predicted).powi(2)
            })
            .sum();
        // Counts are small enough for `f64`.
        #[expect(clippy::cast_precision_loss)]
        let degrees = (observations.len() as f64 - 3.0).max(1.0);
        Some(Self {
            weights,
            spread: (squares / degrees).sqrt().max(SMALLEST_SPREAD),
            count: observations.len(),
        })
    }

    /// Returns the median, the low and the high end of a range, in euro.
    fn range(&self, kilometres: f64, euronight: bool) -> (f64, f64, f64) {
        let features = Self::features(kilometres, euronight);
        let log_price: f64 = features.iter().zip(&self.weights).map(|(x, w)| x * w).sum();
        let half_width = RANGE_STANDARD_DEVIATIONS * self.spread;
        (
            log_price.exp(),
            (log_price - half_width).exp(),
            (log_price + half_width).exp(),
        )
    }
}

/// An estimate of a price, in euro.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NightEstimate {
    /// The median of the price.
    pub median: f64,
    /// The low end of the range that holds four of five prices.
    pub low: f64,
    /// The high end of that range.
    pub high: f64,
    /// The number of trains the estimate is fitted to.
    pub trains: usize,
}

/// How well a measure is predicted on trains the fit has not seen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeasureScore {
    /// The accommodation.
    pub accommodation: Accommodation,
    /// What the price holds.
    pub basis: PriceBasis,
    /// The number of trains scored.
    pub count: usize,
    /// The median error as a factor, such as 1.2 for 20 percent off.
    pub median_error: f64,
    /// The median error of a price that ignores the distance and the kind of train, which is the
    /// median of the other trains.
    pub baseline_median_error: f64,
    /// The share of the prices inside the range of their estimate.
    pub inside_share: f64,
}

/// The measures that the fit covers.
const MEASURES: [(Accommodation, PriceBasis); 9] = [
    (Accommodation::Seat, PriceBasis::WithoutPass),
    (Accommodation::Seat, PriceBasis::WithPass),
    (Accommodation::Seat, PriceBasis::ExtraOverSeat),
    (Accommodation::Couchette, PriceBasis::WithoutPass),
    (Accommodation::Couchette, PriceBasis::WithPass),
    (Accommodation::Couchette, PriceBasis::ExtraOverSeat),
    (Accommodation::Berth, PriceBasis::WithoutPass),
    (Accommodation::Berth, PriceBasis::WithPass),
    (Accommodation::Berth, PriceBasis::ExtraOverSeat),
];

/// Reads the trains from the text of `night_fares.jsonl`.
fn parse_trains(text: &str) -> Result<Vec<NightTrainOffers>, FareDataError> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|error| FareDataError::InvalidValue {
                file: "night_fares.jsonl",
                message: error.to_string(),
            })
        })
        .collect()
}

/// Returns the features and the log price of a measure for every train that has one.
fn observations(
    trains: &[NightTrainOffers],
    accommodation: Accommodation,
    basis: PriceBasis,
) -> Vec<([f64; 3], f64)> {
    trains
        .iter()
        .filter_map(|train| {
            let price = train.price(accommodation, basis)?;
            let features = Fit::features(
                train.straight_kilometres * LINE_PER_STRAIGHT,
                train.train.starts_with("EN"),
            );
            (price > 0.0).then(|| (features, price.ln()))
        })
        .collect()
}

/// Returns the fold of a train, by its two stations without regard to their order.
fn fold_of(train: &NightTrainOffers, folds: usize) -> usize {
    let mut stations = [train.from.name.as_str(), train.to.name.as_str()];
    stations.sort_unstable();
    let hash = stations
        .iter()
        .flat_map(|name| name.bytes())
        .fold(0_u64, |hash, byte| {
            hash.wrapping_mul(31).wrapping_add(u64::from(byte))
        });
    // The remainder is below `folds`, so it fits in `usize`.
    usize::try_from(hash % folds as u64).unwrap_or(0)
}

/// Returns the median of some numbers, which it sorts.
fn median_of(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// The prices of night trains, fitted to the offers of the shop.
#[derive(Debug, Clone)]
pub struct NightFares {
    /// The fit of each measure.
    fits: HashMap<(Accommodation, PriceBasis), Fit>,
    /// The day of the first and the last offers, for the note of a price.
    observed: String,
}

impl NightFares {
    /// Reads the offers from the text of `night_fares.jsonl` and fits every measure.
    ///
    /// # Errors
    ///
    /// Returns an error for a line that is not a night train.
    pub fn from_jsonl(text: &str) -> Result<Self, FareDataError> {
        let trains = parse_trains(text)?;
        let mut days: Vec<&str> = trains
            .iter()
            .map(|train| train.collected_on.as_str())
            .collect();
        days.sort_unstable();
        let observed = match (days.first(), days.last()) {
            (Some(first), Some(last)) if first != last => format!("{first} to {last}"),
            (Some(day), _) => (*day).to_owned(),
            _ => String::new(),
        };
        let mut fits = HashMap::new();
        for (accommodation, basis) in MEASURES {
            if let Some(fit) = Fit::new(&observations(&trains, accommodation, basis)) {
                fits.insert((accommodation, basis), fit);
            }
        }
        Ok(Self { fits, observed })
    }

    /// Scores the fit of every measure on trains it was not fitted to. The trains are split into
    /// folds by their stations, so the two directions of a route and its other days are in the
    /// same fold, and each fold is predicted from the others.
    ///
    /// # Errors
    ///
    /// Returns an error for a line that is not a night train.
    pub fn cross_validate(text: &str, folds: usize) -> Result<Vec<MeasureScore>, FareDataError> {
        let trains = parse_trains(text)?;
        let mut scores = Vec::new();
        for (accommodation, basis) in MEASURES {
            let mut errors: Vec<f64> = Vec::new();
            let mut baseline_errors: Vec<f64> = Vec::new();
            let mut inside = 0_usize;
            for fold in 0..folds {
                let (testing, training): (Vec<&NightTrainOffers>, Vec<&NightTrainOffers>) = trains
                    .iter()
                    .partition(|train| fold_of(train, folds) == fold);
                let training: Vec<NightTrainOffers> = training.into_iter().cloned().collect();
                let observations = observations(&training, accommodation, basis);
                let Some(fit) = Fit::new(&observations) else {
                    continue;
                };
                let mut logs: Vec<f64> = observations.iter().map(|(_, log)| *log).collect();
                let baseline = median_of(&mut logs);
                for train in testing {
                    let Some(price) = train.price(accommodation, basis) else {
                        continue;
                    };
                    let (median, low, high) = fit.range(
                        train.straight_kilometres * LINE_PER_STRAIGHT,
                        train.train.starts_with("EN"),
                    );
                    errors.push((price.ln() - median.ln()).abs());
                    baseline_errors.push((price.ln() - baseline).abs());
                    inside += usize::from(price >= low && price <= high);
                }
            }
            if errors.is_empty() {
                continue;
            }
            // Counts are small enough for `f64`.
            #[expect(clippy::cast_precision_loss)]
            let inside_share = inside as f64 / errors.len() as f64;
            scores.push(MeasureScore {
                accommodation,
                basis,
                count: errors.len(),
                median_error: median_of(&mut errors).exp(),
                baseline_median_error: median_of(&mut baseline_errors).exp(),
                inside_share,
            });
        }
        Ok(scores)
    }

    /// Returns the estimate of a measure for a night train of a length in kilometres along its
    /// line, or `None` when the offers hold none.
    pub fn estimate(
        &self,
        accommodation: Accommodation,
        basis: PriceBasis,
        kilometres: f64,
        euronight: bool,
    ) -> Option<NightEstimate> {
        let fit = self.fits.get(&(accommodation, basis))?;
        let (median, low, high) = fit.range(kilometres.max(1.0), euronight);
        Some(NightEstimate {
            median,
            low,
            high,
            trains: fit.count,
        })
    }

    /// Returns whether a category of the timetable is a night train that the shop sells.
    pub fn sells(category: &str) -> bool {
        NIGHT_CATEGORIES.contains(&category.to_lowercase().as_str())
    }

    /// Returns the estimated price in euro of the cheapest seat without a pass on a night train of
    /// a category over a distance, or `None` when the offers hold too few trains for the fit.
    pub fn seat_price(&self, category: &str, kilometres: f64) -> Option<f64> {
        self.estimate(
            Accommodation::Seat,
            PriceBasis::WithoutPass,
            kilometres,
            Self::is_euronight(category),
        )
        .map(|estimate| estimate.median)
    }

    /// Returns whether a category is a EuroNight.
    pub fn is_euronight(category: &str) -> bool {
        matches!(category.to_lowercase().as_str(), "en" | "euronight")
    }

    /// Returns the price of the accommodation of a night ride as a range from the lowest couchette
    /// to the lowest berth, or `None` when the offers hold no such price.
    ///
    /// A pass holder pays the accommodation with the reservation. Without a pass, the ticket
    /// holds a seat, so the accommodation is the extra over the seat.
    pub fn accommodation_price(
        &self,
        category: &str,
        kilometres: f64,
        with_pass: bool,
    ) -> Option<Price> {
        let basis = if with_pass {
            PriceBasis::WithPass
        } else {
            PriceBasis::ExtraOverSeat
        };
        let euronight = Self::is_euronight(category);
        let couchette = self.estimate(Accommodation::Couchette, basis, kilometres, euronight)?;
        let berth = self.estimate(Accommodation::Berth, basis, kilometres, euronight)?;
        let (low, high) = (
            couchette.median.min(berth.median),
            berth.median.max(couchette.median),
        );
        let holds = if with_pass {
            "the accommodation with its reservation, which a pass holder pays instead of a ticket"
        } else {
            "the accommodation on top of the ticket, which holds a seat"
        };
        Some(Price {
            low: EuroCents::from_euro(low),
            high: EuroCents::from_euro(high),
            kind: PriceKind::Range,
            source: PriceSource {
                name: "ÖBB night train offers".into(),
                note: format!(
                    "The price of {holds}, from the lowest couchette to the lowest berth, as \
                     fitted to {} offers of the ÖBB shop for night trains of the days {}. Night \
                     train prices follow demand, so a price can lie well outside the range.",
                    couchette.trains.min(berth.trains),
                    self.observed,
                )
                .into(),
                as_of: None,
                page: None,
            },
            original: None,
        })
    }
}

/// Prices the runs of night trains from the offers of the shop.
#[derive(Debug, Clone)]
pub struct NightTrainTickets(pub NightFares);

impl TicketPriceSource for NightTrainTickets {
    fn price_run(&self, run: &[RideFacts<'_>]) -> Option<Price> {
        let night = run
            .iter()
            .filter(|ride| ride.offers_berths && NightFares::sells(ride.category))
            .max_by(|a, b| a.distance_kilometres.total_cmp(&b.distance_kilometres))?;
        // A run that holds a night train and a short feeder, such as a train to the station, is
        // priced as the night train, which sets the price. A run with a long other train is not.
        let longest = run
            .iter()
            .map(|ride| ride.distance_kilometres)
            .fold(0.0, f64::max);
        if night.distance_kilometres < longest {
            return None;
        }
        let kilometres: f64 = run.iter().map(|ride| ride.distance_kilometres).sum();
        let estimate = self.0.estimate(
            Accommodation::Seat,
            PriceBasis::WithoutPass,
            kilometres,
            NightFares::is_euronight(night.category),
        )?;
        Some(Price {
            low: EuroCents::from_euro(estimate.low),
            high: EuroCents::from_euro(estimate.high),
            kind: PriceKind::Range,
            source: PriceSource {
                name: "ÖBB night train offers".into(),
                note: format!(
                    "The lowest ticket for a night train, which holds a seat, as fitted to {} \
                     offers of the ÖBB shop for night trains of the days {}. The price hardly \
                     depends on the distance and follows demand.",
                    estimate.trains, self.0.observed,
                )
                .into(),
                as_of: None,
                page: None,
            },
            original: None,
        })
    }
}

#[cfg(test)]
mod tests;
