//! Rules for an assembled bike on a train, from `data/fares/bike_rules.toml`.

use serde::Deserialize;
use smol_str::SmolStr;

use crate::FareDataError;
use crate::reservations::SeatFee;

/// File name used in error messages.
const FILE: &str = "bike_rules.toml";

/// What a traveller needs to take an assembled bike on a train.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BikeCarriageMode {
    /// The bike travels without a ticket.
    Free,
    /// The bike needs a ticket, and any train with bike spaces takes it.
    Ticket,
    /// The bike needs a reserved bike space, and the fee covers the ticket as well.
    Reservation,
    /// Only a bike that is taken apart and packed in a bag can travel.
    BaggedOnly,
    /// No bike can travel.
    NotAllowed,
}

impl BikeCarriageMode {
    /// Returns whether an assembled bike can travel on the train.
    pub const fn allows_assembled_bike(self) -> bool {
        matches!(self, Self::Free | Self::Ticket | Self::Reservation)
    }
}

/// The rides that one bike fee covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BikeFeePeriod {
    /// The fee covers one ride.
    Ride,
    /// The fee covers all rides of the operator on one day.
    Day,
}

/// One bike rule.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct BikeRule {
    /// Identifies the rule.
    pub id: SmolStr,
    /// Whether the values are not checked against the page of the operator.
    #[serde(default)]
    pub unverified: bool,
    /// The operator, as an `id` from `data/operators.toml`.
    pub operator: SmolStr,
    /// The train categories the rule is for. Empty means every category.
    #[serde(default)]
    pub categories: Vec<SmolStr>,
    /// What the traveller needs.
    pub mode: BikeCarriageMode,
    /// The fee in euro for one bike.
    pub fee: SeatFee,
    /// The rides that one fee covers.
    pub per: BikeFeePeriod,
    /// The page where the bike ticket is sold.
    pub booking: SmolStr,
    /// A note for the traveller.
    pub notes: Option<SmolStr>,
}

impl BikeRule {
    /// Returns whether the rule lists a category, ignoring case.
    fn lists_category(&self, category: &str) -> bool {
        self.categories
            .iter()
            .any(|listed| listed.to_uppercase() == category.to_uppercase())
    }
}

/// The `[[rule]]` list of the bike rules file.
#[derive(Deserialize)]
struct BikeRulesFile {
    /// The rules.
    rule: Vec<BikeRule>,
}

/// Every bike rule.
#[derive(Debug, Clone, Default)]
pub struct BikeRules {
    /// The rules, in file order.
    pub rules: Vec<BikeRule>,
}

impl BikeRules {
    /// Reads the rules from the text of the bike rules file.
    pub(crate) fn from_toml(text: &str) -> Result<Self, FareDataError> {
        let file: BikeRulesFile =
            toml::from_str(text).map_err(|source| FareDataError::Invalid { file: FILE, source })?;
        if let Some(rule) = file.rule.iter().find(|rule| rule.fee.low > rule.fee.high) {
            return Err(FareDataError::InvalidValue {
                file: FILE,
                message: format!("Rule {} has a lowest fee above its highest.", rule.id),
            });
        }
        Ok(Self { rules: file.rule })
    }

    /// Returns the rule for a train of an operator and a category.
    ///
    /// A rule that lists the category comes before a rule of the same operator without
    /// categories. Returns `None` when the operator has no rule.
    pub fn for_train(&self, operator: &str, category: &str) -> Option<&BikeRule> {
        let of_operator = || {
            self.rules
                .iter()
                .filter(move |rule| rule.operator == operator)
        };
        of_operator()
            .find(|rule| rule.lists_category(category))
            .or_else(|| of_operator().find(|rule| rule.categories.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FareData;

    #[test]
    fn a_listed_category_comes_before_the_operator_rule() {
        let rules = FareData::bundled().unwrap().bike_rules;
        assert_eq!(
            rules.for_train("oebb", "RJX").unwrap().id,
            "at-long-distance"
        );
        assert_eq!(rules.for_train("oebb", "REX").unwrap().id, "at-regional");
        assert!(rules.for_train("unknown-operator", "IC").is_none());
    }

    #[test]
    fn a_bagged_only_rule_forbids_an_assembled_bike() {
        let rules = FareData::bundled().unwrap().bike_rules;
        let rule = rules.for_train("ouigo", "OUIGO").unwrap();
        assert_eq!(rule.mode, BikeCarriageMode::BaggedOnly);
        assert!(!rule.mode.allows_assembled_bike());
    }

    #[test]
    fn a_rule_with_inverted_fees_is_rejected() {
        let text = r#"
            [[rule]]
            id = "broken"
            operator = "ns"
            mode = "ticket"
            fee = [9.0, 1.0]
            per = "day"
            booking = "https://example.org"
        "#;
        assert!(BikeRules::from_toml(text).is_err());
    }
}
