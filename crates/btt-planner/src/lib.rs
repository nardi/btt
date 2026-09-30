//! Plans the legs of a trip and streams the results to the frontend.

pub mod backend;
pub mod bundle_router;
mod chain;
mod cost_model;
pub mod explanation;
pub mod input;
mod ride_path;
pub mod scoring;
pub mod session;
#[cfg(feature = "stub")]
pub mod stub;

pub use backend::LegPlanningBackend;
pub use bundle_router::TimetableBundleRouter;
pub use explanation::{CandidateExplanation, LegExplanation, RankedJourneyExplanation};
pub use input::{
    IdentifiedLegInput, LegCacheKey, LegChainInput, LegEndpoint, LegPlanningInput, SameDayStop,
    leg_chain_inputs, leg_planning_inputs,
};
pub use session::{
    DEFAULT_PLANNING_DEBOUNCE, LegResultStore, NoLegResultStore, PlanningEventSink,
    TripPlanningSession,
};
#[cfg(feature = "stub")]
pub use stub::StubBackend;
