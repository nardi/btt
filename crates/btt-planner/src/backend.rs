//! The backends that can plan a leg.

use btt_model::LegPlanningResult;
use tokio_util::sync::CancellationToken;

use crate::input::LegPlanningInput;
#[cfg(feature = "stub")]
use crate::stub::StubBackend;

/// Plans single legs. An enum instead of a trait object, because the set of backends is fixed
/// at compile time.
pub enum LegPlanningBackend {
    /// Invents one direct train per leg from the straight-line distance.
    #[cfg(feature = "stub")]
    Stub(StubBackend),
}

impl LegPlanningBackend {
    /// Returns a value that separates this backend's cached results from other backends'.
    ///
    /// Backends built on timetable data include the data version, so a new version never
    /// returns results from the old one.
    pub fn cache_namespace(&self) -> u64 {
        match *self {
            #[cfg(feature = "stub")]
            Self::Stub(_) => 1,
        }
    }

    /// Plans one leg.
    ///
    /// Slow backends stop early when the cancellation token fires, and the caller drops
    /// whatever they return in that case.
    #[allow(clippy::unused_async, reason = "network and timetable backends await")]
    #[cfg_attr(
        not(feature = "stub"),
        allow(unused_variables, reason = "no backend is enabled to use them")
    )]
    pub async fn plan_leg(
        &self,
        input: &LegPlanningInput,
        cancellation: &CancellationToken,
    ) -> LegPlanningResult {
        match *self {
            #[cfg(feature = "stub")]
            Self::Stub(ref stub) => {
                // The stub answers in microseconds, so it never needs to stop early.
                let _ = cancellation;
                stub.plan_leg(input)
            }
        }
    }
}
