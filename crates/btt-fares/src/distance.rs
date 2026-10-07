//! The rough price per kilometre, for every ride that no other source prices.

use btt_model::{EuroCents, Price, PriceKind, PriceSource};
use serde::Deserialize;

use crate::FareDataError;
use crate::price_book::{RideFacts, TicketPriceSource};

/// A price per kilometre with a lowest and a highest value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DistanceModel {
    /// Euro per kilometre of a cheap fare.
    pub low_per_kilometre: f64,
    /// Euro per kilometre of a flexible fare.
    pub high_per_kilometre: f64,
    /// The least a ticket costs, in euro.
    pub minimum_fare: f64,
}

/// The parts of `tariffs.toml` that the model reads.
#[derive(Deserialize)]
struct TariffsFile {
    /// The table of the distance model.
    distance_model: DistanceModelSection,
}

/// The `[distance_model]` table.
#[derive(Deserialize)]
struct DistanceModelSection {
    /// Euro per kilometre of a cheap fare.
    low_per_kilometre: f64,
    /// Euro per kilometre of a flexible fare.
    high_per_kilometre: f64,
    /// The least a ticket costs, in euro.
    minimum_fare: f64,
}

impl DistanceModel {
    /// Reads the model from the text of `tariffs.toml`.
    pub(crate) fn from_toml(text: &str) -> Result<Self, FareDataError> {
        let file: TariffsFile = toml::from_str(text).map_err(|source| FareDataError::Invalid {
            file: "tariffs.toml",
            source,
        })?;
        let section = file.distance_model;
        if section.low_per_kilometre > section.high_per_kilometre || section.minimum_fare < 0.0 {
            return Err(FareDataError::InvalidValue {
                file: "tariffs.toml",
                message: "the distance model needs low <= high and a minimum fare of zero or more"
                    .to_owned(),
            });
        }
        Ok(Self {
            low_per_kilometre: section.low_per_kilometre,
            high_per_kilometre: section.high_per_kilometre,
            minimum_fare: section.minimum_fare,
        })
    }

    /// Prices a distance in kilometres. Both ends of the range are at least the minimum fare.
    pub fn price(&self, kilometres: f64) -> Price {
        let low = (kilometres * self.low_per_kilometre).max(self.minimum_fare);
        let high = (kilometres * self.high_per_kilometre).max(self.minimum_fare);
        Price {
            low: EuroCents::from_euro(low),
            high: EuroCents::from_euro(high),
            kind: PriceKind::Range,
            source: PriceSource {
                name: "Distance estimate".into(),
                note: format!(
                    "No price source covers this ride, so the price is {:.0} to {:.0} cents per \
                     kilometre over {kilometres:.0} km.",
                    self.low_per_kilometre * 100.0,
                    self.high_per_kilometre * 100.0
                )
                .into(),
                as_of: None,
                page: None,
            },
            original: None,
        }
    }
}

impl TicketPriceSource for DistanceModel {
    fn price_run(&self, run: &[RideFacts<'_>]) -> Option<Price> {
        Some(self.price(run.iter().map(|ride| ride.distance_kilometres).sum()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TARIFFS_TOML;

    fn model() -> DistanceModel {
        DistanceModel::from_toml(TARIFFS_TOML).unwrap()
    }

    #[test]
    fn a_long_ride_costs_by_distance() {
        let price = model().price(500.0);
        assert_eq!(price.low, EuroCents(3500));
        assert_eq!(price.high, EuroCents(10000));
        assert_eq!(price.kind, PriceKind::Range);
    }

    #[test]
    fn a_short_ride_costs_the_minimum_fare() {
        let price = model().price(5.0);
        assert_eq!(price.low, EuroCents(250));
        assert_eq!(price.high, EuroCents(250));
    }
}
