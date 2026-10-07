//! Seat reservation rules for passholders.

use btt_model::TravelClass;
use serde::Deserialize;
use smol_str::SmolStr;

use crate::FareDataError;

/// File name used in error messages.
const FILE: &str = "interrail_reservations.toml";

/// What a rule covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleScope {
    /// Trains inside one country.
    Domestic,
    /// Trains between countries.
    International,
    /// Sleeping accommodation on night trains.
    Night,
}

/// Whether a passholder needs the reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReservationMode {
    /// A passholder can't board without one.
    Mandatory,
    /// A reservation is possible and advised.
    Recommended,
    /// A reservation is possible and not needed.
    Optional,
    /// No reservation exists.
    Free,
    /// A fee that is not a reservation, which a passholder pays on top.
    Supplement,
    /// Depends on the train.
    Varies,
}

impl ReservationMode {
    /// Returns whether the optimizer charges the fee, given whether the traveller reserves seats
    /// where a reservation is optional.
    pub fn is_charged(self, reserve_optional_seats: bool) -> bool {
        match self {
            Self::Mandatory | Self::Supplement => true,
            Self::Recommended | Self::Optional | Self::Varies => reserve_optional_seats,
            Self::Free => false,
        }
    }
}

/// The lowest and highest fee in euro.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(from = "[f64; 2]")]
pub struct SeatFee {
    /// The lowest fee seen.
    pub low: f64,
    /// The highest fee seen.
    pub high: f64,
}

impl From<[f64; 2]> for SeatFee {
    fn from([low, high]: [f64; 2]) -> Self {
        Self { low, high }
    }
}

/// The fees of a rule by class.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct ClassFees {
    /// Fee in second class.
    second: SeatFee,
    /// Fee in first class.
    first: SeatFee,
}

/// One reservation rule.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ReservationRule {
    /// Identifies the rule.
    pub id: SmolStr,
    /// Whether the fee is a rough value that has not been checked against its page.
    #[serde(default)]
    pub unverified: bool,
    /// What the rule covers.
    pub scope: RuleScope,
    /// The country of a domestic rule.
    pub country: Option<SmolStr>,
    /// The countries of an international route, in no particular order.
    #[serde(default)]
    pub route: Vec<SmolStr>,
    /// The operator the rule is for, for people to read.
    pub operator: Option<SmolStr>,
    /// The train categories the rule is for. Empty means every category.
    #[serde(default)]
    pub categories: Vec<SmolStr>,
    /// Whether a passholder needs the reservation.
    pub mode: ReservationMode,
    /// The fees by class.
    fee: ClassFees,
    /// A note from the page.
    pub notes: Option<SmolStr>,
}

impl ReservationRule {
    /// Returns the fee for a class.
    pub fn fee(&self, class: TravelClass) -> SeatFee {
        match class {
            TravelClass::First => self.fee.first,
            TravelClass::Second => self.fee.second,
        }
    }

    /// Returns whether the rule lists a category, ignoring case.
    fn lists_category(&self, category: &str) -> bool {
        self.categories
            .iter()
            .any(|listed| listed.to_uppercase() == category.to_uppercase())
    }
}

/// The reservation rules and the categories no pass covers.
#[derive(Debug, Clone)]
pub struct ReservationRules {
    /// Every rule.
    pub rules: Vec<ReservationRule>,
    /// Categories of trains that an Interrail pass does not cover, in upper case.
    pub not_accepted_categories: Vec<SmolStr>,
    /// Operators whose trains an Interrail pass does not cover.
    pub not_accepted_operators: Vec<SmolStr>,
}

/// The `[[rule]]` list of the reservations file.
#[derive(Deserialize)]
struct ReservationsFile {
    /// The rules.
    rule: Vec<ReservationRule>,
}

/// The `[coverage]` table of the rules file.
#[derive(Deserialize)]
struct CoverageFile {
    /// The table.
    coverage: Coverage,
}

/// Categories that the pass does not cover.
#[derive(Deserialize)]
struct Coverage {
    /// The categories in upper case.
    not_accepted_categories: Vec<SmolStr>,
    /// The operators, as `data/operators.toml` names them.
    #[serde(default)]
    not_accepted_operators: Vec<SmolStr>,
}

impl ReservationRules {
    /// Reads the rules from the text of the reservations file and of the rules file.
    pub(crate) fn from_toml(reservations: &str, rules: &str) -> Result<Self, FareDataError> {
        let file: ReservationsFile = toml::from_str(reservations)
            .map_err(|source| FareDataError::Invalid { file: FILE, source })?;
        let coverage: CoverageFile =
            toml::from_str(rules).map_err(|source| FareDataError::Invalid {
                file: "interrail_rules.toml",
                source,
            })?;
        if let Some(rule) = file.rule.iter().find(|rule| {
            rule.fee.second.low > rule.fee.second.high || rule.fee.first.low > rule.fee.first.high
        }) {
            return Err(FareDataError::InvalidValue {
                file: FILE,
                message: format!("rule {} has a lowest fee above its highest", rule.id),
            });
        }
        Ok(Self {
            rules: file.rule,
            not_accepted_categories: coverage.coverage.not_accepted_categories,
            not_accepted_operators: coverage.coverage.not_accepted_operators,
        })
    }

    /// Returns whether a pass covers a train of this category and operator. An empty operator,
    /// as in plans stored without one, is judged by the category alone.
    pub fn pass_accepted(&self, category: &str, operator: &str) -> bool {
        let category = category.to_uppercase();
        !self
            .not_accepted_categories
            .iter()
            .any(|listed| listed.to_uppercase() == category)
            && !self
                .not_accepted_operators
                .iter()
                .any(|listed| listed == operator)
    }

    /// Returns the rule for a ride, or `None` when no rule is known and no fee is charged.
    ///
    /// A ride with berths uses a night rule. A ride between two countries uses an international
    /// rule whose route holds both. A ride inside a country uses a domestic rule of that
    /// country, the one that lists the category before the one for every category.
    pub fn for_ride(
        &self,
        from_country: Option<&str>,
        to_country: Option<&str>,
        category: &str,
        offers_berths: bool,
    ) -> Option<&ReservationRule> {
        if offers_berths {
            return self
                .rules
                .iter()
                .find(|rule| rule.scope == RuleScope::Night && rule.lists_category(category))
                .or_else(|| {
                    self.rules
                        .iter()
                        .find(|rule| rule.scope == RuleScope::Night && rule.lists_category("EN"))
                });
        }
        let (from, to) = (from_country?, to_country?);
        let matching = |rule: &&ReservationRule| {
            if from == to {
                rule.scope == RuleScope::Domestic && rule.country.as_deref() == Some(from)
            } else {
                rule.scope == RuleScope::International
                    && rule.route.iter().any(|country| country == from)
                    && rule.route.iter().any(|country| country == to)
            }
        };
        self.rules
            .iter()
            .filter(matching)
            .find(|rule| rule.lists_category(category))
            .or_else(|| {
                self.rules
                    .iter()
                    .filter(matching)
                    .find(|rule| rule.categories.is_empty())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FareData;

    fn rules() -> ReservationRules {
        FareData::bundled().unwrap().reservations
    }

    #[test]
    fn a_domestic_ride_takes_the_rule_of_its_category() {
        let rules = rules();
        let rule = rules
            .for_ride(Some("IT"), Some("IT"), "FR", false)
            .expect("Frecciarossa has a rule");
        assert_eq!(rule.id, "it-frecce");
        assert_eq!(rule.mode, ReservationMode::Mandatory);
        assert!((rule.fee(TravelClass::Second).low - 13.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_domestic_regional_train_has_no_rule_where_the_rules_list_categories() {
        assert!(
            rules()
                .for_ride(Some("DE"), Some("DE"), "RE", false)
                .is_none()
        );
        assert!(
            rules()
                .for_ride(Some("DE"), Some("DE"), "ICE", false)
                .is_some()
        );
    }

    #[test]
    fn an_international_ride_needs_both_countries_on_the_route() {
        let rules = rules();
        let rule = rules
            .for_ride(Some("FR"), Some("DE"), "TGV INOUI", false)
            .expect("Paris to Germany has a rule");
        assert_eq!(rule.id, "intl-paris-germany");
        assert!(
            rules
                .for_ride(Some("PL"), Some("DE"), "EC", false)
                .is_none()
        );
    }

    #[test]
    fn a_ride_with_berths_takes_the_night_rule() {
        let rules = rules();
        let rule = rules.for_ride(None, None, "NJ", true).unwrap();
        assert_eq!(rule.scope, RuleScope::Night);
        assert_eq!(rule.id, "night-nightjet");
    }

    #[test]
    fn optional_reservations_are_charged_only_when_asked_for() {
        assert!(!ReservationMode::Optional.is_charged(false));
        assert!(ReservationMode::Optional.is_charged(true));
        assert!(ReservationMode::Mandatory.is_charged(false));
        assert!(!ReservationMode::Free.is_charged(true));
    }

    #[test]
    fn some_categories_are_not_covered_by_a_pass() {
        let rules = rules();
        assert!(!rules.pass_accepted("Italo", ""));
        assert!(rules.pass_accepted("ICE", "db-fernverkehr"));
        // A FlixTrain runs as `IC` in the German feed, so only its operator tells it apart.
        assert!(!rules.pass_accepted("IC", "flixtrain"));
    }
}
