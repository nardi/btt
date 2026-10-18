//! The operators of trains, recognised from the agency names of the timetable feeds.

use chrono::NaiveDate;
use regex::Regex;
use serde::Deserialize;
use smol_str::SmolStr;

/// The operator rules, as they are compiled in.
const OPERATORS_TOML: &str = include_str!("../../../data/operators.toml");

/// One rule of `data/operators.toml`.
#[derive(Deserialize)]
struct RawRule {
    /// The name the app uses for the operator.
    id: String,
    /// The name the app shows to the traveller.
    name: String,
    /// The country of the operator's home network, as an ISO 3166 code.
    country: String,
    /// The pattern of the agency names of its trains.
    agency: String,
    /// The pattern of the categories of its trains, when the rule is for some of them only.
    category: Option<String>,
}

/// The rules of the file.
#[derive(Deserialize)]
struct RawRules {
    /// The rules in order.
    operator: Vec<RawRule>,
}

/// A rule that gives the operator of a train.
struct OperatorRule {
    /// The name the app uses for the operator.
    id: SmolStr,
    /// The name the app shows to the traveller.
    name: SmolStr,
    /// The country of the operator's home network.
    country: SmolStr,
    /// Matches the agency names of its trains.
    agency: Regex,
    /// Matches the categories of its trains, when the rule is for some of them only.
    category: Option<Regex>,
}

/// The last day on which a listed operator has trains in a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorEnd {
    /// The name the app uses for the operator.
    pub id: SmolStr,
    /// The name the app shows to the traveller.
    pub name: SmolStr,
    /// The country of the operator's home network.
    pub country: SmolStr,
    /// The last day on which one of the operator's trains departs from its first stop.
    pub last_departure_date: NaiveDate,
}

/// Tells the operator of a train from its agency and category.
pub struct OperatorTable {
    /// The rules in order. The first that matches wins.
    rules: Vec<OperatorRule>,
}

impl OperatorTable {
    /// Reads the rules that are compiled in.
    ///
    /// # Panics
    ///
    /// Panics when the file does not parse or holds a bad pattern, which a test of this module
    /// rules out.
    pub fn bundled() -> Self {
        let raw: RawRules = toml::from_str(OPERATORS_TOML).expect("data/operators.toml parses");
        let rules = raw
            .operator
            .into_iter()
            .map(|rule| OperatorRule {
                id: rule.id.into(),
                name: rule.name.into(),
                country: rule.country.into(),
                agency: Regex::new(&rule.agency).expect("agency patterns are valid"),
                category: rule
                    .category
                    .map(|pattern| Regex::new(&pattern).expect("category patterns are valid")),
            })
            .collect();
        Self { rules }
    }

    /// Returns the name and country of a listed operator, or `None` for an operator that only
    /// has its agency name.
    pub fn details_of(&self, id: &str) -> Option<(&str, &str)> {
        self.rules
            .iter()
            .find(|rule| rule.id == id)
            .map(|rule| (rule.name.as_str(), rule.country.as_str()))
    }

    /// Returns the operator of a train. A train that no rule matches gets its agency name,
    /// written in lower case with dashes, such as `southern` or `arriva-rp`.
    pub fn operator_of(&self, agency: &str, category: &str) -> SmolStr {
        self.rules
            .iter()
            .find(|rule| {
                rule.agency.is_match(agency)
                    && rule
                        .category
                        .as_ref()
                        .is_none_or(|pattern| pattern.is_match(category))
            })
            .map_or_else(|| slug(agency), |rule| rule.id.clone())
    }
}

/// Writes a name in lower case, with a dash for every run of other characters than letters and
/// digits.
fn slug(name: &str) -> SmolStr {
    let mut out = String::with_capacity(name.len());
    for character in name.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            out.push(character);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agencies_with_many_names_get_one_operator() {
        let table = OperatorTable::bundled();
        assert_eq!(table.operator_of("SNCF Voyageurs LO", "TGV INOUI"), "sncf");
        assert_eq!(table.operator_of("TMR SA", "TER"), "sncf");
        assert_eq!(table.operator_of("SNCF VOYAGEURS", "OUIGO"), "ouigo");
        assert_eq!(table.operator_of("RegioJet,a.s.", "RJ"), "regiojet");
        assert_eq!(table.operator_of("RegioJet ÚK a.s.", "Os"), "regiojet");
        assert_eq!(table.operator_of("DB Regio AG NRW", "RE"), "db-regio");
        assert_eq!(
            table.operator_of("OEBB Personenverkehr AG Kundenservice", "NJ"),
            "oebb"
        );
        assert_eq!(
            table.operator_of("Italo - Nuovo Trasporto Viaggiatori", "Italo"),
            "italo"
        );
        assert_eq!(table.operator_of("TRENITALIA", "FR"), "trenitalia");
    }

    #[test]
    fn every_feed_name_of_european_sleeper_gets_its_operator() {
        // European Sleeper sells its tickets alone, so a train that misses the operator would
        // share a ticket with the trains before it.
        let table = OperatorTable::bundled();
        assert_eq!(
            table.operator_of("European Sleeper", "ES"),
            "european-sleeper"
        );
        assert_eq!(
            table.operator_of("Train Charter Services B.V.", "ES"),
            "european-sleeper"
        );
        assert_eq!(
            table.operator_of("CityRail, a.s.", "ES"),
            "european-sleeper"
        );
        assert_ne!(
            table.operator_of("CityRail, a.s.", "Os"),
            "european-sleeper"
        );
    }

    #[test]
    fn an_unknown_agency_keeps_its_name() {
        let table = OperatorTable::bundled();
        assert_eq!(
            table.operator_of("West Midlands Trains", "X"),
            "west-midlands-trains"
        );
        assert_eq!(slug("Arriva RP"), "arriva-rp");
        assert_eq!(slug(""), "");
    }
}
