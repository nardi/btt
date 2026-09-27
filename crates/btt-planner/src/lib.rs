//! Plans the legs of a trip and streams the results to the frontend.

pub mod backend;
pub mod bundle_router;
pub mod input;
pub mod session;
#[cfg(feature = "stub")]
pub mod stub;

pub use backend::LegPlanningBackend;
pub use bundle_router::TimetableBundleRouter;
pub use input::{LegCacheKey, LegEndpoint, LegPlanningInput, leg_planning_inputs};
pub use session::{DEFAULT_PLANNING_DEBOUNCE, PlanningEventSink, TripPlanningSession};
#[cfg(feature = "stub")]
pub use stub::StubBackend;
