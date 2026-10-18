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
    TimetableBundleRouter, TripPlanningSession,
};
use btt_quotes::QuoteStore;
use btt_quotes::sbb::SbbService;
use btt_storage::Storage;
use btt_timetable::manifest::last_valid_day;
use chrono::NaiveDate;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use tauri::ipc::Channel;

use crate::commands::CommandError;
use crate::log_buffer::RecentLogLines;
use crate::place_naming::{has_provisional_name, provisional_place_name};
use crate::pricing::PriceBookJourneyPricer;
use crate::ticket_report::{LiveQuoteLogEntry, TicketPlanRecord};
use crate::timetable_updates::{
    InstalledBundle, KEPT_BUNDLE_FILES, TimetableUpdates, bundle_file_name, prune_bundles,
};

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
        if crate::quote_refresh::ignores(key) {
            return None;
        }
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
    pub planning_session:
        TripPlanningSession<ChannelEventSink, StorageLegResultStore, PriceBookJourneyPricer>,
    /// Earlier and undone states of the trip, for undo and redo. Kept in memory only, so it
    /// ends when the trip closes.
    pub history: TripHistory,
}

/// The timetable bundle the planner uses, with the backend that plans on it.
#[derive(Default)]
struct ActiveTimetable {
    /// Plans single legs for every planning session. `None` while the app has no timetable
    /// bundle, and then no trip can open.
    backend: Option<Arc<LegPlanningBackend<PriceBookJourneyPricer>>>,
    /// Facts about the bundle, for the timetable status.
    installed: Option<InstalledBundle>,
    /// The file of the bundle.
    path: Option<PathBuf>,
}

/// Everything the commands share.
pub struct AppState {
    /// The app's database, shared with the planning sessions.
    pub storage: Arc<Storage>,
    /// The timetable the planner uses. A download replaces it while the app runs.
    active_timetable: Mutex<ActiveTimetable>,
    /// Finds the time zone of places, shared by every router the app loads.
    time_zone_finder: Arc<TimeZoneFinder>,
    /// Searches places by name and position.
    pub photon: PhotonClient,
    /// The pass list, reservation rules and price model for tickets, shared with the planner's
    /// pricer.
    pub fare_data: Arc<FareData>,
    /// The sources that price single tickets, shared with the planner's pricer.
    pub price_book: Arc<PriceBook>,
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
    /// Looks up prices on the booking site of European Sleeper. `None` when its HTTP client could
    /// not be built.
    pub european_sleeper_service: Option<btt_quotes::european_sleeper::EuropeanSleeperService>,
    /// Looks up prices on the sites that answer a search with their connections. `None` when
    /// an HTTP client could not be built.
    pub site_services: Option<crate::site_quotes::SiteServices>,
    /// Fetches the ECB reference rates. `None` when its HTTP client could not be built, and
    /// then the fixed rates of the fare data convert prices.
    pub exchange_rate_service: Option<btt_quotes::ecb::ExchangeRateService>,
    /// Checks for newer timetable bundles and downloads them.
    pub timetable_updates: TimetableUpdates,
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
    /// Plans with the timetable bundle `find_bundle` returns. Without one, or when it fails to
    /// load, the app starts without a planning backend and asks for a download.
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
        // The fare data comes first, because the planner's pricer shares it.
        // The data files are compiled in and a test in `btt-fares` loads them, so this cannot
        // fail.
        let fare_data = Arc::new(FareData::bundled().expect("the bundled fare data is valid"));
        let price_book = Arc::new(PriceBook::from_data(&fare_data));
        let pricer = PriceBookJourneyPricer::new(Arc::clone(&fare_data), Arc::clone(&price_book));
        let timetable_updates = TimetableUpdates::new(data_directory, USER_AGENT)?;
        // Nothing has mapped a bundle yet, so old files can go. This must run before the bundle
        // loads, because Windows refuses to delete a mapped file.
        if let Err(error) = prune_bundles(timetable_updates.bundles_directory(), KEPT_BUNDLE_FILES)
        {
            tracing::warn!(%error, "could not remove old timetable bundles");
        }
        let OpenedBundle {
            path: bundle_path,
            installed: installed_bundle,
            router: bundle_router,
        } = open_bundle(data_directory, &time_zone_finder, pricer);
        let planning_backend = bundle_router
            .map(|router| Arc::new(LegPlanningBackend::TimetableBundle(Arc::new(router))));
        if let Some(backend) = &planning_backend {
            remove_stale_legs(&storage, backend);
        } else {
            tracing::info!("no timetable bundle, the app asks for a download");
        }
        // The data files are compiled in and a test in `btt-fares` loads them, so this cannot fail.
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
        let european_sleeper_service =
            btt_quotes::european_sleeper::EuropeanSleeperService::new(Arc::clone(&quote_store))
                .map_err(|error| {
                    tracing::warn!(%error, "live European Sleeper prices are not available");
                })
                .ok();
        let site_services = crate::site_quotes::SiteServices::new(&quote_store)
            .map_err(
                |error| tracing::warn!(%error, "live prices of several sites are not available"),
            )
            .ok();
        let exchange_rate_service =
            btt_quotes::ecb::ExchangeRateService::new(Arc::clone(&quote_store))
                .map_err(|error| tracing::warn!(%error, "ECB exchange rates are not available"))
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
            european_sleeper_service,
            site_services,
            exchange_rate_service,
            storage,
            active_timetable: Mutex::new(ActiveTimetable {
                backend: planning_backend,
                installed: installed_bundle,
                path: bundle_path,
            }),
            time_zone_finder,
            photon: PhotonClient::new(PUBLIC_PHOTON_BASE_URL, USER_AGENT)?,
            timetable_updates,
            recent_log_lines,
            // Place names come in English until the app has a language setting.
            language: "en".into(),
            ticket_plans: Mutex::new(FxHashMap::default()),
            live_quote_log: Mutex::new(Vec::new()),
            open_trips: Mutex::new(FxHashMap::default()),
        })
    }

    /// Returns euro for one unit of a currency: from the latest ECB reference rates when the
    /// app has them, else from the fixed rates of the fare data. Returns `None` for a currency
    /// that neither knows.
    pub async fn euro_per(&self, currency: &str) -> Option<f64> {
        let reference_rates = match &self.exchange_rate_service {
            Some(service) => service.rates().await,
            None => None,
        };
        reference_rates
            .and_then(|rates| rates.euro_per(currency))
            .or_else(|| self.fare_data.exchange_rates.euro_per(currency))
    }

    /// Locks the open trips. Every section that holds the lock replaces whole values, so a
    /// poisoned lock is still consistent.
    pub fn open_trips(&self) -> MutexGuard<'_, FxHashMap<TripId, OpenTrip>> {
        self.open_trips
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Returns the backend that plans legs.
    ///
    /// # Errors
    ///
    /// Returns an error when the app has no timetable bundle.
    pub fn planning_backend(
        &self,
    ) -> Result<Arc<LegPlanningBackend<PriceBookJourneyPricer>>, CommandError> {
        self.current_backend()
            .ok_or_else(|| CommandError::Timetable {
                message: "the app has no timetable, download one first".to_owned(),
            })
    }

    /// Locks the active timetable. Every section that holds the lock replaces whole values, so
    /// a poisoned lock is still consistent.
    fn active_timetable(&self) -> MutexGuard<'_, ActiveTimetable> {
        self.active_timetable
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Returns the backend that plans legs, or `None` while the app has no timetable.
    pub fn current_backend(&self) -> Option<Arc<LegPlanningBackend<PriceBookJourneyPricer>>> {
        self.active_timetable().backend.clone()
    }

    /// Returns the facts about the bundle the planner uses, for the timetable status.
    pub fn installed_bundle(&self) -> Option<InstalledBundle> {
        self.active_timetable().installed.clone()
    }

    /// Returns the file of the bundle the planner uses.
    pub fn bundle_path(&self) -> Option<PathBuf> {
        self.active_timetable().path.clone()
    }

    /// Makes the planner use the downloaded bundle of a version, unless it is not newer than the
    /// one in use.
    ///
    /// Trips that are open keep planning on the bundle they opened with, and the trips opened
    /// after this call use the new one. The old bundle stays mapped until the last of those
    /// trips closes, so its file is never deleted here.
    ///
    /// # Errors
    ///
    /// Returns an error when the file does not load.
    pub async fn activate_bundle(&self, version: u32) -> Result<(), CommandError> {
        if self
            .installed_bundle()
            .is_some_and(|installed| installed.version >= version)
        {
            return Ok(());
        }
        let path = self
            .timetable_updates
            .bundles_directory()
            .join(bundle_file_name(version));
        let time_zone_finder = Arc::clone(&self.time_zone_finder);
        let pricer =
            PriceBookJourneyPricer::new(Arc::clone(&self.fare_data), Arc::clone(&self.price_book));
        let path_to_open = path.clone();
        // Building the indexes of a bundle takes seconds, so it stays off the async threads.
        let router = tokio::task::spawn_blocking(move || {
            TimetableBundleRouter::open(&path_to_open, time_zone_finder, pricer)
        })
        .await
        .map_err(|error| CommandError::Timetable {
            message: error.to_string(),
        })?
        .map_err(|error| CommandError::Timetable {
            message: error.to_string(),
        })?;
        tracing::info!(path = %path.display(), version = router.bundle_version(), "planning with a downloaded timetable bundle");
        let installed = installed_bundle_of(&router);
        let backend = Arc::new(LegPlanningBackend::TimetableBundle(Arc::new(router)));
        remove_stale_legs(&self.storage, &backend);
        *self.active_timetable() = ActiveTimetable {
            backend: Some(backend),
            installed: Some(installed),
            path: Some(path),
        };
        Ok(())
    }

    /// Checks that a start date lies in the days the timetable covers.
    ///
    /// # Errors
    ///
    /// Returns an error when the date is before the first or after the last day of the
    /// timetable. An app without a timetable accepts every date.
    pub fn check_start_date(&self, start_date: NaiveDate) -> Result<(), CommandError> {
        match self
            .current_backend()
            .and_then(|backend| backend.validity_window())
        {
            Some((first, last)) if start_date < first || start_date > last => {
                Err(CommandError::StartDateOutsideTimetable { first, last })
            }
            _ => Ok(()),
        }
    }

    /// Starts a planning session for a trip and registers it as open. Replaces an earlier
    /// session for the same trip, for example after the frontend reloaded.
    ///
    /// # Errors
    ///
    /// Returns an error when the app has no timetable bundle.
    pub fn open_trip(
        &self,
        trip: Trip,
        events: Channel<PlanningSessionEvent>,
    ) -> Result<(), CommandError> {
        let planning_session = TripPlanningSession::with_result_store(
            self.planning_backend()?,
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
        Ok(())
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
        if let TripEdit::SetStartDate(start_date) = &edit {
            self.check_start_date(*start_date)?;
        }
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

/// The timetable bundle the app found at startup and what came of loading it.
struct OpenedBundle {
    /// The file `find_bundle` picked, if there is one.
    path: Option<PathBuf>,
    /// Facts about the bundle, when it loaded.
    installed: Option<InstalledBundle>,
    /// The router over the bundle, when it loaded.
    router: Option<TimetableBundleRouter<PriceBookJourneyPricer>>,
}

/// Returns the facts about the bundle a router plans on.
fn installed_bundle_of(router: &TimetableBundleRouter<PriceBookJourneyPricer>) -> InstalledBundle {
    InstalledBundle {
        version: router.bundle_version(),
        valid_from: router.validity_start(),
        valid_until: last_valid_day(router.validity_start(), router.validity_days()),
    }
}

/// Deletes the planned legs of other bundle versions than the backend's, which can never match
/// again.
fn remove_stale_legs(storage: &Storage, backend: &LegPlanningBackend<PriceBookJourneyPricer>) {
    match storage.delete_leg_results_outside(backend.cache_namespace()) {
        Ok(0) => {}
        Ok(deleted) => tracing::info!(deleted, "removed planned legs of an older timetable"),
        Err(error) => tracing::warn!(%error, "could not clean up planned legs"),
    }
}

/// Finds the timetable bundle to plan with and loads it. A bundle that fails to load is logged
/// and treated as missing, so the app starts and offers to download a new one.
fn open_bundle(
    data_directory: &Path,
    time_zone_finder: &Arc<TimeZoneFinder>,
    pricer: PriceBookJourneyPricer,
) -> OpenedBundle {
    let path = find_bundle(data_directory);
    let Some(bundle_path) = &path else {
        return OpenedBundle {
            path,
            installed: None,
            router: None,
        };
    };
    match TimetableBundleRouter::open(bundle_path, Arc::clone(time_zone_finder), pricer) {
        Ok(router) => {
            tracing::info!(path = %bundle_path.display(), version = router.bundle_version(), "planning with a timetable bundle");
            let installed = Some(installed_bundle_of(&router));
            OpenedBundle {
                path,
                installed,
                router: Some(router),
            }
        }
        Err(error) => {
            tracing::warn!(path = %bundle_path.display(), %error, "the timetable bundle failed to load");
            OpenedBundle {
                path,
                installed: None,
                router: None,
            }
        }
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
