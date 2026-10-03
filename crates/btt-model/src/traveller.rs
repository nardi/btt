//! Traveller profiles, which live outside any single trip.

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use specta::Type;
use uuid::Uuid;

use crate::trip::{TravelClass, TravellerId};

impl TravellerId {
    /// Creates a new time-ordered traveller ID.
    pub fn new_v7() -> Self {
        Self(Uuid::now_v7())
    }
}

/// A discount card that lowers the price of single tickets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum DiscountCard {
    /// Deutsche Bahn `BahnCard` 25.
    BahnCard25,
    /// Deutsche Bahn `BahnCard` 50.
    BahnCard50,
    /// Deutsche Bahn `BahnCard` 100, which covers all trains of Deutsche Bahn.
    BahnCard100,
    /// Swiss Half Fare Card.
    SwissHalfFare,
    /// ÖBB Vorteilscard.
    OebbVorteilscard,
    /// SNCF Carte Avantage.
    SncfAvantage,
}

/// An Interrail pass the traveller already owns.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct OwnedInterrailPass {
    /// The `id` of the pass in `data/fares/interrail_passes.toml`.
    pub pass_id: SmolStr,
    /// First day of validity of the pass.
    pub first_valid_date: NaiveDate,
}

/// Age groups of the Interrail pass, by the age on the first day of travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum AgeGroup {
    /// Younger than 4, who travel free.
    Infant,
    /// 4 to 11.
    Child,
    /// 12 to 27.
    Youth,
    /// 28 to 59.
    Adult,
    /// 60 and older.
    Senior,
}

/// A person who travels on a trip.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct Traveller {
    /// Identifies the traveller across trips.
    pub id: TravellerId,
    /// Name shown in lists.
    pub name: SmolStr,
    /// Date of birth, from which the age group on a day follows.
    pub birth_date: NaiveDate,
    /// Country of residence as a two-letter code, which decides the Interrail residence rule.
    pub residence_country: SmolStr,
    /// Class the traveller books.
    pub travel_class: TravelClass,
    /// Discount cards the traveller owns.
    pub discount_cards: Vec<DiscountCard>,
    /// An Interrail pass the traveller already owns, if any.
    pub owned_pass: Option<OwnedInterrailPass>,
    /// The traveller whose pass a child travels with, if any.
    pub accompanying_traveller: Option<TravellerId>,
}

impl Traveller {
    /// Creates a second class traveller with no cards or pass.
    pub fn new(name: SmolStr, birth_date: NaiveDate, residence_country: SmolStr) -> Self {
        Self {
            id: TravellerId::new_v7(),
            name,
            birth_date,
            residence_country,
            travel_class: TravelClass::Second,
            discount_cards: Vec::new(),
            owned_pass: None,
            accompanying_traveller: None,
        }
    }

    /// Returns the age in whole years on a day. Before the birth date this is 0.
    pub fn age_on(&self, day: NaiveDate) -> u32 {
        let mut years = day.year() - self.birth_date.year();
        if (day.month(), day.day()) < (self.birth_date.month(), self.birth_date.day()) {
            years -= 1;
        }
        u32::try_from(years).unwrap_or(0)
    }

    /// Returns the Interrail age group on a day.
    pub fn age_group_on(&self, day: NaiveDate) -> AgeGroup {
        match self.age_on(day) {
            0..=3 => AgeGroup::Infant,
            4..=11 => AgeGroup::Child,
            12..=27 => AgeGroup::Youth,
            28..=59 => AgeGroup::Adult,
            _ => AgeGroup::Senior,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn age_groups_change_on_the_birthday() {
        let traveller = Traveller::new("Sam".into(), date(1998, 6, 15), "NL".into());
        assert_eq!(traveller.age_group_on(date(2026, 6, 14)), AgeGroup::Youth);
        assert_eq!(traveller.age_on(date(2026, 6, 14)), 27);
        assert_eq!(traveller.age_group_on(date(2026, 6, 15)), AgeGroup::Adult);
    }

    #[test]
    fn a_date_before_birth_gives_an_infant() {
        let traveller = Traveller::new("Sam".into(), date(2020, 1, 1), "NL".into());
        assert_eq!(traveller.age_group_on(date(2019, 1, 1)), AgeGroup::Infant);
        assert_eq!(traveller.age_group_on(date(2024, 1, 1)), AgeGroup::Child);
        assert_eq!(traveller.age_group_on(date(2080, 1, 1)), AgeGroup::Senior);
    }
}
