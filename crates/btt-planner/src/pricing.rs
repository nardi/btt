//! Prices a planned journey, so the planner can show what a journey costs next to its time.

use btt_model::{EuroRange, Journey, LegPlanStation};

/// Prices the rides of a planned journey, with the same sources the ticket plan uses.
///
/// The planner calls this as a generic parameter, so every call is a static dispatch. The
/// trait lives in the planner and speaks the model's types only, so implementing it needs no
/// fare crate and the planner stays free of `btt-fares`.
pub trait JourneyPricer: Send + Sync + 'static {
    /// Returns the estimated price of the single tickets and the night accommodation of a
    /// journey, or `None` when no price is known for its trains.
    fn journey_price(&self, journey: &Journey, stations: &[LegPlanStation]) -> Option<EuroRange>;
}

/// A pricer that knows no prices, for backends and tests that plan without fares.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoPrice;

impl JourneyPricer for NoPrice {
    fn journey_price(&self, _: &Journey, _: &[LegPlanStation]) -> Option<EuroRange> {
        None
    }
}
