//! Assistance, wheelchair spaces and companion discounts per operator, from
//! `data/fares/accessibility_rules.toml`.

use serde::Deserialize;
use smol_str::SmolStr;

use crate::FareDataError;
use crate::reservations::SeatFee;

/// File name used in error messages.
const FILE: &str = "accessibility_rules.toml";

/// Whether a wheelchair user needs to reserve a wheelchair space.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WheelchairSpace {
    /// A space is available without a reservation.
    Free,
    /// A space must be reserved.
    Reservation {
        /// The fee in euro.
        fee: SeatFee,
    },
    /// The trains have no wheelchair space.
    NotAvailable,
}

/// The discount for one companion of a wheelchair user.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CompanionDiscount {
    /// The companion travels free of charge.
    Free {
        /// The card or document that gives the discount.
        entitlement: SmolStr,
    },
    /// The companion gets a share off a single ticket.
    PercentOff {
        /// The share in percent, from 1 to 100.
        percent: u8,
        /// The card or document that gives the discount.
        entitlement: SmolStr,
    },
    /// There is no companion discount.
    None,
}

impl CompanionDiscount {
    /// Returns the share of the fare that the companion pays, from 0 to 1.
    pub fn fare_share(&self) -> f64 {
        match self {
            Self::Free { .. } => 0.0,
            Self::PercentOff { percent, .. } => 1.0 - f64::from(*percent) / 100.0,
            Self::None => 1.0,
        }
    }
}

/// The accessibility services of one operator.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct OperatorAccessibility {
    /// The operator, as an `id` from `data/operators.toml`.
    pub id: SmolStr,
    /// Whether the values are not checked against the page of the operator.
    #[serde(default)]
    pub unverified: bool,
    /// The page where the traveller books assistance.
    pub assistance_url: SmolStr,
    /// The number of hours before departure by which assistance has to be booked.
    pub notice_hours: u16,
    /// Whether a wheelchair space has to be reserved.
    pub wheelchair_space: WheelchairSpace,
    /// The discount for one companion.
    pub companion: CompanionDiscount,
    /// Train categories that the operator lists as not step-free.
    #[serde(default)]
    pub categories_not_accessible: Vec<SmolStr>,
    /// A note for the traveller.
    pub notes: Option<SmolStr>,
}

impl OperatorAccessibility {
    /// Returns whether the operator lists a train category as not step-free, ignoring case.
    pub fn excludes_category(&self, category: &str) -> bool {
        self.wheelchair_space == WheelchairSpace::NotAvailable
            || self
                .categories_not_accessible
                .iter()
                .any(|listed| listed.to_uppercase() == category.to_uppercase())
    }
}

/// The `[[operator]]` list of the accessibility rules file.
#[derive(Deserialize)]
struct AccessibilityRulesFile {
    /// The entries.
    operator: Vec<OperatorAccessibility>,
}

/// The accessibility services of every operator with an entry.
#[derive(Debug, Clone, Default)]
pub struct AccessibilityRules {
    /// The entries, in file order.
    pub operators: Vec<OperatorAccessibility>,
}

impl AccessibilityRules {
    /// Reads the entries from the text of the accessibility rules file.
    pub(crate) fn from_toml(text: &str) -> Result<Self, FareDataError> {
        let file: AccessibilityRulesFile =
            toml::from_str(text).map_err(|source| FareDataError::Invalid { file: FILE, source })?;
        let invalid = file.operator.iter().find(|entry| {
            let fee_inverted = matches!(
                entry.wheelchair_space,
                WheelchairSpace::Reservation { fee } if fee.low > fee.high
            );
            let percent_invalid = matches!(
                entry.companion,
                CompanionDiscount::PercentOff { percent, .. } if percent == 0 || percent > 100
            );
            fee_inverted || percent_invalid
        });
        if let Some(entry) = invalid {
            return Err(FareDataError::InvalidValue {
                file: FILE,
                message: format!(
                    "The entry of {} has a fee or a percentage out of range.",
                    entry.id
                ),
            });
        }
        Ok(Self {
            operators: file.operator,
        })
    }

    /// Returns the entry of an operator, or `None` when the operator has no entry.
    pub fn for_operator(&self, operator: &str) -> Option<&OperatorAccessibility> {
        self.operators.iter().find(|entry| entry.id == operator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FareData;

    #[test]
    fn the_entries_parse_with_their_tagged_values() {
        let rules = FareData::bundled().unwrap().accessibility_rules;
        let sbb = rules.for_operator("sbb").unwrap();
        assert_eq!(sbb.wheelchair_space, WheelchairSpace::Free);
        assert!(sbb.companion.fare_share().abs() < f64::EPSILON);
        let italo = rules.for_operator("italo").unwrap();
        assert!((italo.companion.fare_share() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn an_operator_without_wheelchair_spaces_excludes_every_category() {
        let rules = FareData::bundled().unwrap().accessibility_rules;
        assert!(
            rules
                .for_operator("european-sleeper")
                .unwrap()
                .excludes_category("ES")
        );
        assert!(!rules.for_operator("ns").unwrap().excludes_category("IC"));
    }

    #[test]
    fn a_percentage_above_one_hundred_is_rejected() {
        let text = r#"
            [[operator]]
            id = "broken"
            assistance_url = "https://example.org"
            notice_hours = 1
            wheelchair_space = { kind = "free" }
            companion = { kind = "percent_off", percent = 120, entitlement = "card" }
        "#;
        assert!(AccessibilityRules::from_toml(text).is_err());
    }
}
