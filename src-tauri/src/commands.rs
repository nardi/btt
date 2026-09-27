//! Commands the frontend calls. `tauri-specta` generates their TypeScript wrappers.

// Tauri passes `State` and other arguments by value, which the command signatures must match.
#![allow(clippy::needless_pass_by_value)]

use btt_model::edit::NewTripPlace;
use btt_model::{
    GeoCoordinates, PlaceInsertionPoint, PlaceSearchResult, PlanningSessionEvent,
    StationSearchRadius, Trip, TripEdit, TripEditError, TripEditOutcome, TripId, TripSummary,
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
