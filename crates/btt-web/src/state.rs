//! The state of the backend and the commands that read and change it.

use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex, PoisonError};

use btt_backend::editing::{PreparedEdit, prepare_edit};
use btt_backend::place_naming::{has_provisional_name, provisional_place_name};
use btt_backend::pricing::PriceBookJourneyPricer;
use btt_backend::sharing::{LinkOptions, share_snapshot, trip_file_json, trip_file_name};
use btt_backend::tickets::{first_traveller, plan_trip_pass_scenarios, plan_trip_tickets};
use btt_backend::timetable_status::{
    InstalledBundle, OperatorEndDate, StatusInputs, build_status, installed_bundle_of,
};
use btt_fares::{FareData, PriceBook};
use btt_geo::TimeZoneFinder;
use btt_model::edit::NewTripPlace;
use btt_model::{
    CommandError, GeoCoordinates, LegPlanningResult, PassScenarioLeg, PlaceInsertionPoint,
    PlanningSessionEvent, SnapshotLeg, StationSearchRadius, TicketLeg, Traveller, TravellerId,
    Trip, TripEdit, TripEditNotices, TripEditOutcome, TripHistory, TripId, TripPlaceId,
    TripSnapshot,
};
use btt_planner::{
    DEFAULT_PLANNING_DEBOUNCE, LegCacheKey, LegPlanningBackend, LegResultStore, PlanningEventSink,
    TimetableBundleRouter, TripPlanningSession,
};
use btt_timetable::LoadedTimetableBundle;
use btt_timetable::manifest::TimetableManifest;
use chrono::NaiveDate;
use futures_channel::mpsc::UnboundedSender;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use smol_str::SmolStr;

use crate::host::{Host, Outgoing};
use crate::storage::WebStorage;
use crate::{Arguments, Failure};

/// Prefix of the text that stands for a channel in the arguments of a command.
const CHANNEL_PREFIX: &str = "__CHANNEL__:";

/// Version of the app that asks for timetables, which decides what bundles it can read.
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Sends the planner's events to a channel of the page.
struct ChannelSink {
    /// The channel the page created for `open_trip`.
    channel_id: u32,
    /// The queue to the page.
    outbox: UnboundedSender<Outgoing>,
}

impl PlanningEventSink for ChannelSink {
    fn send_event(&self, event: PlanningSessionEvent) {
        if let Ok(json) = serde_json::to_string(&event) {
            // The queue only closes when the module shuts down, and then nobody needs the event.
            let _ = self.outbox.unbounded_send(Outgoing::Channel {
                channel_id: self.channel_id,
                json,
            });
        }
    }
}

/// A leg that the page read back from the browser's storage.
#[derive(Deserialize)]
pub(crate) struct StoredLeg {
    /// The cache key, in decimal.
    pub(crate) key: String,
    /// The planned leg as JSON.
    pub(crate) result: String,
}

/// Keeps planned legs in memory, and hands each new one to the page so it reaches the browser's
/// storage. The page reads the stored legs back in when the module starts.
#[derive(Clone)]
struct WebLegResultStore {
    /// The planned legs of the timetable in use.
    results: Arc<Mutex<FxHashMap<LegCacheKey, LegPlanningResult>>>,
    /// The queue to the page.
    outbox: UnboundedSender<Outgoing>,
}

impl WebLegResultStore {
    /// Creates a store with no legs that sends new legs to the page through a queue.
    fn new(outbox: UnboundedSender<Outgoing>) -> Self {
        Self {
            results: Arc::default(),
            outbox,
        }
    }

    /// Keeps a leg that the page stored earlier, without sending it back to the page.
    fn insert_stored(&self, key: LegCacheKey, result: LegPlanningResult) {
        self.results
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, result);
    }
}

impl LegResultStore for WebLegResultStore {
    fn load(&self, key: LegCacheKey) -> Option<LegPlanningResult> {
        self.results
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned()
    }

    fn store(&self, key: LegCacheKey, namespace: u64, result: &LegPlanningResult) {
        {
            let mut results = self.results.lock().unwrap_or_else(PoisonError::into_inner);
            // The cache key includes the bundle version, so results of an older bundle never
            // match. The limit only keeps memory in check.
            if results.len() >= 512 {
                results.clear();
            }
            results.insert(key, result.clone());
        }
        if let Ok(json) = serde_json::to_string(result) {
            // The queue only closes when the module shuts down, and then nobody needs the leg.
            let _ = self.outbox.unbounded_send(Outgoing::Leg {
                namespace: namespace.to_string(),
                key: key.0.to_string(),
                json,
            });
        }
    }
}

/// A trip the page has open, with the session that plans it.
struct OpenTrip {
    /// The current state of the trip, which every edit starts from.
    trip: Trip,
    /// Plans the trip and sends results to the page.
    planning_session: TripPlanningSession<ChannelSink, WebLegResultStore, PriceBookJourneyPricer>,
    /// Earlier and undone states of the trip.
    history: TripHistory,
}

/// The timetable the planner uses.
#[derive(Default)]
struct ActiveTimetable {
    /// Plans single legs. `None` until a bundle is installed.
    backend: Option<Arc<LegPlanningBackend<PriceBookJourneyPricer>>>,
    /// Facts about the bundle.
    installed: Option<InstalledBundle>,
}

/// What the page last learned about the timetables on the server.
#[derive(Default)]
struct ServerTimetables {
    /// The manifest, once a check succeeded.
    manifest: Option<TimetableManifest>,
    /// Why the last check failed.
    check_error: Option<String>,
}

/// Everything the commands share.
pub(crate) struct WebBackend {
    /// What the page and the module pass to each other.
    host: Host,
    /// The trips and travellers.
    storage: RefCell<WebStorage>,
    /// The pass list, reservation rules and price model.
    fare_data: Arc<FareData>,
    /// The sources that price single tickets.
    price_book: Arc<PriceBook>,
    /// Finds the time zone of places.
    time_zone_finder: Arc<TimeZoneFinder>,
    /// The timetable the planner uses.
    timetable: RefCell<ActiveTimetable>,
    /// The timetables on the server.
    server_timetables: RefCell<ServerTimetables>,
    /// Planned legs, shared by the sessions.
    leg_results: WebLegResultStore,
    /// The trips the page has open.
    open_trips: RefCell<FxHashMap<TripId, OpenTrip>>,
    /// Whether the state changed since the page last saved it.
    state_changed: Cell<bool>,
}

/// Returns today's date in UTC.
fn today_utc() -> NaiveDate {
    let milliseconds = js_sys::Date::now();
    // The clock of a browser is a finite, positive number of milliseconds.
    #[allow(clippy::cast_possible_truncation)]
    let seconds = (milliseconds / 1000.0) as i64;
    chrono::DateTime::from_timestamp(seconds, 0)
        .map_or(NaiveDate::MIN, |moment| moment.date_naive())
}

/// Returns the time in milliseconds since the Unix epoch.
fn now_milliseconds() -> u64 {
    // The clock of a browser is a finite, positive number of milliseconds.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        js_sys::Date::now() as u64
    }
}

/// Serializes the result of a command.
fn to_value(result: &impl Serialize) -> Result<Value, Failure> {
    serde_json::to_value(result).map_err(|error| Failure::Other(error.to_string()))
}

impl WebBackend {
    /// Creates the backend around the stored state of an earlier visit.
    pub(crate) fn new(host: Host, stored_state: Option<&str>) -> Result<Self, String> {
        let storage = match stored_state {
            Some(json) => WebStorage::from_json(json)?,
            None => WebStorage::default(),
        };
        // The data files are compiled in and a test in `btt-fares` loads them, so this cannot
        // fail.
        let fare_data = Arc::new(FareData::bundled().map_err(|error| error.to_string())?);
        let price_book = Arc::new(PriceBook::from_data(&fare_data));
        let leg_results = WebLegResultStore::new(host.outbox());
        Ok(Self {
            host,
            storage: RefCell::new(storage),
            fare_data,
            price_book,
            time_zone_finder: Arc::new(TimeZoneFinder::new()),
            timetable: RefCell::new(ActiveTimetable::default()),
            server_timetables: RefCell::new(ServerTimetables::default()),
            leg_results,
            open_trips: RefCell::new(FxHashMap::default()),
            state_changed: Cell::new(false),
        })
    }

    /// Loads a bundle and makes the planner use it. Trips that are open keep the bundle they
    /// opened with.
    pub(crate) fn install_bundle(&self, bytes: &[u8]) -> Result<(), String> {
        let loaded = LoadedTimetableBundle::from_bytes(bytes).map_err(|error| error.to_string())?;
        let pricer =
            PriceBookJourneyPricer::new(Arc::clone(&self.fare_data), Arc::clone(&self.price_book));
        let router =
            TimetableBundleRouter::from_loaded(loaded, Arc::clone(&self.time_zone_finder), pricer);
        let installed = installed_bundle_of(&router);
        *self.timetable.borrow_mut() = ActiveTimetable {
            backend: Some(Arc::new(LegPlanningBackend::TimetableBundle(Arc::new(
                router,
            )))),
            installed: Some(installed),
        };
        Ok(())
    }

    /// Records what the page learned from the server.
    pub(crate) fn record_manifest(
        &self,
        manifest_json: Option<&str>,
        check_error: Option<String>,
    ) -> Result<(), String> {
        let mut server = self.server_timetables.borrow_mut();
        if let Some(json) = manifest_json {
            server.manifest = Some(
                serde_json::from_str(json)
                    .map_err(|error| format!("the manifest is wrong: {error}"))?,
            );
        }
        server.check_error = check_error;
        Ok(())
    }

    /// Returns the namespace of the timetable in use, which keys the stored legs.
    pub(crate) fn leg_cache_namespace(&self) -> Result<String, String> {
        self.timetable
            .borrow()
            .backend
            .as_ref()
            .map(|backend| backend.cache_namespace().to_string())
            .ok_or_else(|| "the backend has no timetable".to_owned())
    }

    /// Keeps the planned legs that the page read back from the browser's storage. An entry that
    /// does not read back is skipped, because the planner plans it again when it is needed.
    pub(crate) fn load_leg_results(&self, entries_json: &str) -> Result<(), String> {
        let entries: Vec<StoredLeg> = serde_json::from_str(entries_json)
            .map_err(|error| format!("the stored legs are unreadable: {error}"))?;
        for entry in entries {
            let key = entry.key.parse().map(LegCacheKey);
            let result = serde_json::from_str::<LegPlanningResult>(&entry.result);
            if let (Ok(key), Ok(result)) = (key, result) {
                self.leg_results.insert_stored(key, result);
            } else {
                tracing::warn!("a stored leg does not read back");
            }
        }
        Ok(())
    }

    /// Runs one command.
    pub(crate) fn run(&self, command: &str, arguments: &Arguments) -> Result<Value, Failure> {
        let outcome = self.dispatch(command, arguments);
        if self.state_changed.replace(false) {
            self.host.save_state(&self.storage.borrow().to_json());
        }
        outcome
    }

    /// Finds the code of a command.
    fn dispatch(&self, command: &str, arguments: &Arguments) -> Result<Value, Failure> {
        match command {
            "list_trips" => to_value(&self.storage.borrow().list_trips()),
            "create_trip" => self.create_trip(arguments.get("name")?, arguments.get("startDate")?),
            "open_trip" => {
                self.open_trip(arguments.get("tripId")?, channel_id(arguments, "events")?)
            }
            "close_trip" => {
                self.open_trips
                    .borrow_mut()
                    .remove(&arguments.get::<TripId>("tripId")?);
                Ok(Value::Null)
            }
            "apply_trip_edit" => {
                to_value(&self.apply_edit(arguments.get("tripId")?, arguments.get("edit")?)?)
            }
            "undo_trip_edit" => {
                to_value(&self.step_through_history(arguments.get("tripId")?, TripHistory::undo)?)
            }
            "redo_trip_edit" => {
                to_value(&self.step_through_history(arguments.get("tripId")?, TripHistory::redo)?)
            }
            "add_place_at_coordinates" => {
                let coordinates: GeoCoordinates = arguments.get("coordinates")?;
                let edit = TripEdit::AddPlace {
                    place: NewTripPlace {
                        name: provisional_place_name(coordinates),
                        coordinates,
                        station_search_radius: StationSearchRadius::DEFAULT,
                    },
                    insert_at: PlaceInsertionPoint::LeastAddedDistance,
                };
                to_value(&self.apply_edit(arguments.get("tripId")?, edit)?)
            }
            "web_name_place" => self.name_place(
                arguments.get("tripId")?,
                arguments.get("placeId")?,
                arguments.get("coordinates")?,
                arguments.get("name")?,
            ),
            "delete_trip" => {
                let trip_id: TripId = arguments.get("tripId")?;
                self.open_trips.borrow_mut().remove(&trip_id);
                self.storage.borrow_mut().delete_trip(trip_id);
                self.state_changed.set(true);
                Ok(Value::Null)
            }
            "list_travellers" => to_value(&self.storage.borrow().travellers()),
            "save_traveller" => {
                self.storage
                    .borrow_mut()
                    .save_traveller(&arguments.get::<Traveller>("traveller")?);
                self.state_changed.set(true);
                Ok(Value::Null)
            }
            "delete_traveller" => {
                self.storage
                    .borrow_mut()
                    .delete_traveller(arguments.get::<TravellerId>("travellerId")?);
                self.state_changed.set(true);
                Ok(Value::Null)
            }
            "plan_tickets" => self.plan_tickets(
                arguments.get("tripId")?,
                &arguments.get::<Vec<TicketLeg>>("legs")?,
            ),
            "plan_pass_scenarios" => self.plan_pass_scenarios(
                arguments.get("tripId")?,
                &arguments.get::<Vec<PassScenarioLeg>>("legs")?,
            ),
            // Booking sites do not allow requests from a page, so the browser has no live prices.
            "live_journey_price" | "fetch_live_quote" | "export_problem_report" => Ok(Value::Null),
            "share_trip" => self.share_trip(arguments),
            "export_trip_file" => self.export_trip_file(
                arguments.get("tripId")?,
                &arguments.get::<Vec<SnapshotLeg>>("legs")?,
            ),
            "timetable_status" => Ok(to_value(&self.timetable_status())?),
            "operator_end_dates" => Ok(to_value(&self.operator_end_dates())?),
            "timetable_feeds" => Ok(to_value(&self.timetable_feeds())?),
            "set_timetable_checks" => {
                self.storage.borrow_mut().timetable_checks_enabled = arguments.get("enabled")?;
                self.state_changed.set(true);
                Ok(Value::Null)
            }
            other => Err(Failure::Other(format!(
                "the command `{other}` does not exist"
            ))),
        }
    }

    /// Returns the backend that plans legs.
    fn planning_backend(
        &self,
    ) -> Result<Arc<LegPlanningBackend<PriceBookJourneyPricer>>, CommandError> {
        self.timetable
            .borrow()
            .backend
            .clone()
            .ok_or_else(|| CommandError::Timetable {
                message: "the app has no timetable, download one first".to_owned(),
            })
    }

    /// Checks that a start date lies in the days the timetable covers.
    fn check_start_date(&self, start_date: NaiveDate) -> Result<(), CommandError> {
        let window = self
            .timetable
            .borrow()
            .backend
            .as_ref()
            .and_then(|backend| backend.validity_window());
        match window {
            Some((first, last)) if start_date < first || start_date > last => {
                Err(CommandError::StartDateOutsideTimetable { first, last })
            }
            _ => Ok(()),
        }
    }

    /// Creates and stores an empty trip.
    fn create_trip(&self, name: SmolStr, start_date: NaiveDate) -> Result<Value, Failure> {
        self.check_start_date(start_date)?;
        let trip = Trip::new(name, start_date);
        self.save_trip(&trip);
        to_value(&trip)
    }

    /// Stores a trip.
    fn save_trip(&self, trip: &Trip) {
        self.storage
            .borrow_mut()
            .save_trip(trip, now_milliseconds());
        self.state_changed.set(true);
    }

    /// Opens a trip for editing and starts planning it.
    fn open_trip(&self, trip_id: TripId, channel_id: u32) -> Result<Value, Failure> {
        let trip = self
            .storage
            .borrow()
            .load_trip(trip_id)
            .ok_or(CommandError::TripNotFound(trip_id))?;
        let planning_session = TripPlanningSession::with_result_store(
            self.planning_backend()?,
            ChannelSink {
                channel_id,
                outbox: self.host.outbox(),
            },
            self.leg_results.clone(),
            DEFAULT_PLANNING_DEBOUNCE,
        );
        planning_session.trip_changed(trip.clone());
        self.open_trips.borrow_mut().insert(
            trip_id,
            OpenTrip {
                trip: trip.clone(),
                planning_session,
                history: TripHistory::default(),
            },
        );
        to_value(&trip)
    }

    /// Applies an edit from the page to an open trip, saves it and replans.
    fn apply_edit(&self, trip_id: TripId, edit: TripEdit) -> Result<TripEditOutcome, CommandError> {
        if let TripEdit::SetStartDate(start_date) = &edit {
            self.check_start_date(*start_date)?;
        }
        let mut open_trips = self.open_trips.borrow_mut();
        let open_trip = open_trips
            .get_mut(&trip_id)
            .ok_or(CommandError::TripNotOpen(trip_id))?;
        let PreparedEdit {
            trip,
            notices,
            description,
        } = prepare_edit(&open_trip.trip, edit)?;
        self.save_trip(&trip);
        let before = std::mem::replace(&mut open_trip.trip, trip.clone());
        open_trip.history.record(before, &trip, description);
        open_trip.planning_session.trip_changed(trip.clone());
        Ok(TripEditOutcome {
            trip,
            notices,
            history: open_trip.history.state(),
        })
    }

    /// Moves an open trip one step through its history, and saves and replans the trip it
    /// arrives at.
    fn step_through_history(
        &self,
        trip_id: TripId,
        step: impl FnOnce(&mut TripHistory, Trip) -> Option<Trip>,
    ) -> Result<Option<TripEditOutcome>, CommandError> {
        let mut open_trips = self.open_trips.borrow_mut();
        let open_trip = open_trips
            .get_mut(&trip_id)
            .ok_or(CommandError::TripNotOpen(trip_id))?;
        let Some(trip) = step(&mut open_trip.history, open_trip.trip.clone()) else {
            return Ok(None);
        };
        self.save_trip(&trip);
        open_trip.trip = trip.clone();
        open_trip.planning_session.trip_changed(trip.clone());
        Ok(Some(TripEditOutcome {
            trip,
            notices: TripEditNotices::default(),
            history: open_trip.history.state(),
        }))
    }

    /// Renames a place after the page looked up the town at its position. Does nothing when the
    /// traveller renamed or moved the place meanwhile.
    fn name_place(
        &self,
        trip_id: TripId,
        place_id: TripPlaceId,
        coordinates: GeoCoordinates,
        name: SmolStr,
    ) -> Result<Value, Failure> {
        let mut open_trips = self.open_trips.borrow_mut();
        let Some(open_trip) = open_trips.get_mut(&trip_id) else {
            return Ok(Value::Null);
        };
        let still_provisional = open_trip
            .trip
            .itinerary
            .place(place_id)
            .is_some_and(|place| place.coordinates == coordinates && has_provisional_name(place));
        if !still_provisional {
            return Ok(Value::Null);
        }
        let mut trip = open_trip.trip.clone();
        trip.apply_edit(TripEdit::RenamePlace { place_id, name })
            .map_err(CommandError::InvalidEdit)?;
        self.save_trip(&trip);
        open_trip.trip = trip.clone();
        open_trip.planning_session.trip_changed_elsewhere(trip);
        Ok(Value::Null)
    }

    /// Returns the preferences of an open trip and the traveller its tickets are priced for.
    fn pricing_context(
        &self,
        trip_id: TripId,
    ) -> Result<(btt_model::TripPreferences, Option<Traveller>), CommandError> {
        let preferences = self
            .open_trips
            .borrow()
            .get(&trip_id)
            .map(|open_trip| open_trip.trip.preferences.clone())
            .ok_or(CommandError::TripNotOpen(trip_id))?;
        let storage = self.storage.borrow();
        let traveller = first_traveller(&preferences, storage.travellers()).cloned();
        Ok((preferences, traveller))
    }

    /// Finds the cheapest tickets for the journeys of a trip's legs.
    fn plan_tickets(&self, trip_id: TripId, legs: &[TicketLeg]) -> Result<Value, Failure> {
        let (preferences, traveller) = self.pricing_context(trip_id)?;
        to_value(&plan_trip_tickets(
            legs,
            &preferences,
            traveller.as_ref(),
            today_utc(),
            &self.fare_data,
            &self.price_book,
        ))
    }

    /// Chooses how to cover a trip's legs with single tickets and passes.
    fn plan_pass_scenarios(
        &self,
        trip_id: TripId,
        legs: &[PassScenarioLeg],
    ) -> Result<Value, Failure> {
        let (preferences, traveller) = self.pricing_context(trip_id)?;
        to_value(&plan_trip_pass_scenarios(
            legs,
            &preferences,
            traveller.as_ref(),
            &self.fare_data,
        ))
    }

    /// Builds the snapshot of an open trip from the legs as the page shows them.
    fn snapshot_of_open_trip(
        &self,
        trip_id: TripId,
        legs: &[SnapshotLeg],
    ) -> Result<TripSnapshot, CommandError> {
        let trip = self
            .open_trips
            .borrow()
            .get(&trip_id)
            .map(|open_trip| open_trip.trip.clone())
            .ok_or(CommandError::TripNotOpen(trip_id))?;
        Ok(TripSnapshot::new(
            &trip,
            legs,
            today_utc(),
            self.planning_backend()?.bundle_version(),
        ))
    }

    /// Writes an open trip as a link for the web viewer.
    fn share_trip(&self, arguments: &Arguments) -> Result<Value, Failure> {
        let snapshot = self.snapshot_of_open_trip(
            arguments.get("tripId")?,
            &arguments.get::<Vec<SnapshotLeg>>("legs")?,
        )?;
        let shared = share_snapshot(
            snapshot,
            LinkOptions {
                include_alternatives: arguments.get("includeAlternatives")?,
                include_prices: arguments.get("includePrices")?,
                name_places_by_station: arguments.get("namePlacesByStation")?,
                compact: arguments.get("compact")?,
            },
        )?;
        to_value(&shared)
    }

    /// Hands the page a trip file to download. Returns the name of the file.
    fn export_trip_file(&self, trip_id: TripId, legs: &[SnapshotLeg]) -> Result<Value, Failure> {
        let snapshot = self.snapshot_of_open_trip(trip_id, legs)?;
        let file_name = trip_file_name(&snapshot.trip.name);
        let json = trip_file_json(snapshot)?;
        self.host.save_file(&file_name, &json);
        Ok(json!(file_name))
    }

    /// Collects what the page shows about the timetable.
    fn timetable_status(&self) -> btt_backend::timetable_status::TimetableStatus {
        let server = self.server_timetables.borrow();
        build_status(
            self.timetable.borrow().installed.as_ref(),
            server.manifest.as_ref(),
            StatusInputs {
                today: today_utc(),
                app_version: APP_VERSION,
                checks_enabled: self.storage.borrow().timetable_checks_enabled,
                check_error: server.check_error.as_deref(),
            },
        )
    }

    /// Returns the listed operators whose trains stop before the timetable does.
    fn operator_end_dates(&self) -> Vec<OperatorEndDate> {
        self.timetable
            .borrow()
            .backend
            .as_ref()
            .map(|backend| {
                backend
                    .operator_ends()
                    .iter()
                    .map(|end| OperatorEndDate {
                        name: end.name.to_string(),
                        country: end.country.to_string(),
                        last_departure_date: end.last_departure_date,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns the feeds of the timetable, ordered by name.
    fn timetable_feeds(&self) -> Vec<btt_backend::timetable_status::FeedCredit> {
        let mut feeds: Vec<_> = self
            .timetable
            .borrow()
            .backend
            .as_ref()
            .map(|backend| {
                backend
                    .feed_attributions()
                    .into_iter()
                    .map(
                        |(name, license, url)| btt_backend::timetable_status::FeedCredit {
                            name: name.to_owned(),
                            license: license.to_owned(),
                            url: url.to_owned(),
                        },
                    )
                    .collect()
            })
            .unwrap_or_default();
        feeds.sort_by(|left, right| left.name.cmp(&right.name));
        feeds
    }
}

/// Reads the number of the channel that an argument stands for, such as `__CHANNEL__:3`.
fn channel_id(arguments: &Arguments, name: &str) -> Result<u32, Failure> {
    let text: String = arguments.get(name)?;
    text.strip_prefix(CHANNEL_PREFIX)
        .and_then(|number| number.parse().ok())
        .ok_or_else(|| Failure::Other(format!("the argument `{name}` is not a channel")))
}
