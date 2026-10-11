//! Prices planned journeys for the planner, with the sources the ticket plan uses.

use std::sync::Arc;

use btt_fares::{FareData, PriceBook};
use btt_model::{EuroRange, Journey, LegPlanStation};
use btt_planner::JourneyPricer;

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
}
