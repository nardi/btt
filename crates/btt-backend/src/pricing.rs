//! Prices planned journeys for the planner, with the sources the ticket plan uses.

use std::sync::Arc;

use btt_fares::{FareData, PriceBook};
use btt_model::{EuroRange, Journey, LegPlanStation};
use btt_planner::{
    JourneyPricer, RestrictedMode, RidePassRow, RidePassTable, RidePriceClass, RidePriceRow,
    RidePriceTable, RuleVerdict,
};

/// Prices a journey through the ticket optimizer, so the price the plan shows and the price
/// the Tickets screen shows come from the same sources.
pub struct PriceBookJourneyPricer {
    /// The tariffs, reservation rules and price model.
    data: Arc<FareData>,
    /// The sources that price single tickets.
    book: Arc<PriceBook>,
}

impl PriceBookJourneyPricer {
    /// Shares the fare data and the price book the app plans and prices tickets with.
    pub fn new(data: Arc<FareData>, book: Arc<PriceBook>) -> Self {
        Self { data, book }
    }
}

impl JourneyPricer for PriceBookJourneyPricer {
    fn journey_price(&self, journey: &Journey, stations: &[LegPlanStation]) -> Option<EuroRange> {
        btt_ticketing::journey_price(journey, stations, &self.data, &self.book)
    }

    fn journey_price_with_pass(
        &self,
        journey: &Journey,
        stations: &[LegPlanStation],
    ) -> Option<EuroRange> {
        btt_ticketing::journey_price_on_pass(journey, stations, &self.data, &self.book)
    }

    fn carriage_rule(&self, mode: RestrictedMode, operator: &str, category: &str) -> RuleVerdict {
        match mode {
            RestrictedMode::Bike => self.data.bike_rules.for_train(operator, category).map_or(
                RuleVerdict::NoRule,
                |rule| {
                    if rule.mode.allows_assembled_bike() {
                        RuleVerdict::Allows {
                            verified: !rule.unverified,
                        }
                    } else {
                        RuleVerdict::Forbids
                    }
                },
            ),
            // The file lists the services of an operator and not its step-free trains. So an
            // entry can only forbid a train, through a category that it lists.
            RestrictedMode::Wheelchair => self
                .data
                .accessibility_rules
                .for_operator(operator)
                .map_or(RuleVerdict::NoRule, |entry| {
                    if entry.excludes_category(category) {
                        RuleVerdict::Forbids
                    } else {
                        RuleVerdict::NoRule
                    }
                }),
        }
    }

    fn pass_price_table(&self, classes: &[RidePriceClass]) -> Option<RidePassTable> {
        // Without a pass in the catalogue no leg lies in a pass window, so the planner
        // searches none of them on pass days.
        if self.data.passes.passes.is_empty() {
            return None;
        }
        let rows = classes
            .iter()
            .map(|class| {
                let country = btt_fares::country_of_time_zone(&class.time_zone);
                RidePassRow {
                    takes: self
                        .data
                        .reservations
                        .pass_accepted(&class.category, &class.operator),
                    cents: btt_ticketing::pass_cents_for_class(
                        &class.category,
                        &class.operator,
                        country,
                        class.offers_berths,
                        &self.data,
                    ),
                }
            })
            .collect();
        Some(RidePassTable { rows })
    }

    fn price_table(&self, classes: &[RidePriceClass]) -> Option<RidePriceTable> {
        let model = self.data.price_model.read();
        let classes: Vec<btt_fares::RouteClass> = classes
            .iter()
            .map(|class| btt_fares::RouteClass {
                operator: class.operator.clone(),
                category: class.category.clone(),
                offers_berths: class.offers_berths,
                country: btt_fares::country_of_time_zone(&class.time_zone).map(Into::into),
            })
            .collect();
        let table = btt_fares::PriceTable::from_model(&model, &classes);
        let rows = table
            .rows()
            .iter()
            .map(|row| RidePriceRow {
                constant: row.constant,
                distance: row.distance,
                curve: row.curve,
                base: row.base,
                shift: row.shift,
            })
            .collect();
        Some(RidePriceTable {
            rows,
            reference_kilometres: table.reference_kilometres(),
        })
    }
}
