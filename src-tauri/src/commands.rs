//! Commands the frontend calls. `tauri-specta` generates their TypeScript wrappers.

// Tauri passes `State` and other arguments by value, which the command signatures must match.
#![allow(clippy::needless_pass_by_value)]

use btt_model::{
    PlaceSearchResult, PlanningSessionEvent, Trip, TripEdit, TripEditError, TripEditOutcome,
    TripId, TripSummary,
};
use chrono::NaiveDate;
use serde::Serialize;
use smol_str::SmolStr;
use specta::Type;
use tauri::State;
use tauri::ipc::Channel;
use thiserror::Error;

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
    state: State<'_, AppState>,
    trip_id: TripId,
    edit: TripEdit,
) -> Result<TripEditOutcome, CommandError> {
    let mut open_trips = state.open_trips();
    let open_trip = open_trips
        .get_mut(&trip_id)
        .ok_or(CommandError::TripNotOpen(trip_id))?;
    let mut trip = open_trip.trip.clone();
    let notices = trip.apply_edit(edit).map_err(CommandError::InvalidEdit)?;
    // Saving under the lock keeps the stored order of edits the same as the applied order.
    state.storage.save_trip(&trip)?;
    open_trip.trip = trip.clone();
    open_trip.planning_session.trip_changed(trip.clone());
    Ok(TripEditOutcome { trip, notices })
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
