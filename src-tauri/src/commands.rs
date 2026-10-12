//! Commands the frontend calls. `tauri-specta` generates their TypeScript wrappers.

// Tauri passes `State` and other arguments by value, which the command signatures must match.
#![allow(clippy::needless_pass_by_value)]

use btt_model::edit::NewTripPlace;
use btt_model::{
    EuroCents, EuroRange, GeoCoordinates, LiveQuote, LiveQuoteRequest, PassScenario,
    PassScenarioLeg, PlaceInsertionPoint, PlaceSearchResult, PlanningSessionEvent,
    StationSearchRadius, StrategyKind, TicketLeg, TicketPlan, TicketPreferences, Traveller,
    TravellerId, Trip, TripEdit, TripEditError, TripEditOutcome, TripId, TripSummary,
};
use chrono::NaiveDate;
use serde::Serialize;
use smol_str::SmolStr;
use specta::Type;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use thiserror::Error;

use crate::place_naming::{name_provisional_places, provisional_place_name};
use crate::state::AppState;

/// Errors a command returns to the frontend.
#[derive(Debug, Error, Serialize, Type)]
pub enum CommandError {
    /// No trip with this ID exists.
    #[error("trip {0:?} does not exist")]
    TripNotFound(TripId),
    /// The frontend edited a trip it has not opened.
    #[error("trip {0:?} is not open")]
    TripNotOpen(TripId),
    /// The edit does not fit the trip.
    #[error("invalid edit: {0}")]
    InvalidEdit(TripEditError),
    /// Reading or writing the database failed.
    #[error("storage failed: {message}")]
    Storage {
        /// Description of the failure.
        message: String,
    },
    /// The place search service failed or could not be reached.
    #[error("place search failed: {message}")]
    PlaceSearch {
        /// Description of the failure.
        message: String,
    },
}

impl From<btt_storage::StorageError> for CommandError {
    fn from(error: btt_storage::StorageError) -> Self {
        Self::Storage {
            message: error.to_string(),
        }
    }
}

/// Returns a summary of every stored trip, most recently changed first.
#[tauri::command]
#[specta::specta]
pub fn list_trips(state: State<'_, AppState>) -> Result<Vec<TripSummary>, CommandError> {
    Ok(state.storage.list_trips()?)
}

/// Creates and stores an empty trip.
#[tauri::command]
#[specta::specta]
pub fn create_trip(
    state: State<'_, AppState>,
    name: SmolStr,
    start_date: NaiveDate,
) -> Result<Trip, CommandError> {
    let trip = Trip::new(name, start_date);
    state.storage.save_trip(&trip)?;
    Ok(trip)
}

/// Opens a trip for editing and starts planning it. Planning events go to `events`.
///
/// Async, so it runs on the Tokio runtime the planning session spawns its tasks on.
#[tauri::command]
#[specta::specta]
pub async fn open_trip(
    state: State<'_, AppState>,
    trip_id: TripId,
    events: Channel<PlanningSessionEvent>,
) -> Result<Trip, CommandError> {
    let trip = state
        .storage
        .load_trip(trip_id)?
        .ok_or(CommandError::TripNotFound(trip_id))?;
    state.open_trip(trip.clone(), events);
    Ok(trip)
}

/// Stops planning a trip.
#[tauri::command]
#[specta::specta]
pub fn close_trip(state: State<'_, AppState>, trip_id: TripId) {
    state.open_trips().remove(&trip_id);
}

/// Applies an edit to an open trip, saves it and replans the legs it changed.
///
/// Async, so it runs on the Tokio runtime the planning session spawns its tasks on.
#[tauri::command]
#[specta::specta]
pub async fn apply_trip_edit(
    app: AppHandle,
    state: State<'_, AppState>,
    trip_id: TripId,
    edit: TripEdit,
) -> Result<TripEditOutcome, CommandError> {
    let outcome = state.apply_edit_from_frontend(trip_id, edit)?;
    name_provisional_places(&app, &outcome.trip);
    Ok(outcome)
}

/// Takes back the most recent edit of an open trip. Returns `None` when there is nothing to
/// undo.
///
/// Async, so it runs on the Tokio runtime the planning session spawns its tasks on.
#[tauri::command]
#[specta::specta]
pub async fn undo_trip_edit(
    app: AppHandle,
    state: State<'_, AppState>,
    trip_id: TripId,
) -> Result<Option<TripEditOutcome>, CommandError> {
    let outcome = state.undo_edit(trip_id)?;
    // A place restored with its provisional name gets a name from reverse geocoding again.
    if let Some(outcome) = &outcome {
        name_provisional_places(&app, &outcome.trip);
    }
    Ok(outcome)
}

/// Applies the most recently undone edit of an open trip again. Returns `None` when there is
/// nothing to redo.
///
/// Async, so it runs on the Tokio runtime the planning session spawns its tasks on.
#[tauri::command]
#[specta::specta]
pub async fn redo_trip_edit(
    app: AppHandle,
    state: State<'_, AppState>,
    trip_id: TripId,
) -> Result<Option<TripEditOutcome>, CommandError> {
    let outcome = state.redo_edit(trip_id)?;
    if let Some(outcome) = &outcome {
        name_provisional_places(&app, &outcome.trip);
    }
    Ok(outcome)
}

/// Adds a place where the user clicked on the map, at the position in the itinerary that adds
/// the least distance. The place gets a name from its coordinates until reverse geocoding finds
/// the town.
///
/// Async, so it runs on the Tokio runtime the planning session spawns its tasks on.
#[tauri::command]
#[specta::specta]
pub async fn add_place_at_coordinates(
    app: AppHandle,
    state: State<'_, AppState>,
    trip_id: TripId,
    coordinates: GeoCoordinates,
) -> Result<TripEditOutcome, CommandError> {
    let edit = TripEdit::AddPlace {
        place: NewTripPlace {
            name: provisional_place_name(coordinates),
            coordinates,
            station_search_radius: StationSearchRadius::DEFAULT,
        },
        insert_at: PlaceInsertionPoint::LeastAddedDistance,
    };
    let outcome = state.apply_edit_from_frontend(trip_id, edit)?;
    name_provisional_places(&app, &outcome.trip);
    Ok(outcome)
}

/// Deletes a trip and stops planning it.
#[tauri::command]
#[specta::specta]
pub fn delete_trip(state: State<'_, AppState>, trip_id: TripId) -> Result<(), CommandError> {
    state.open_trips().remove(&trip_id);
    state.storage.delete_trip(trip_id)?;
    Ok(())
}

/// Returns every traveller profile.
#[tauri::command]
#[specta::specta]
pub fn list_travellers(state: State<'_, AppState>) -> Result<Vec<Traveller>, CommandError> {
    Ok(state.storage.list_travellers()?)
}

/// Inserts a traveller profile, or replaces the one with the same ID.
#[tauri::command]
#[specta::specta]
pub fn save_traveller(
    state: State<'_, AppState>,
    traveller: Traveller,
) -> Result<(), CommandError> {
    state.storage.save_traveller(&traveller)?;
    Ok(())
}

/// Deletes a traveller profile.
#[tauri::command]
#[specta::specta]
pub fn delete_traveller(
    state: State<'_, AppState>,
    traveller_id: TravellerId,
) -> Result<(), CommandError> {
    state.storage.delete_traveller(traveller_id)?;
    Ok(())
}

/// Returns today's date in UTC, which is close enough to decide whether sales have opened.
pub(crate) fn today_utc() -> NaiveDate {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0));
    chrono::DateTime::from_timestamp(seconds, 0)
        .map_or(NaiveDate::MIN, |moment| moment.date_naive())
}

/// Finds the cheapest tickets for the journeys of a trip's legs, for the first traveller of the
/// trip.
#[tauri::command]
#[specta::specta]
pub fn plan_tickets(
    state: State<'_, AppState>,
    trip_id: TripId,
    legs: Vec<TicketLeg>,
) -> Result<TicketPlan, CommandError> {
    let preferences = state
        .open_trips()
        .get(&trip_id)
        .map(|open_trip| open_trip.trip.preferences.clone())
        .ok_or(CommandError::TripNotOpen(trip_id))?;
    let travellers = state.storage.list_travellers()?;
    let traveller = preferences
        .traveller_ids
        .iter()
        .find_map(|id| travellers.iter().find(|traveller| traveller.id == *id));
    let rides = legs
        .iter()
        .flat_map(|leg| {
            btt_ticketing::rides_of_journey(leg.arrival_place_id, &leg.journey, &leg.stations)
        })
        .collect();
    let plan = btt_ticketing::plan_tickets(
        &btt_ticketing::TicketRequest {
            rides,
            traveller,
            preferences: &preferences.ticket_preferences,
            today: today_utc(),
        },
        &state.fare_data,
        &state.price_book,
    );
    state
        .ticket_plans
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            trip_id,
            crate::ticket_report::TicketPlanRecord {
                created_at: chrono::Local::now().to_rfc3339(),
                legs,
                traveller: traveller.cloned(),
                preferences: preferences.ticket_preferences.clone(),
                plan: plan.clone(),
            },
        );
    Ok(plan)
}

/// Chooses how to cover a trip's legs: single tickets everywhere, and the best one or two
/// passes with the legs that ride on their travel days. The legs come in travel order, and
/// the scenarios come sorted, the lowest weighted total first. The first traveller of the trip
/// prices the passes.
#[tauri::command]
#[specta::specta]
pub fn plan_pass_scenarios(
    state: State<'_, AppState>,
    trip_id: TripId,
    legs: Vec<PassScenarioLeg>,
) -> Result<Vec<PassScenario>, CommandError> {
    let preferences = state
        .open_trips()
        .get(&trip_id)
        .map(|open_trip| open_trip.trip.preferences.clone())
        .ok_or(CommandError::TripNotOpen(trip_id))?;
    let travellers = state.storage.list_travellers()?;
    let traveller = preferences
        .traveller_ids
        .iter()
        .find_map(|id| travellers.iter().find(|traveller| traveller.id == *id));
    Ok(btt_ticketing::pass_scenarios(
        &legs,
        &state.fare_data.passes,
        traveller,
        preferences.price_weight,
    ))
}

/// Asks the booking site named in a ticket's booking link for the live price of the ticket.
///
/// Returns `LiveQuote::Unavailable` when the site shows the trip and sells no ticket for it.
/// Returns `None` when the site is not one the app reads, a failure of the site leaves no price,
/// or the site has no trip at the time. The site's answers are kept for half an hour, and
/// requests are spaced out and paused after a failure. Every lookup goes into a log that
/// problem reports include.
#[tauri::command]
#[specta::specta]
pub async fn fetch_live_quote(
    state: State<'_, AppState>,
    request: LiveQuoteRequest,
) -> Result<Option<LiveQuote>, CommandError> {
    Ok(quote_ticket(&state, &request).await)
}

/// Looks up the live price of a ticket the way `fetch_live_quote` describes, teaches the price
/// model from it and logs the lookup.
async fn quote_ticket(state: &AppState, request: &LiveQuoteRequest) -> Option<LiveQuote> {
    let (outcome, site_answer) = crate::quote_refresh::with_refresh(
        request.refresh,
        crate::ticket_report::look_up(state, request),
    )
    .await;
    let model_estimate = crate::price_learning::learn_from_lookup(state, request, &outcome);
    let quote = match &outcome {
        crate::ticket_report::LiveQuoteOutcome::Quoted { price } => {
            Some(LiveQuote::Priced(price.clone()))
        }
        crate::ticket_report::LiveQuoteOutcome::NotSold => Some(LiveQuote::Unavailable),
        _ => None,
    };
    let mut log = state
        .live_quote_log
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    crate::ticket_report::log_lookup(&mut log, request, outcome, site_answer, model_estimate);
    quote
}

/// Returns the price of a journey's single tickets with the live prices of the booking sites in
/// place of the estimates, for the planner to rank the journey by. The tickets are the ones
/// the planner's estimate holds: one adult in second class on the cheapest single tickets and
/// the night accommodation. Returns `None` when the trip turns live prices off or no site gave a
/// price, in which case the estimate stands.
///
/// Every ticket is one lookup, spaced out and paused like those of `fetch_live_quote`, so this
/// takes seconds.
#[tauri::command]
#[specta::specta]
pub async fn live_journey_price(
    state: State<'_, AppState>,
    trip_id: TripId,
    leg: TicketLeg,
) -> Result<Option<EuroRange>, CommandError> {
    let live_prices = state
        .open_trips()
        .get(&trip_id)
        .map(|open_trip| open_trip.trip.preferences.ticket_preferences.live_prices)
        .ok_or(CommandError::TripNotOpen(trip_id))?;
    if !live_prices {
        return Ok(None);
    }
    let preferences = TicketPreferences::default();
    let plan = btt_ticketing::plan_tickets(
        &btt_ticketing::TicketRequest {
            rides: btt_ticketing::rides_of_journey(
                leg.arrival_place_id,
                &leg.journey,
                &leg.stations,
            ),
            traveller: None,
            preferences: &preferences,
            today: today_utc(),
        },
        &state.fare_data,
        &state.price_book,
    );
    let Some(strategy) = plan
        .strategies
        .iter()
        .find(|strategy| strategy.kind == StrategyKind::SingleTickets)
    else {
        return Ok(None);
    };
    let (mut low, mut middle, mut high) = (0_u64, 0_u64, 0_u64);
    let mut any_live = false;
    for item in &strategy.items {
        let mut price = item.price.clone();
        if let Some(request) = &item.live_quote_request
            && let Some(LiveQuote::Priced(live)) = quote_ticket(&state, request).await
        {
            price = live;
            any_live = true;
        }
        low += u64::from(price.low.0);
        middle += u64::from(price.middle().0);
        high += u64::from(price.high.0);
    }
    if !any_live {
        return Ok(None);
    }
    let to_cents = |cents: u64| EuroCents(u32::try_from(cents).unwrap_or(u32::MAX));
    Ok(EuroRange::new(to_cents(low), to_cents(middle), to_cents(high)).ok())
}

/// Searches settlements and railway stations by name. Answers repeated searches from the
/// cache.
#[tauri::command]
#[specta::specta]
pub async fn search_places(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<PlaceSearchResult>, CommandError> {
    let normalized_query = query.trim().to_lowercase();
    if normalized_query.is_empty() {
        return Ok(Vec::new());
    }
    let request_key = format!("search:{}:{normalized_query}", state.language);
    if let Some(cached) = state.storage.cached_place_search(&request_key)? {
        return Ok(cached);
    }
    let results = state
        .photon
        .search(&normalized_query, &state.language)
        .await
        .map_err(|error| CommandError::PlaceSearch {
            message: error.to_string(),
        })?
        .into_vec();
    state.storage.store_place_search(&request_key, &results)?;
    Ok(results)
}
