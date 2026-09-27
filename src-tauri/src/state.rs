//! State the app shares between commands.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use btt_geo::{PUBLIC_PHOTON_BASE_URL, PhotonClient, TimeZoneFinder};
use btt_model::{PlanningSessionEvent, Trip, TripEdit, TripEditOutcome, TripId};
use btt_planner::{
    DEFAULT_PLANNING_DEBOUNCE, LegPlanningBackend, PlanningEventSink, StubBackend,
    TimetableBundleRouter, TripPlanningSession,
};
use btt_storage::Storage;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use tauri::ipc::Channel;

use crate::commands::CommandError;
use crate::place_naming::{has_provisional_name, provisional_place_name};

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
    /// Plans with the timetable bundle `find_bundle` returns, or with the stub backend when
    /// there is none or it fails to load.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client for Photon cannot be built.
    pub fn new(storage: Storage, data_directory: &Path) -> Result<Self, btt_geo::PhotonError> {
        let time_zone_finder = Arc::new(TimeZoneFinder::new());
        let bundle_router = find_bundle(data_directory).and_then(|path| {
            match TimetableBundleRouter::open(&path, Arc::clone(&time_zone_finder)) {
                Ok(router) => {
                    tracing::info!(path = %path.display(), version = router.bundle_version(), "planning with a timetable bundle");
                    Some(router)
                }
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "the timetable bundle failed to load");
                    None
                }
            }
        });
        let planning_backend = Arc::new(if let Some(router) = bundle_router {
            LegPlanningBackend::TimetableBundle(Arc::new(router))
        } else {
            tracing::info!("no timetable bundle, planning with made-up trains");
            LegPlanningBackend::Stub(StubBackend::new(time_zone_finder))
        });
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

    /// Applies an edit from the frontend to an open trip, saves it and replans.
    ///
    /// Moving a place that still has its provisional name also updates that name, so it keeps
    /// matching the coordinates and reverse geocoding can replace it.
    ///
    /// # Errors
    ///
    /// Returns an error when the trip is not open, the edit does not fit or saving fails.
    pub fn apply_edit_from_frontend(
        &self,
        trip_id: TripId,
        edit: TripEdit,
    ) -> Result<TripEditOutcome, CommandError> {
        let mut open_trips = self.open_trips();
        let open_trip = open_trips
            .get_mut(&trip_id)
            .ok_or(CommandError::TripNotOpen(trip_id))?;
        let mut trip = open_trip.trip.clone();
        let provisional_rename = match &edit {
            TripEdit::MovePlace {
                place_id,
                coordinates,
            } if trip
                .itinerary
                .place(*place_id)
                .is_some_and(has_provisional_name) =>
            {
                Some(TripEdit::RenamePlace {
                    place_id: *place_id,
                    name: provisional_place_name(*coordinates),
                })
            }
            _ => None,
        };
        let mut notices = trip.apply_edit(edit).map_err(CommandError::InvalidEdit)?;
        if let Some(rename) = provisional_rename {
            notices.extend(trip.apply_edit(rename).map_err(CommandError::InvalidEdit)?);
        }
        // Saving under the lock keeps the stored order of edits the same as the applied order.
        self.storage.save_trip(&trip)?;
        open_trip.trip = trip.clone();
        open_trip.planning_session.trip_changed(trip.clone());
        Ok(TripEditOutcome { trip, notices })
    }

    /// Applies an edit that the backend makes on its own, such as a name from reverse geocoding,
    /// and sends the changed trip to the frontend.
    ///
    /// Does nothing when the trip is no longer open or `still_applies` returns `false`, because
    /// the user may have changed the trip since the backend started its work.
    ///
    /// # Errors
    ///
    /// Returns an error when the edit does not fit or saving fails.
    pub fn apply_edit_from_backend(
        &self,
        trip_id: TripId,
        edit: TripEdit,
        still_applies: impl FnOnce(&Trip) -> bool,
    ) -> Result<(), CommandError> {
        let mut open_trips = self.open_trips();
        let Some(open_trip) = open_trips.get_mut(&trip_id) else {
            return Ok(());
        };
        if !still_applies(&open_trip.trip) {
            return Ok(());
        }
        let mut trip = open_trip.trip.clone();
        trip.apply_edit(edit).map_err(CommandError::InvalidEdit)?;
        self.storage.save_trip(&trip)?;
        open_trip.trip = trip.clone();
        open_trip.planning_session.trip_changed_elsewhere(trip);
        Ok(())
    }
}

/// Environment variable that points the app at a timetable bundle file, for development.
const BUNDLE_PATH_VARIABLE: &str = "BTT_TIMETABLE_BUNDLE";

/// Returns the timetable bundle to plan with: the file `BTT_TIMETABLE_BUNDLE` names, or else
/// the bundle with the highest version in the `bundles` folder of the app data directory.
///
/// Bundle file names end in their version as `yyyymmddNN`, so the highest name is the newest.
fn find_bundle(data_directory: &Path) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BUNDLE_PATH_VARIABLE) {
        return Some(PathBuf::from(path));
    }
    let entries = std::fs::read_dir(data_directory.join("bundles")).ok()?;
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "bundle")
        })
        .max()
}
