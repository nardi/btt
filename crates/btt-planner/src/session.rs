//! Plans the legs of one open trip and streams the results as they finish.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use btt_model::{
    LegPlan, LegPlanningResult, NearbyDaySuggestion, PlanningRunId, PlanningSessionEvent, Trip,
    TripPlaceId,
};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::backend::LegPlanningBackend;
use crate::input::{LegCacheKey, LegChainInput, leg_chain_inputs};
use crate::pricing::{JourneyPricer, NoPrice};

/// Time a session waits after the last trip change before it starts planning, so a burst of
/// edits leads to one run.
pub const DEFAULT_PLANNING_DEBOUNCE: Duration = Duration::from_millis(250);

/// Largest number of leg results the session keeps in memory before it clears them.
const MAXIMUM_CACHED_LEG_RESULTS: usize = 512;

/// Receives the events of a planning session, such as a Tauri channel to the frontend.
pub trait PlanningEventSink: Send + Sync + 'static {
    /// Delivers one event. Called while the session holds its lock, so events arrive in order.
    fn send_event(&self, event: PlanningSessionEvent);
}

/// Keeps leg results beyond a session, so reopening a trip or restarting the app needs no
/// replanning.
pub trait LegResultStore: Send + Sync + 'static {
    /// Returns the result stored under a key, if any. Failures count as a miss.
    fn load(&self, key: LegCacheKey) -> Option<LegPlanningResult>;

    /// Stores a result under a key and the backend namespace it belongs to. Failures are only
    /// logged, because the result is still delivered.
    fn store(&self, key: LegCacheKey, namespace: u64, result: &LegPlanningResult);
}

/// A result store that keeps nothing, for sessions that only cache in memory.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoLegResultStore;

impl LegResultStore for NoLegResultStore {
    fn load(&self, _: LegCacheKey) -> Option<LegPlanningResult> {
        None
    }

    fn store(&self, _: LegCacheKey, _: u64, _: &LegPlanningResult) {}
}

/// Plans the legs of one open trip.
///
/// Each trip change cancels the running plan, waits for the debounce time and then replans the
/// legs whose inputs differ from the ones the frontend has results for. Results stream to the
/// event sink leg by leg.
pub struct TripPlanningSession<
    S: PlanningEventSink,
    C: LegResultStore = NoLegResultStore,
    P: JourneyPricer = NoPrice,
> {
    /// State shared with the running planning task.
    shared: Arc<SessionShared<S, C, P>>,
}

/// Parts of a session that the planning task also uses.
struct SessionShared<S, C, P: JourneyPricer> {
    /// Plans single legs.
    backend: Arc<LegPlanningBackend<P>>,
    /// Receives the session's events.
    event_sink: S,
    /// Keeps leg results beyond the session, such as in the app database.
    result_store: C,
    /// Time to wait after a change before planning.
    debounce: Duration,
    /// Mutable state, locked only for short, non-blocking sections.
    state: Mutex<SessionState>,
}

/// Mutable state of a session.
#[derive(Default)]
struct SessionState {
    /// Identifier for the next run.
    next_run_id: u32,
    /// Cache key of the result the frontend holds for each leg, keyed by arrival place.
    delivered_leg_keys: SmallVec<[(TripPlaceId, LegCacheKey); 16]>,
    /// Results of earlier runs, so undoing an edit needs no replanning.
    cached_results: FxHashMap<LegCacheKey, LegPlanningResult>,
    /// The run that is waiting or planning, if any.
    current_run: Option<CurrentRun>,
}

/// A run that has not finished yet.
struct CurrentRun {
    /// Fires when a newer change replaces this run.
    cancellation: CancellationToken,
    /// The task that waits for the debounce time and then plans.
    task: JoinHandle<()>,
}

impl<S: PlanningEventSink, P: JourneyPricer> TripPlanningSession<S, NoLegResultStore, P> {
    /// Creates a session that plans with a backend and sends events to a sink, and keeps
    /// results in memory only.
    pub fn new(backend: Arc<LegPlanningBackend<P>>, event_sink: S, debounce: Duration) -> Self {
        Self::with_result_store(backend, event_sink, NoLegResultStore, debounce)
    }
}

impl<S: PlanningEventSink, C: LegResultStore, P: JourneyPricer> TripPlanningSession<S, C, P> {
    /// Creates a session that also looks up and keeps leg results in a store.
    pub fn with_result_store(
        backend: Arc<LegPlanningBackend<P>>,
        event_sink: S,
        result_store: C,
        debounce: Duration,
    ) -> Self {
        Self {
            shared: Arc::new(SessionShared {
                backend,
                event_sink,
                result_store,
                debounce,
                state: Mutex::new(SessionState::default()),
            }),
        }
    }

    /// Replaces the running plan with one for the changed trip. Must be called from within a
    /// Tokio runtime.
    pub fn trip_changed(&self, trip: Trip) {
        let mut state = self.shared.lock_state();
        if let Some(previous_run) = state.current_run.take() {
            previous_run.cancellation.cancel();
            previous_run.task.abort();
        }
        let cancellation = CancellationToken::new();
        let task = tokio::spawn(Arc::clone(&self.shared).run(trip, cancellation.clone()));
        state.current_run = Some(CurrentRun { cancellation, task });
    }

    /// Sends a trip change that did not come from the frontend, such as a name from reverse
    /// geocoding, and replans it.
    pub fn trip_changed_elsewhere(&self, trip: Trip) {
        self.shared
            .event_sink
            .send_event(PlanningSessionEvent::TripChanged(Box::new(trip.clone())));
        self.trip_changed(trip);
    }
}

impl<S: PlanningEventSink, C: LegResultStore, P: JourneyPricer> Drop
    for TripPlanningSession<S, C, P>
{
    fn drop(&mut self) {
        if let Some(run) = self.shared.lock_state().current_run.take() {
            run.cancellation.cancel();
            run.task.abort();
        }
    }
}

impl<S: PlanningEventSink, C: LegResultStore, P: JourneyPricer> SessionShared<S, C, P> {
    /// Locks the state. A panic in another holder leaves the state consistent, because every
    /// section only replaces whole values, so a poisoned lock is used as is.
    fn lock_state(&self) -> MutexGuard<'_, SessionState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Waits for the debounce time, then plans the legs whose inputs changed.
    async fn run(self: Arc<Self>, trip: Trip, cancellation: CancellationToken) {
        tokio::time::sleep(self.debounce).await;
        let namespace = self.backend.cache_namespace();
        let legs_to_plan = {
            let mut state = self.lock_state();
            if cancellation.is_cancelled() {
                return;
            }
            self.start_run(&mut state, &trip, namespace)
        };
        let run_id = legs_to_plan.run_id;

        let mut planning_tasks = JoinSet::new();
        for chain in legs_to_plan.uncached {
            let backend = Arc::clone(&self.backend);
            let cancellation = cancellation.clone();
            planning_tasks.spawn(async move {
                let results = backend.plan_chain(&chain, &cancellation).await;
                (chain, results)
            });
        }
        while let Some(joined) = planning_tasks.join_next().await {
            let Ok((chain, results)) = joined else {
                continue;
            };
            let mut state = self.lock_state();
            if cancellation.is_cancelled() {
                return;
            }
            for (index, (leg, result)) in chain.legs.iter().zip(results).enumerate() {
                tracing::info!(run = run_id.0, arrival_place = ?leg.arrival_place_id, "planned leg");
                let key = chain.leg_cache_key(namespace, index);
                if state.cached_results.len() >= MAXIMUM_CACHED_LEG_RESULTS {
                    state.cached_results.clear();
                }
                state.cached_results.insert(key, result.clone());
                self.result_store.store(key, namespace, &result);
                self.deliver(&mut state, run_id, leg.arrival_place_id, key, result);
            }
        }

        {
            let state = self.lock_state();
            if cancellation.is_cancelled() {
                return;
            }
            self.event_sink
                .send_event(PlanningSessionEvent::PlanningFinished { run_id });
            drop(state);
        }
        self.suggest_nearby_days(run_id, &legs_to_plan.replanned, namespace, &cancellation)
            .await;
        let mut state = self.lock_state();
        if !cancellation.is_cancelled() {
            state.current_run = None;
        }
    }

    /// Plans every replanned chain a day earlier and a day later, and suggests the other day
    /// for a leg that is much better there. This runs after `PlanningFinished`, so the plan
    /// itself doesn't wait for it.
    async fn suggest_nearby_days(
        &self,
        run_id: PlanningRunId,
        chains: &[LegChainInput],
        namespace: u64,
        cancellation: &CancellationToken,
    ) {
        let mut tasks = JoinSet::new();
        for chain in chains {
            for day_offset in [-1_i8, 1] {
                let Some(shifted) = shifted_chain(chain, day_offset) else {
                    continue;
                };
                let keys: SmallVec<[LegCacheKey; 4]> = (0..shifted.legs.len())
                    .map(|index| shifted.leg_cache_key(namespace, index))
                    .collect();
                let cached: Option<Vec<LegPlanningResult>> = {
                    let mut state = self.lock_state();
                    keys.iter()
                        .map(|key| self.cached_result(&mut state, *key))
                        .collect()
                };
                let current_keys: SmallVec<[LegCacheKey; 4]> = (0..chain.legs.len())
                    .map(|index| chain.leg_cache_key(namespace, index))
                    .collect();
                let backend = Arc::clone(&self.backend);
                let cancellation = cancellation.clone();
                tasks.spawn(async move {
                    let (results, planned) = match cached {
                        Some(results) => (results, false),
                        None => (backend.plan_chain(&shifted, &cancellation).await, true),
                    };
                    (shifted, keys, current_keys, day_offset, results, planned)
                });
            }
        }

        let mut best_by_place: SmallVec<[(TripPlaceId, NearbyDaySuggestion); 8]> = SmallVec::new();
        while let Some(joined) = tasks.join_next().await {
            let Ok((shifted, keys, current_keys, day_offset, results, planned)) = joined else {
                continue;
            };
            let mut state = self.lock_state();
            if cancellation.is_cancelled() {
                return;
            }
            for (index, (leg, result)) in shifted.legs.iter().zip(results).enumerate() {
                if planned {
                    state.cached_results.insert(keys[index], result.clone());
                    self.result_store.store(keys[index], namespace, &result);
                }
                let LegPlanningResult::Found(plan) = result else {
                    continue;
                };
                let current = state.cached_results.get(&current_keys[index]);
                if !is_much_better(current, &plan) {
                    continue;
                }
                let suggestion = NearbyDaySuggestion {
                    day_offset,
                    departure_date: leg.input.departure_date,
                    plan,
                };
                match best_by_place
                    .iter_mut()
                    .find(|(place_id, _)| *place_id == leg.arrival_place_id)
                {
                    Some((_, best))
                        if best.plan.best_journey_cost > suggestion.plan.best_journey_cost =>
                    {
                        *best = suggestion;
                    }
                    Some(_) => {}
                    None => best_by_place.push((leg.arrival_place_id, suggestion)),
                }
            }
        }

        let _state = self.lock_state();
        if cancellation.is_cancelled() {
            return;
        }
        for (arrival_place_id, suggestion) in best_by_place {
            self.event_sink
                .send_event(PlanningSessionEvent::NearbyDaySuggested {
                    run_id,
                    arrival_place_id,
                    suggestion,
                });
        }
    }

    /// Returns a result from memory or else from the result store, keeping it in memory.
    fn cached_result(
        &self,
        state: &mut SessionState,
        key: LegCacheKey,
    ) -> Option<LegPlanningResult> {
        state.cached_results.get(&key).cloned().or_else(|| {
            let stored = self.result_store.load(key)?;
            state.cached_results.insert(key, stored.clone());
            Some(stored)
        })
    }

    /// Starts a run. Announces the legs it replans, delivers the ones it finds in the cache
    /// right away and returns the chains that still need planning.
    ///
    /// Legs joined by same-day stops are planned as a chain, and a change to one leg of a chain
    /// changes the cache keys of all its legs, so a chain is always replanned as a whole.
    fn start_run(&self, state: &mut SessionState, trip: &Trip, namespace: u64) -> StartedRun {
        let run_id = PlanningRunId(state.next_run_id);
        state.next_run_id = state.next_run_id.wrapping_add(1);

        let chains = leg_chain_inputs(trip);
        // Legs of removed places no longer exist, so their delivered keys go.
        state.delivered_leg_keys.retain(|(place_id, _)| {
            chains
                .iter()
                .flat_map(|chain| &chain.legs)
                .any(|leg| leg.arrival_place_id == *place_id)
        });
        let changed_chains: SmallVec<[(LegChainInput, SmallVec<[LegCacheKey; 4]>); 8]> = chains
            .into_iter()
            .map(|chain| {
                let keys: SmallVec<[LegCacheKey; 4]> = (0..chain.legs.len())
                    .map(|index| chain.leg_cache_key(namespace, index))
                    .collect();
                (chain, keys)
            })
            .filter(|(chain, keys)| {
                chain.legs.iter().zip(keys).any(|(leg, key)| {
                    !state
                        .delivered_leg_keys
                        .contains(&(leg.arrival_place_id, *key))
                })
            })
            .collect();

        self.event_sink
            .send_event(PlanningSessionEvent::PlanningStarted {
                run_id,
                replanned_arrival_place_ids: changed_chains
                    .iter()
                    .flat_map(|(chain, _)| chain.legs.iter().map(|leg| leg.arrival_place_id))
                    .collect(),
            });

        let mut uncached = SmallVec::new();
        let mut replanned = SmallVec::new();
        for (chain, keys) in changed_chains {
            replanned.push(chain.clone());
            let cached: Option<Vec<LegPlanningResult>> = keys
                .iter()
                .map(|key| self.cached_result(state, *key))
                .collect();
            match cached {
                Some(results) => {
                    for ((leg, key), result) in chain.legs.iter().zip(keys).zip(results) {
                        self.deliver(state, run_id, leg.arrival_place_id, key, result);
                    }
                }
                None => uncached.push(chain),
            }
        }
        StartedRun {
            run_id,
            uncached,
            replanned,
        }
    }

    /// Records that the frontend holds a result for a leg and sends it.
    fn deliver(
        &self,
        state: &mut SessionState,
        run_id: PlanningRunId,
        arrival_place_id: TripPlaceId,
        key: LegCacheKey,
        result: LegPlanningResult,
    ) {
        match state
            .delivered_leg_keys
            .iter_mut()
            .find(|(place_id, _)| *place_id == arrival_place_id)
        {
            Some(entry) => entry.1 = key,
            None => state.delivered_leg_keys.push((arrival_place_id, key)),
        }
        self.event_sink
            .send_event(PlanningSessionEvent::LegPlanned {
                run_id,
                arrival_place_id,
                result,
            });
    }
}

/// A run after its start, with the legs that still need the backend.
struct StartedRun {
    /// Identifies the run.
    run_id: PlanningRunId,
    /// Changed chains without cached results.
    uncached: SmallVec<[LegChainInput; 8]>,
    /// Every changed chain, with or without a cached result.
    replanned: SmallVec<[LegChainInput; 8]>,
}

/// Smallest gain in cost, in minutes of train travel, for which another day is suggested.
const NEARBY_DAY_MINIMUM_GAIN: u32 = 90;

/// Largest cost of another day, in percent of the current day's cost, for which it is
/// suggested. Together with `NEARBY_DAY_MINIMUM_GAIN`, a small saving on a long leg and a
/// large share of a short one both stay quiet.
const NEARBY_DAY_MAXIMUM_COST_PERCENT: u32 = 85;

/// Returns a chain with every leg moved by a number of days. Picks don't carry over, because
/// they name trains at a time on the original day. Returns `None` at the edge of the calendar.
fn shifted_chain(chain: &LegChainInput, day_offset: i8) -> Option<LegChainInput> {
    let mut shifted = chain.clone();
    for leg in &mut shifted.legs {
        let days = chrono::Days::new(u64::from(day_offset.unsigned_abs()));
        leg.input.departure_date = if day_offset < 0 {
            leg.input.departure_date.checked_sub_days(days)?
        } else {
            leg.input.departure_date.checked_add_days(days)?
        };
        leg.input.chosen_journey = None;
    }
    Some(shifted)
}

/// Returns whether a plan on another day is much better than the current result of a leg,
/// which holds when the current day has no journey or costs at least
/// `NEARBY_DAY_MINIMUM_GAIN` minutes and `100 - NEARBY_DAY_MAXIMUM_COST_PERCENT` percent more.
fn is_much_better(current: Option<&LegPlanningResult>, other: &LegPlan) -> bool {
    match current {
        Some(LegPlanningResult::Found(current)) => {
            let other_cost = other.best_journey_cost;
            let current_cost = current.best_journey_cost;
            other_cost + NEARBY_DAY_MINIMUM_GAIN <= current_cost
                && u64::from(other_cost) * 100
                    <= u64::from(current_cost) * u64::from(NEARBY_DAY_MAXIMUM_COST_PERCENT)
        }
        Some(LegPlanningResult::NoJourneyFound(_)) => true,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use btt_geo::TimeZoneFinder;
    use btt_model::{GeoCoordinates, PlaceStay, TripEdit};
    use tokio::sync::mpsc;

    use super::*;
    use crate::input::tests::{FOUR_CITIES, place_id, trip_with_places};
    use crate::stub::StubBackend;

    /// Forwards events into a channel the test reads from.
    struct ChannelSink(mpsc::UnboundedSender<PlanningSessionEvent>);

    impl PlanningEventSink for ChannelSink {
        fn send_event(&self, event: PlanningSessionEvent) {
            let _ = self.0.send(event);
        }
    }

    fn stub_backend() -> Arc<LegPlanningBackend> {
        static TIME_ZONE_FINDER: OnceLock<Arc<TimeZoneFinder>> = OnceLock::new();
        let finder = TIME_ZONE_FINDER.get_or_init(|| Arc::new(TimeZoneFinder::new()));
        Arc::new(LegPlanningBackend::Stub(StubBackend::new(Arc::clone(
            finder,
        ))))
    }

    fn session() -> (
        TripPlanningSession<ChannelSink>,
        mpsc::UnboundedReceiver<PlanningSessionEvent>,
    ) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let session = TripPlanningSession::new(
            stub_backend(),
            ChannelSink(sender),
            DEFAULT_PLANNING_DEBOUNCE,
        );
        (session, receiver)
    }

    /// Collects the events of one run up to its `PlanningFinished`.
    async fn events_of_next_run(
        receiver: &mut mpsc::UnboundedReceiver<PlanningSessionEvent>,
    ) -> Vec<PlanningSessionEvent> {
        let mut events = Vec::new();
        while let Some(event) = receiver.recv().await {
            let finished = matches!(event, PlanningSessionEvent::PlanningFinished { .. });
            events.push(event);
            if finished {
                return events;
            }
        }
        panic!("the session stopped before the run finished");
    }

    fn replanned_places(events: &[PlanningSessionEvent]) -> Vec<TripPlaceId> {
        match &events[0] {
            PlanningSessionEvent::PlanningStarted {
                replanned_arrival_place_ids,
                ..
            } => replanned_arrival_place_ids.clone(),
            other => panic!("expected PlanningStarted, got {other:?}"),
        }
    }

    fn planned_places(events: &[PlanningSessionEvent]) -> Vec<TripPlaceId> {
        let mut places: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                PlanningSessionEvent::LegPlanned {
                    arrival_place_id, ..
                } => Some(*arrival_place_id),
                _ => None,
            })
            .collect();
        places.sort();
        places
    }

    #[tokio::test(start_paused = true)]
    async fn first_run_plans_every_leg() {
        let (session, mut receiver) = session();
        let trip = trip_with_places(&FOUR_CITIES);
        session.trip_changed(trip.clone());
        let events = events_of_next_run(&mut receiver).await;
        let expected: Vec<_> = (1..4).map(|index| place_id(&trip, index)).collect();
        assert_eq!(replanned_places(&events), expected);
        assert_eq!(planned_places(&events), expected);
    }

    #[tokio::test(start_paused = true)]
    async fn changing_nights_replans_only_later_legs() {
        let (session, mut receiver) = session();
        let mut trip = trip_with_places(&FOUR_CITIES);
        session.trip_changed(trip.clone());
        events_of_next_run(&mut receiver).await;

        let vienna = place_id(&trip, 2);
        let two_nights = PlaceStay::Nights(2.try_into().unwrap());
        trip.apply_edit(TripEdit::SetPlaceStay {
            place_id: vienna,
            stay: two_nights,
        })
        .unwrap();
        session.trip_changed(trip.clone());
        let events = events_of_next_run(&mut receiver).await;
        assert_eq!(replanned_places(&events), [place_id(&trip, 3)]);
    }

    #[tokio::test(start_paused = true)]
    async fn legs_around_a_same_day_stop_replan_together() {
        let (session, mut receiver) = session();
        let mut trip = trip_with_places(&FOUR_CITIES);
        let berlin = place_id(&trip, 1);
        trip.apply_edit(TripEdit::SetPlaceStay {
            place_id: berlin,
            stay: PlaceStay::SameDay {
                stay_range_override: None,
                prefers_longer_stay: false,
            },
        })
        .unwrap();
        session.trip_changed(trip.clone());
        events_of_next_run(&mut receiver).await;

        // Moving Amsterdam changes only the leg to Berlin, but the leg on from Berlin depends
        // on its arrival, so both are planned again. The leg from Vienna is not.
        let amsterdam = place_id(&trip, 0);
        let haarlem = GeoCoordinates::from_degrees(52.387_4, 4.638_1).unwrap();
        trip.apply_edit(TripEdit::MovePlace {
            place_id: amsterdam,
            coordinates: haarlem,
        })
        .unwrap();
        session.trip_changed(trip.clone());
        let events = events_of_next_run(&mut receiver).await;
        assert_eq!(replanned_places(&events), [berlin, place_id(&trip, 2)]);
    }

    #[tokio::test(start_paused = true)]
    async fn burst_of_edits_leads_to_one_run() {
        let (session, mut receiver) = session();
        let mut trip = trip_with_places(&FOUR_CITIES);
        let berlin = place_id(&trip, 1);
        for step in 0..5 {
            let latitude = 52.0 + f64::from(step) * 0.1;
            let coordinates = GeoCoordinates::from_degrees(latitude, 13.0).unwrap();
            trip.apply_edit(TripEdit::MovePlace {
                place_id: berlin,
                coordinates,
            })
            .unwrap();
            session.trip_changed(trip.clone());
        }
        let events = events_of_next_run(&mut receiver).await;
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, PlanningSessionEvent::PlanningStarted { .. }))
                .count(),
            1
        );
        assert!(
            receiver.try_recv().is_err(),
            "cancelled runs sent no events"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_session_answers_from_the_result_store() {
        /// Stored results, and the keys of loads that found one.
        type MemoryStoreContents = (FxHashMap<LegCacheKey, LegPlanningResult>, Vec<LegCacheKey>);

        /// Keeps results and records the keys of loads that found one.
        #[derive(Clone, Default)]
        struct MemoryStore(Arc<Mutex<MemoryStoreContents>>);

        impl LegResultStore for MemoryStore {
            fn load(&self, key: LegCacheKey) -> Option<LegPlanningResult> {
                let mut contents = self.0.lock().unwrap();
                let found = contents.0.get(&key).cloned();
                if found.is_some() {
                    contents.1.push(key);
                }
                found
            }

            fn store(&self, key: LegCacheKey, _: u64, result: &LegPlanningResult) {
                self.0.lock().unwrap().0.insert(key, result.clone());
            }
        }

        let store = MemoryStore::default();
        let trip = trip_with_places(&FOUR_CITIES);
        for _ in 0..2 {
            let (sender, mut receiver) = mpsc::unbounded_channel();
            let session = TripPlanningSession::with_result_store(
                stub_backend(),
                ChannelSink(sender),
                store.clone(),
                DEFAULT_PLANNING_DEBOUNCE,
            );
            session.trip_changed(trip.clone());
            let events = events_of_next_run(&mut receiver).await;
            assert_eq!(planned_places(&events).len(), 3);
        }
        // The second session loads the three legs from the store. The planning of nearby days
        // loads other keys too.
        let namespace = stub_backend().cache_namespace();
        let loaded = store.0.lock().unwrap().1.clone();
        for chain in crate::input::leg_chain_inputs(&trip) {
            for index in 0..chain.legs.len() {
                assert!(loaded.contains(&chain.leg_cache_key(namespace, index)));
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn undoing_an_edit_answers_from_the_cache() {
        let (session, mut receiver) = session();
        let mut trip = trip_with_places(&FOUR_CITIES);
        session.trip_changed(trip.clone());
        events_of_next_run(&mut receiver).await;

        let berlin = place_id(&trip, 1);
        let original = trip.itinerary.place(berlin).unwrap().coordinates;
        let potsdam = GeoCoordinates::from_degrees(52.391_3, 13.066_8).unwrap();
        trip.apply_edit(TripEdit::MovePlace {
            place_id: berlin,
            coordinates: potsdam,
        })
        .unwrap();
        session.trip_changed(trip.clone());
        events_of_next_run(&mut receiver).await;

        trip.apply_edit(TripEdit::MovePlace {
            place_id: berlin,
            coordinates: original,
        })
        .unwrap();
        session.trip_changed(trip.clone());
        let events = events_of_next_run(&mut receiver).await;
        assert_eq!(planned_places(&events).len(), 2);
    }

    #[test]
    fn only_a_much_better_day_is_suggested() {
        let plan_with_cost = |cost: u32| {
            let LegPlanningResult::Found(mut plan) = stub_backend_plan() else {
                panic!("the stub always finds a journey");
            };
            plan.best_journey_cost = cost;
            plan
        };
        let current = LegPlanningResult::Found(plan_with_cost(1000));
        // 90 minutes and 15% better is enough, less of either isn't.
        assert!(is_much_better(Some(&current), &plan_with_cost(850)));
        assert!(!is_much_better(Some(&current), &plan_with_cost(911)));
        let short = LegPlanningResult::Found(plan_with_cost(400));
        assert!(!is_much_better(Some(&short), &plan_with_cost(330)));
        let none =
            LegPlanningResult::NoJourneyFound(btt_model::NoJourneyReason::NoConnectionInWindow);
        assert!(is_much_better(Some(&none), &plan_with_cost(5000)));
    }

    #[test]
    fn a_shifted_chain_moves_every_leg_and_drops_picks() {
        let trip = trip_with_places(&FOUR_CITIES);
        let chain = crate::input::leg_chain_inputs(&trip)[0].clone();
        let later = shifted_chain(&chain, 1).unwrap();
        assert_eq!(
            later.legs[0].input.departure_date,
            chain.legs[0].input.departure_date.succ_opt().unwrap()
        );
        assert!(later.legs[0].input.chosen_journey.is_none());
    }

    /// Plans the first leg of the four cities with the stub.
    fn stub_backend_plan() -> LegPlanningResult {
        let trip = trip_with_places(&FOUR_CITIES);
        let input = &crate::input::leg_planning_inputs(&trip)[0].input;
        let backend = stub_backend();
        let LegPlanningBackend::Stub(stub) = backend.as_ref() else {
            panic!("the test backend is the stub");
        };
        stub.plan_leg(input)
    }
}
