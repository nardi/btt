//! Commands the frontend calls. `tauri-specta` generates their TypeScript wrappers.

// Tauri passes `State` and other arguments by value, which the command signatures must match.
#![allow(clippy::needless_pass_by_value)]

use btt_model::edit::NewTripPlace;
use btt_model::{
    EuroCents, GeoCoordinates, LiveQuoteRequest, PlaceInsertionPoint, PlaceSearchResult,
    PlanningSessionEvent, Price, PriceKind, PriceSource, StationSearchRadius, TicketLeg,
    TicketPlan, TravelClass, Traveller, TravellerId, Trip, TripEdit, TripEditError,
    TripEditOutcome, TripId, TripSummary,
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
fn today_utc() -> NaiveDate {
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
    Ok(btt_ticketing::plan_tickets(
        &btt_ticketing::TicketRequest {
            rides,
            traveller,
            preferences: &preferences.ticket_preferences,
            today: today_utc(),
        },
        &state.fare_data,
        &state.price_book,
    ))
}

/// Asks the booking site named in a ticket's booking link for the live price of the ticket.
///
/// Returns `None` when the site is not one the app reads, the preferences or a failure of the
/// site leave no price, or the site has no trip at the time. The site's answers are kept for
/// half an hour, and requests are spaced out and paused after a failure.
#[tauri::command]
#[specta::specta]
pub async fn fetch_live_quote(
    state: State<'_, AppState>,
    request: LiveQuoteRequest,
) -> Result<Option<Price>, CommandError> {
    let Some(service) = &state.sbb_service else {
        return Ok(None);
    };
    let Ok(search) = btt_quotes::sbb::Search::from_link(&request.booking_link) else {
        return Ok(None);
    };
    let quotes = match service.quotes(&search).await {
        Ok(quotes) => quotes,
        Err(error) => {
            tracing::info!(%error, "no live price from the SBB site");
            return Ok(None);
        }
    };
    let departure_seconds = i64::from(request.first_departure_time.0) * 60;
    let Some(quote) =
        btt_quotes::sbb::find_trip(&quotes, departure_seconds, usize::from(request.ride_count))
    else {
        return Ok(None);
    };
    let class = match request.travel_class {
        TravelClass::First => "FIRST",
        TravelClass::Second => "SECOND",
    };
    let Some(class_price) = quote.prices.as_ref().and_then(|prices| {
        prices
            .prices
            .iter()
            .find(|price| price.travel_class == class)
    }) else {
        return Ok(None);
    };
    if class_price.price.currency != "CHF" {
        return Ok(None);
    }
    let francs = f64::from(class_price.price.amount) / 100.0;
    let cents = EuroCents::from_euro(francs * state.fare_data.chf_to_eur);
    Ok(Some(Price {
        low: cents,
        high: cents,
        kind: PriceKind::Live,
        source: PriceSource {
            name: "SBB booking site".into(),
            note: format!(
                "CHF {francs:.2} for {} class, {}, converted at 1 CHF = {} EUR, a fixed rough rate. \
                 The booking site shows the final price.",
                class.to_lowercase(),
                class_price.after_sale_flexibility.to_lowercase().replace('_', " "),
                state.fare_data.chf_to_eur
            )
            .into(),
            as_of: Some(today_utc()),
            page: Some(request.booking_link),
        },
    }))
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
