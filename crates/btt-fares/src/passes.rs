//! The Interrail pass catalogue.

use std::collections::HashMap;

use btt_model::{AgeGroup, EuroCents, TravelClass};
use chrono::{Days, Months, NaiveDate};
use serde::Deserialize;
use smol_str::SmolStr;

use crate::FareDataError;

/// File name used in error messages.
const FILE: &str = "interrail_passes.toml";

/// The age groups that have their own price. Children and infants have no price of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PriceAgeGroup {
    /// 12 to 27.
    Youth,
    /// 28 to 59.
    Adult,
    /// 60 and older.
    Senior,
}

impl PriceAgeGroup {
    /// Returns the group that prices an age group, or `None` for children and infants.
    pub fn of(age_group: AgeGroup) -> Option<Self> {
        match age_group {
            AgeGroup::Infant | AgeGroup::Child => None,
            AgeGroup::Youth => Some(Self::Youth),
            AgeGroup::Adult => Some(Self::Adult),
            AgeGroup::Senior => Some(Self::Senior),
        }
    }

    /// Index into the price table of a pass.
    fn index(self) -> usize {
        match self {
            Self::Youth => 0,
            Self::Adult => 1,
            Self::Senior => 2,
        }
    }
}

/// How long a pass is valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validity {
    /// A number of days, counting the first.
    Days(u16),
    /// A number of calendar months from the first day.
    Months(u8),
}

/// Whether a pass has travel days to pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassKind {
    /// A number of travel days to use within the validity.
    Flexi {
        /// How many days can be travel days.
        travel_days: u8,
    },
    /// Every day of the validity is a travel day.
    Continuous,
}

/// The prices of a pass in euro for each class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassPrices {
    /// Second class.
    pub second: EuroCents,
    /// First class.
    pub first: EuroCents,
}

/// One kind of Interrail pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassType {
    /// The `id` in the price list.
    pub id: SmolStr,
    /// Whether the pass has travel days to pick.
    pub kind: PassKind,
    /// How long the pass is valid.
    pub validity: Validity,
    /// Prices by age group, in the order of `PriceAgeGroup::index`.
    prices: [Option<PassPrices>; 3],
}

impl PassType {
    /// Returns the price for an age group and class, or `None` when the list has none.
    pub fn price(&self, age_group: PriceAgeGroup, class: TravelClass) -> Option<EuroCents> {
        self.prices[age_group.index()].map(|prices| match class {
            TravelClass::First => prices.first,
            TravelClass::Second => prices.second,
        })
    }

    /// Returns the last day of validity of a pass whose first day is `first_valid_date`.
    pub fn last_valid_date(&self, first_valid_date: NaiveDate) -> NaiveDate {
        let day_after_end = match self.validity {
            Validity::Days(days) => first_valid_date.checked_add_days(Days::new(u64::from(days))),
            Validity::Months(months) => {
                first_valid_date.checked_add_months(Months::new(u32::from(months)))
            }
        };
        day_after_end
            .and_then(|day| day.pred_opt())
            .unwrap_or(first_valid_date)
    }

    /// Returns a name for the pass, such as "7 travel days in 1 month".
    pub fn title(&self) -> String {
        let months = |count: u8| {
            if count == 1 {
                "1 month".to_owned()
            } else {
                format!("{count} months")
            }
        };
        match (self.kind, self.validity) {
            (PassKind::Flexi { travel_days }, Validity::Months(count)) => {
                format!("{travel_days} travel days in {}", months(count))
            }
            (PassKind::Flexi { travel_days }, Validity::Days(count)) => {
                format!("{travel_days} travel days in {count} days")
            }
            (PassKind::Continuous, Validity::Days(count)) => format!("{count} days continuous"),
            (PassKind::Continuous, Validity::Months(count)) => {
                format!("{} continuous", months(count))
            }
        }
    }
}

/// The pass list.
#[derive(Debug, Clone)]
pub struct PassCatalogue {
    /// The day the prices were valid.
    pub as_of: NaiveDate,
    /// Where the prices come from.
    pub source: SmolStr,
    /// Every pass.
    pub passes: Vec<PassType>,
}

/// The parts of `interrail_passes.toml` that the catalogue reads.
#[derive(Deserialize)]
struct PassesFile {
    /// The day the prices were valid.
    as_of: NaiveDate,
    /// Where the prices come from.
    source: String,
    /// The passes.
    pass: Vec<RawPass>,
}

/// One `[[pass]]` table.
#[derive(Deserialize)]
struct RawPass {
    /// The id of the pass.
    id: String,
    /// `flexi` or `continuous`.
    kind: String,
    /// Travel days of a flexi pass.
    travel_days: Option<u8>,
    /// Validity in months.
    validity_months: Option<u8>,
    /// Validity in days.
    validity_days: Option<u16>,
    /// Prices by age group name.
    prices: HashMap<String, RawClassPrices>,
}

/// The prices of one age group.
#[derive(Deserialize)]
struct RawClassPrices {
    /// Second class price in euro.
    second: f64,
    /// First class price in euro.
    first: f64,
}

impl PassCatalogue {
    /// Reads the catalogue from the text of `interrail_passes.toml`.
    pub(crate) fn from_toml(text: &str) -> Result<Self, FareDataError> {
        let file: PassesFile =
            toml::from_str(text).map_err(|source| FareDataError::Invalid { file: FILE, source })?;
        let invalid = |message: String| FareDataError::InvalidValue {
            file: FILE,
            message,
        };
        let mut passes = Vec::new();
        for raw in file.pass {
            let validity = match (raw.validity_months, raw.validity_days) {
                (Some(months), None) => Validity::Months(months),
                (None, Some(days)) => Validity::Days(days),
                _ => {
                    return Err(invalid(format!(
                        "pass {} needs one of validity_months and validity_days",
                        raw.id
                    )));
                }
            };
            let kind = match (raw.kind.as_str(), raw.travel_days) {
                ("flexi", Some(travel_days)) => PassKind::Flexi { travel_days },
                ("continuous", None) => PassKind::Continuous,
                _ => return Err(invalid(format!("pass {} has an unknown kind", raw.id))),
            };
            let mut prices = [None; 3];
            for (group, group_prices) in raw.prices {
                let group = match group.as_str() {
                    "youth" => PriceAgeGroup::Youth,
                    "adult" => PriceAgeGroup::Adult,
                    "senior" => PriceAgeGroup::Senior,
                    other => {
                        return Err(invalid(format!("pass {} has an age group {other}", raw.id)));
                    }
                };
                prices[group.index()] = Some(PassPrices {
                    second: EuroCents::from_euro(group_prices.second),
                    first: EuroCents::from_euro(group_prices.first),
                });
            }
            passes.push(PassType {
                id: raw.id.into(),
                kind,
                validity,
                prices,
            });
        }
        Ok(Self {
            as_of: file.as_of,
            source: file.source.into(),
            passes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FareData, PASSES_TOML};

    fn catalogue() -> PassCatalogue {
        PassCatalogue::from_toml(PASSES_TOML).unwrap()
    }

    #[test]
    fn prices_follow_the_price_list() {
        let catalogue = catalogue();
        let flexi_seven = catalogue
            .passes
            .iter()
            .find(|pass| pass.id == "global-flexi-7-in-1-month")
            .unwrap();
        assert_eq!(
            flexi_seven.price(PriceAgeGroup::Adult, TravelClass::Second),
            Some(EuroCents(38100))
        );
        assert_eq!(
            flexi_seven.price(PriceAgeGroup::Youth, TravelClass::First),
            Some(EuroCents(37100))
        );
        assert_eq!(flexi_seven.title(), "7 travel days in 1 month");
    }

    #[test]
    fn a_pass_ends_on_the_day_before_its_period_repeats() {
        let catalogue = catalogue();
        let date = |year, month, day| NaiveDate::from_ymd_opt(year, month, day).unwrap();
        let find = |id: &str| catalogue.passes.iter().find(|pass| pass.id == id).unwrap();
        assert_eq!(
            find("global-flexi-7-in-1-month").last_valid_date(date(2026, 1, 1)),
            date(2026, 1, 31)
        );
        assert_eq!(
            find("global-flexi-7-in-1-month").last_valid_date(date(2026, 1, 31)),
            date(2026, 2, 27)
        );
        assert_eq!(
            find("global-continuous-15-days").last_valid_date(date(2026, 3, 1)),
            date(2026, 3, 15)
        );
    }

    #[test]
    fn every_pass_has_a_price_for_every_group() {
        let data = FareData::bundled().unwrap();
        for pass in &data.passes.passes {
            for group in [
                PriceAgeGroup::Youth,
                PriceAgeGroup::Adult,
                PriceAgeGroup::Senior,
            ] {
                assert!(
                    pass.price(group, TravelClass::Second).is_some(),
                    "{}",
                    pass.id
                );
            }
        }
    }
}
