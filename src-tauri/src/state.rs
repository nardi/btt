//! State the app shares between commands.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use btt_fares::{FareData, PriceBook};
use btt_geo::{PUBLIC_PHOTON_BASE_URL, PhotonClient, TimeZoneFinder};
use btt_model::{
    LegPlanningResult, PlanningSessionEvent, Trip, TripEdit, TripEditNotices, TripEditOutcome,
    TripHistory, TripId,
};
use btt_planner::{
    DEFAULT_PLANNING_DEBOUNCE, LegCacheKey, LegPlanningBackend, LegResultStore, PlanningEventSink,
    StubBackend, TimetableBundleRouter, TripPlanningSession,
};
use btt_quotes::QuoteStore;
use btt_quotes::sbb::SbbService;
use btt_storage::Storage;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use tauri::ipc::Channel;

use crate::commands::CommandError;
use crate::log_buffer::RecentLogLines;
use crate::place_naming::{has_provisional_name, provisional_place_name};
use crate::ticket_report::{LiveQuoteLogEntry, TicketPlanRecord};

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

/// Keeps planned legs in the app database, so reopened trips need no replanning.
#[derive(Clone)]
pub struct StorageLegResultStore(Arc<Storage>);

impl LegResultStore for StorageLegResultStore {
    fn load(&self, key: LegCacheKey) -> Option<LegPlanningResult> {
        self.0.cached_leg_result(key.0).unwrap_or_else(|error| {
            tracing::warn!(%error, "could not read a planned leg from the database");
            None
        })
    }

    fn store(&self, key: LegCacheKey, namespace: u64, result: &LegPlanningResult) {
        if let Err(error) = self.0.store_leg_result(key.0, namespace, result) {
            tracing::warn!(%error, "could not save a planned leg to the database");
        }
    }
}

/// Keeps quotes from booking sites in the app database, so they survive a restart.
struct StorageQuoteStore(Arc<Storage>);

impl QuoteStore for StorageQuoteStore {
    fn load(&self, key: &str, max_age: std::time::Duration) -> Option<String> {
        let max_age_seconds = i64::try_from(max_age.as_secs()).unwrap_or(i64::MAX);
        self.0
            .cached_quote(key, max_age_seconds)
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "could not read a quote from the database");
                None
            })
    }

    fn save(&self, key: &str, document: &str) {
        if let Err(error) = self.0.store_quote(key, document) {
            tracing::warn!(%error, "could not save a quote to the database");
        }
    }
}

/// A trip the frontend has open, with the session that plans it.
pub struct OpenTrip {
    /// The current state of the trip, which every edit starts from.
    pub trip: Trip,
    /// Plans the trip and streams results to the frontend.
    pub planning_session: TripPlanningSession<ChannelEventSink, StorageLegResultStore>,
    /// Earlier and undone states of the trip, for undo and redo. Kept in memory only, so it
    /// ends when the trip closes.
    pub history: TripHistory,
}

/// Everything the commands share.
pub struct AppState {
    /// The app's database, shared with the planning sessions.
    pub storage: Arc<Storage>,
    /// Plans single legs for every planning session.
    pub planning_backend: Arc<LegPlanningBackend>,
    /// Searches places by name and position.
    pub photon: PhotonClient,
    /// The pass list, reservation rules and price model for tickets.
    pub fare_data: FareData,
    /// The sources that price single tickets.
    pub price_book: PriceBook,
    /// Looks up prices on the SBB booking site. `None` when its HTTP client could not be built.
    pub sbb_service: Option<SbbService>,
    /// Looks up prices on the booking sites that are read in a hidden window.
    pub webview_service: Option<crate::webview_quotes::WebviewQuoteService>,
    /// Looks up prices on the Trenitalia site. `None` when its HTTP client could not be built.
    pub trenitalia_service: Option<btt_quotes::trenitalia::TrenitaliaService>,
    /// Looks up NS fares. `None` when its HTTP client could not be built.
    pub ns_service: Option<btt_quotes::ns::NsService>,
    /// Looks up prices on the service of the DB app. `None` when its HTTP client could not be
    /// built.
    pub db_service: Option<btt_quotes::db::DbService>,
    /// Looks up prices on the ÖBB shop. `None` when its HTTP client could not be built.
    pub obb_service: Option<btt_quotes::obb::ObbService>,
    /// The timetable bundle the planner loaded, if any.
    pub bundle_path: Option<PathBuf>,
    /// The most recent log lines, for problem reports.
    pub recent_log_lines: RecentLogLines,
    /// Language for place names, as a two-letter code.
    pub language: SmolStr,
    /// The last ticket plan of each trip, for problem reports.
    pub ticket_plans: Mutex<FxHashMap<TripId, TicketPlanRecord>>,
    /// The most recent live price lookups, for problem reports.
    pub live_quote_log: Mutex<Vec<LiveQuoteLogEntry>>,
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
    pub fn new(
        app: tauri::AppHandle,
        storage: Storage,
        data_directory: &Path,
        recent_log_lines: RecentLogLines,
    ) -> Result<Self, btt_geo::PhotonError> {
        let time_zone_finder = Arc::new(TimeZoneFinder::new());
        let bundle_path = find_bundle(data_directory);
        let bundle_router = bundle_path.clone().and_then(|path| {
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
        // Legs planned with another bundle version or backend can never match again.
        let namespace = planning_backend.cache_namespace();
        match storage.delete_leg_results_outside(namespace) {
            Ok(0) => {}
            Ok(deleted) => tracing::info!(deleted, "removed planned legs of an older timetable"),
            Err(error) => tracing::warn!(%error, "could not clean up planned legs"),
        }
        // The data files are compiled in and a test in `btt-fares` loads them, so this cannot fail.
        let fare_data = FareData::bundled().expect("the bundled fare data is valid");
        let price_book = PriceBook::from_data(&fare_data);
        let storage = Arc::new(storage);
        let quote_store: Arc<dyn QuoteStore> = Arc::new(StorageQuoteStore(Arc::clone(&storage)));
        let trenitalia_service =
            btt_quotes::trenitalia::TrenitaliaService::new(Arc::clone(&quote_store))
                .map_err(|error| tracing::warn!(%error, "live Trenitalia prices are not available"))
                .ok();
        let ns_service = btt_quotes::ns::NsService::new(Arc::clone(&quote_store))
            .map_err(|error| tracing::warn!(%error, "NS fares are not available"))
            .ok();
        let db_service = btt_quotes::db::DbService::new(Arc::clone(&quote_store))
            .map_err(|error| tracing::warn!(%error, "live DB prices are not available"))
            .ok();
        let obb_service = btt_quotes::obb::ObbService::new(Arc::clone(&quote_store))
            .map_err(|error| tracing::warn!(%error, "live ÖBB prices are not available"))
            .ok();
        let webview_service = Some(crate::webview_quotes::WebviewQuoteService::new(
            app,
            Arc::clone(&quote_store),
        ));
        let sbb_service = SbbService::new(quote_store)
            .map_err(|error| tracing::warn!(%error, "live SBB prices are not available"))
            .ok();
        Ok(Self {
            fare_data,
            price_book,
            sbb_service,
            webview_service,
            trenitalia_service,
            ns_service,
            db_service,
            obb_service,
            storage,
            planning_backend,
            photon: PhotonClient::new(PUBLIC_PHOTON_BASE_URL, USER_AGENT)?,
            bundle_path,
            recent_log_lines,
            // Place names come in English until the app has a language setting.
            language: "en".into(),
            ticket_plans: Mutex::new(FxHashMap::default()),
            live_quote_log: Mutex::new(Vec::new()),
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
        let planning_session = TripPlanningSession::with_result_store(
            Arc::clone(&self.planning_backend),
            ChannelEventSink(events),
            StorageLegResultStore(Arc::clone(&self.storage)),
            DEFAULT_PLANNING_DEBOUNCE,
        );
        planning_session.trip_changed(trip.clone());
        self.open_trips().insert(
            trip.id,
            OpenTrip {
                trip,
                planning_session,
                history: TripHistory::default(),
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
        let description = edit.describe(&trip);
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
        let before = std::mem::replace(&mut open_trip.trip, trip.clone());
        open_trip.history.record(before, &trip, description);
        open_trip.planning_session.trip_changed(trip.clone());
        Ok(TripEditOutcome {
            trip,
            notices,
            history: open_trip.history.state(),
        })
    }

    /// Takes back the most recent edit of an open trip, saves the trip and replans it. Returns
    /// `None` when there is nothing to undo.
    ///
    /// # Errors
    ///
    /// Returns an error when the trip is not open or saving fails.
    pub fn undo_edit(&self, trip_id: TripId) -> Result<Option<TripEditOutcome>, CommandError> {
        self.step_through_history(trip_id, TripHistory::undo)
    }

    /// Applies the most recently undone edit of an open trip again, saves the trip and replans
    /// it. Returns `None` when there is nothing to redo.
    ///
    /// # Errors
    ///
    /// Returns an error when the trip is not open or saving fails.
    pub fn redo_edit(&self, trip_id: TripId) -> Result<Option<TripEditOutcome>, CommandError> {
        self.step_through_history(trip_id, TripHistory::redo)
    }

    /// Moves an open trip one step through its history with `step`, and saves and replans the
    /// trip it arrives at. The planning session keeps earlier results, so a step back shows the
    /// earlier plan without a search.
    fn step_through_history(
        &self,
        trip_id: TripId,
        step: impl FnOnce(&mut TripHistory, Trip) -> Option<Trip>,
    ) -> Result<Option<TripEditOutcome>, CommandError> {
        let mut open_trips = self.open_trips();
        let open_trip = open_trips
            .get_mut(&trip_id)
            .ok_or(CommandError::TripNotOpen(trip_id))?;
        let Some(trip) = step(&mut open_trip.history, open_trip.trip.clone()) else {
            return Ok(None);
        };
        self.storage.save_trip(&trip)?;
        open_trip.trip = trip.clone();
        open_trip.planning_session.trip_changed(trip.clone());
        Ok(Some(TripEditOutcome {
            trip,
            notices: TripEditNotices::default(),
            history: open_trip.history.state(),
        }))
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
