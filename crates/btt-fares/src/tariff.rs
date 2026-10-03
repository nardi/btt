//! Operator tariffs that are a function of the distance, and booking windows.

use btt_model::{EuroCents, Price, PriceKind, PriceSource};
use chrono::NaiveDate;
use serde::Deserialize;
use smol_str::SmolStr;

use crate::FareDataError;
use crate::price_book::{RideFacts, TicketPriceSource};

/// A tariff of the form `base + per_kilometre * distance` in the operator's currency.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DistanceTariff {
    /// Identifies the tariff.
    pub id: SmolStr,
    /// Name of the operator.
    pub operator: SmolStr,
    /// The country whose domestic rides the tariff prices.
    pub country: SmolStr,
    /// Where the numbers come from.
    pub source: SmolStr,
    /// The day the tariff was valid.
    pub as_of: NaiveDate,
    /// What the tariff is, and how well the formula fits it.
    pub notes: SmolStr,
    /// Fixed part of the price in the operator's currency.
    base: f64,
    /// Price per kilometre in the operator's currency.
    per_kilometre: f64,
    /// Euro for one unit of the operator's currency.
    euro_per_crown: f64,
}

impl DistanceTariff {
    /// Returns the price in euro of a ticket for a distance in kilometres.
    pub fn price_euro(&self, kilometres: f64) -> f64 {
        (self.base + self.per_kilometre * kilometres) * self.euro_per_crown
    }
}

impl TicketPriceSource for DistanceTariff {
    fn price_run(&self, run: &[RideFacts<'_>]) -> Option<Price> {
        let domestic = run.iter().all(|ride| {
            ride.from_country == Some(self.country.as_str())
                && ride.to_country == Some(self.country.as_str())
        });
        if !domestic || run.is_empty() {
            return None;
        }
        let kilometres: f64 = run.iter().map(|ride| ride.distance_kilometres).sum();
        let cents = EuroCents::from_euro(self.price_euro(kilometres));
        Some(Price {
            low: cents,
            high: cents,
            kind: PriceKind::Formula,
            source: PriceSource {
                name: format!("{} basic fare", self.operator).into(),
                note: format!(
                    "{} The distance is the length of the line, and the tariff distance can be a \
                     little longer. Cheaper offers may exist.",
                    self.notes
                )
                .into(),
                as_of: Some(self.as_of),
                page: Some(self.source.clone()),
            },
        })
    }
}

/// How far ahead tickets of a country can be bought.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct BookingWindow {
    /// The country of the first train.
    pub country: SmolStr,
    /// Days before the departure that sales open.
    pub days: u32,
    /// Name of the operator the window is for.
    pub operator: SmolStr,
    /// Where the number comes from.
    pub source: SmolStr,
    /// What to know about the window.
    pub notes: SmolStr,
}

/// The `[[tariff]]` list of `tariffs.toml`.
#[derive(Deserialize)]
struct TariffsFile {
    /// The tariffs.
    tariff: Vec<DistanceTariff>,
}

/// The `[[window]]` list of `booking_windows.toml`.
#[derive(Deserialize)]
struct WindowsFile {
    /// The windows.
    window: Vec<BookingWindow>,
}

/// Reads the tariffs from the text of `tariffs.toml`.
pub(crate) fn tariffs_from_toml(text: &str) -> Result<Vec<DistanceTariff>, FareDataError> {
    let file: TariffsFile = toml::from_str(text).map_err(|source| FareDataError::Invalid {
        file: "tariffs.toml",
        source,
    })?;
    Ok(file.tariff)
}

/// Reads the booking windows from the text of `booking_windows.toml`.
pub(crate) fn windows_from_toml(text: &str) -> Result<Vec<BookingWindow>, FareDataError> {
    let file: WindowsFile = toml::from_str(text).map_err(|source| FareDataError::Invalid {
        file: "booking_windows.toml",
        source,
    })?;
    Ok(file.window)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FareData;

    #[test]
    fn the_czech_formula_matches_the_published_prices() {
        let data = FareData::bundled().unwrap();
        let tariff = &data.tariffs[0];
        // Published prices in crowns for 25, 100 and 250 km, converted at the file's rate.
        for (kilometres, crowns) in [(25.0, 68.0), (100.0, 221.0), (250.0, 527.0)] {
            let euro = tariff.price_euro(kilometres);
            assert!(
                (euro - crowns * 0.041).abs() < 0.05,
                "{kilometres} km gives {euro}"
            );
        }
    }

    #[test]
    fn the_tariff_prices_only_domestic_runs() {
        let data = FareData::bundled().unwrap();
        let tariff = &data.tariffs[0];
        let ride = |from, to| RideFacts {
            from_country: Some(from),
            to_country: Some(to),
            category: "EC",
            distance_kilometres: 100.0,
        };
        assert!(tariff.price_run(&[ride("CZ", "CZ")]).is_some());
        assert!(tariff.price_run(&[ride("CZ", "AT")]).is_none());
        assert!(
            tariff
                .price_run(&[ride("CZ", "CZ"), ride("CZ", "SK")])
                .is_none()
        );
    }

    #[test]
    fn booking_windows_load() {
        let data = FareData::bundled().unwrap();
        assert!(
            data.booking_windows
                .iter()
                .any(|window| window.country == "DE")
        );
    }
}
