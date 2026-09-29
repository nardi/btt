//! Plans the legs of one open trip and streams the results as they finish.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use btt_model::{LegPlanningResult, PlanningRunId, PlanningSessionEvent, Trip, TripPlaceId};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::backend::LegPlanningBackend;
use crate::input::{LegCacheKey, LegChainInput, leg_chain_inputs};

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
pub struct TripPlanningSession<S: PlanningEventSink, C: LegResultStore = NoLegResultStore> {
    /// State shared with the running planning task.
    shared: Arc<SessionShared<S, C>>,
}

/// Parts of a session that the planning task also uses.
struct SessionShared<S, C> {
    /// Plans single legs.
    backend: Arc<LegPlanningBackend>,
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

impl<S: PlanningEventSink> TripPlanningSession<S> {
    /// Creates a session that plans with a backend and sends events to a sink, and keeps
    /// results in memory only.
    pub fn new(backend: Arc<LegPlanningBackend>, event_sink: S, debounce: Duration) -> Self {
        Self::with_result_store(backend, event_sink, NoLegResultStore, debounce)
    }
}

impl<S: PlanningEventSink, C: LegResultStore> TripPlanningSession<S, C> {
    /// Creates a session that also looks up and keeps leg results in a store.
    pub fn with_result_store(
        backend: Arc<LegPlanningBackend>,
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

impl<S: PlanningEventSink, C: LegResultStore> Drop for TripPlanningSession<S, C> {
    fn drop(&mut self) {
        if let Some(run) = self.shared.lock_state().current_run.take() {
            run.cancellation.cancel();
            run.task.abort();
        }
    }
}

impl<S: PlanningEventSink, C: LegResultStore> SessionShared<S, C> {
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

        let mut state = self.lock_state();
        if !cancellation.is_cancelled() {
            state.current_run = None;
            self.event_sink
                .send_event(PlanningSessionEvent::PlanningFinished { run_id });
        }
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
        for (chain, keys) in changed_chains {
            let cached: Option<Vec<LegPlanningResult>> = keys
                .iter()
                .map(|key| {
                    state.cached_results.get(key).cloned().or_else(|| {
                        let stored = self.result_store.load(*key)?;
                        state.cached_results.insert(*key, stored.clone());
                        Some(stored)
                    })
                })
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
        StartedRun { run_id, uncached }
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
        /// Counts loads that found a result.
        #[derive(Clone, Default)]
        struct MemoryStore(Arc<Mutex<(FxHashMap<LegCacheKey, LegPlanningResult>, usize)>>);

        impl LegResultStore for MemoryStore {
            fn load(&self, key: LegCacheKey) -> Option<LegPlanningResult> {
                let mut contents = self.0.lock().unwrap();
                let found = contents.0.get(&key).cloned();
                contents.1 += usize::from(found.is_some());
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
        assert_eq!(
            store.0.lock().unwrap().1,
            3,
            "the second session loaded all three legs"
        );
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
}
