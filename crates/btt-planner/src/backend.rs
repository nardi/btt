//! The backends that can plan a leg.

use std::sync::Arc;

use btt_model::{LegPlanningResult, NoJourneyReason};
use tokio_util::sync::CancellationToken;

use crate::bundle_router::TimetableBundleRouter;
use crate::explanation::LegExplanation;
use crate::input::{LegChainInput, LegPlanningInput};
use crate::pricing::{JourneyPricer, NoPrice};
#[cfg(feature = "stub")]
use crate::stub::StubBackend;

/// Revision of the planner's results. Cached results are `postcard` bytes, which don't
/// survive a change to the layout of `LegPlanningResult`, and a change to the planning logic
/// makes them stale. Either change raises this number, so the app drops the old results.
const LEG_PLAN_REVISION: u64 = 17;

/// Plans single legs. An enum instead of a trait object, because the set of backends is fixed
/// at compile time.
pub enum LegPlanningBackend<P: JourneyPricer = NoPrice> {
    /// Invents one direct train per leg from the straight-line distance.
    #[cfg(feature = "stub")]
    Stub(StubBackend),
    /// Searches journeys with RAPTOR on a timetable bundle.
    TimetableBundle(Arc<TimetableBundleRouter<P>>),
}

impl<P: JourneyPricer> LegPlanningBackend<P> {
    /// Returns a value that separates this backend's cached results from other backends'.
    ///
    /// The bundle backend includes the bundle version, so a new version never returns results
    /// from the old one. Every backend includes `LEG_PLAN_REVISION`, so results of an older
    /// planner are never read back either.
    pub fn cache_namespace(&self) -> u64 {
        let backend = match self {
            #[cfg(feature = "stub")]
            Self::Stub(_) => 1,
            Self::TimetableBundle(router) => (2 << 32) | u64::from(router.bundle_version()),
        };
        (LEG_PLAN_REVISION << 40) | backend
    }

    /// Plans one leg.
    ///
    /// The bundle backend searches on a blocking thread and stops early when the cancellation
    /// token fires. The caller drops whatever a cancelled search returns.
    pub async fn plan_leg(
        &self,
        input: &LegPlanningInput,
        cancellation: &CancellationToken,
    ) -> LegPlanningResult {
        match self {
            #[cfg(feature = "stub")]
            Self::Stub(stub) => {
                // The stub answers in microseconds, so it never needs to stop early.
                let _ = cancellation;
                stub.plan_leg(input)
            }
            Self::TimetableBundle(router) => {
                let router = Arc::clone(router);
                let input = input.clone();
                let cancellation = cancellation.clone();
                let search = tokio::task::spawn_blocking(move || {
                    router.plan_leg(&input, || cancellation.is_cancelled())
                });
                search.await.unwrap_or(LegPlanningResult::NoJourneyFound(
                    NoJourneyReason::BackendUnavailable,
                ))
            }
        }
    }

    /// Plans the legs of a chain joined by same-day stops, one result per leg. The stub plans
    /// each leg on its own.
    pub async fn plan_chain(
        &self,
        chain: &LegChainInput,
        cancellation: &CancellationToken,
    ) -> Vec<LegPlanningResult> {
        match self {
            #[cfg(feature = "stub")]
            Self::Stub(stub) => {
                let _ = cancellation;
                chain
                    .legs
                    .iter()
                    .map(|leg| stub.plan_leg(&leg.input))
                    .collect()
            }
            Self::TimetableBundle(router) => {
                let router = Arc::clone(router);
                let chain = chain.clone();
                let leg_count = chain.legs.len();
                let cancellation = cancellation.clone();
                let search = tokio::task::spawn_blocking(move || {
                    router.plan_chain(&chain, || cancellation.is_cancelled())
                });
                search.await.unwrap_or_else(|_| {
                    vec![
                        LegPlanningResult::NoJourneyFound(NoJourneyReason::BackendUnavailable);
                        leg_count
                    ]
                })
            }
        }
    }

    /// Plans one leg and explains the result, for problem reports. The stub has nothing to
    /// explain beyond its result.
    pub async fn explain_leg(&self, input: &LegPlanningInput) -> LegExplanation {
        match self {
            #[cfg(feature = "stub")]
            Self::Stub(stub) => LegExplanation::without_search(stub.plan_leg(input)),
            Self::TimetableBundle(router) => {
                let router = Arc::clone(router);
                let input = input.clone();
                tokio::task::spawn_blocking(move || router.explain_leg(&input))
                    .await
                    .unwrap_or_else(|_| {
                        LegExplanation::without_search(LegPlanningResult::NoJourneyFound(
                            NoJourneyReason::BackendUnavailable,
                        ))
                    })
            }
        }
    }

    /// Returns a short name of the backend, for problem reports.
    pub fn kind_name(&self) -> &'static str {
        match self {
            #[cfg(feature = "stub")]
            Self::Stub(_) => "stub",
            Self::TimetableBundle(_) => "timetable bundle",
        }
    }
}
