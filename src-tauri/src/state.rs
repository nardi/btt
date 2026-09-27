//! State the app shares between commands.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use btt_geo::{PUBLIC_PHOTON_BASE_URL, PhotonClient, TimeZoneFinder};
use btt_model::{PlanningSessionEvent, Trip, TripId};
use btt_planner::{
    DEFAULT_PLANNING_DEBOUNCE, LegPlanningBackend, PlanningEventSink, StubBackend,
    TripPlanningSession,
};
use btt_storage::Storage;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use tauri::ipc::Channel;

/// `User-Agent` for requests to public services, which ask clients to identify themselves.
const USER_AGENT: &str = concat!(
    "BigTrainTrip/",
    env!("CARGO_PKG_VERSION"),
    " (open-source train trip planner)"
);

/// Sends planning events to the frontend through the channel it passed to `open_trip`.
pub struct ChannelEventSink(pub Channel<PlanningSessionEvent>);

impl PlanningEventSink for ChannelEventSink {
    fn send_event(&self, event: PlanningSessionEvent) {
        // The channel only fails when the window has closed, and then nobody needs the event.
        let _ = self.0.send(event);
    }
}

/// A trip the frontend has open, with the session that plans it.
pub struct OpenTrip {
    /// The current state of the trip, which every edit starts from.
    pub trip: Trip,
    /// Plans the trip and streams results to the frontend.
    pub planning_session: TripPlanningSession<ChannelEventSink>,
}

/// Everything the commands share.
pub struct AppState {
    /// The app's database.
    pub storage: Storage,
    /// Plans single legs for every planning session.
    pub planning_backend: Arc<LegPlanningBackend>,
    /// Searches places by name and position.
    pub photon: PhotonClient,
    /// Language for place names, as a two-letter code.
    pub language: SmolStr,
    /// Trips the frontend has open. Edits lock this map, which keeps edits to one trip in
    /// order.
    open_trips: Mutex<FxHashMap<TripId, OpenTrip>>,
}

impl AppState {
    /// Creates the shared state around an opened database.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client for Photon cannot be built.
    pub fn new(storage: Storage) -> Result<Self, btt_geo::PhotonError> {
        let time_zone_finder = Arc::new(TimeZoneFinder::new());
        let planning_backend =
            Arc::new(LegPlanningBackend::Stub(StubBackend::new(time_zone_finder)));
        Ok(Self {
            storage,
            planning_backend,
            photon: PhotonClient::new(PUBLIC_PHOTON_BASE_URL, USER_AGENT)?,
            // Place names come in English until the app has a language setting.
            language: "en".into(),
            open_trips: Mutex::new(FxHashMap::default()),
        })
    }

    /// Locks the open trips. Every section that holds the lock replaces whole values, so a
    /// poisoned lock is still consistent.
    pub fn open_trips(&self) -> MutexGuard<'_, FxHashMap<TripId, OpenTrip>> {
        self.open_trips
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts a planning session for a trip and registers it as open. Replaces an earlier
    /// session for the same trip, for example after the frontend reloaded.
    pub fn open_trip(&self, trip: Trip, events: Channel<PlanningSessionEvent>) {
        let planning_session = TripPlanningSession::new(
            Arc::clone(&self.planning_backend),
            ChannelEventSink(events),
            DEFAULT_PLANNING_DEBOUNCE,
        );
        planning_session.trip_changed(trip.clone());
        self.open_trips().insert(
            trip.id,
            OpenTrip {
                trip,
                planning_session,
            },
        );
    }
}
