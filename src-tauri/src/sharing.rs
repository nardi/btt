//! Sharing a trip with the web viewer, as a link or as a file.

// Tauri passes `State` and other arguments by value, which the command signatures must match.
#![allow(clippy::needless_pass_by_value)]

use std::path::PathBuf;

use btt_backend::sharing::{
    LinkOptions, SharedTrip, share_snapshot, trip_file_json, trip_file_name,
};
use btt_model::{CommandError, SnapshotLeg, TripId, TripSnapshot};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::commands::today_utc;
use crate::state::AppState;

/// Environment variable that saves the trip file to a fixed path without asking, for automated
/// tests.
const TRIP_FILE_PATH_VARIABLE: &str = "BTT_TRIP_FILE_PATH";

/// Builds the snapshot of an open trip, from the legs as the frontend shows them.
fn snapshot_of_open_trip(
    state: &AppState,
    trip_id: TripId,
    legs: &[SnapshotLeg],
) -> Result<TripSnapshot, CommandError> {
    let trip = state
        .open_trips()
        .get(&trip_id)
        .map(|open_trip| open_trip.trip.clone())
        .ok_or(CommandError::TripNotOpen(trip_id))?;
    Ok(TripSnapshot::new(
        &trip,
        legs,
        today_utc(),
        state.planning_backend()?.bundle_version(),
    ))
}

/// Writes an open trip with the journeys the frontend shows as a link for the web viewer.
///
/// The link holds the trip after the `#`, so a browser never sends it to a server. It holds no
/// traveller profiles. It holds the chosen journey of every leg, and the alternatives too when
/// `include_alternatives` is set, which makes the link about three times as long. It holds the
/// price estimates of the journeys unless `include_prices` is unset, which makes the link a little
/// shorter. It holds the names of the places unless `name_places_by_station` is set, which leaves
/// out the names that the city dictionary cannot shorten, and the viewer then names those places
/// by their nearest station. A `compact` link is written in a bigger alphabet and has less than
/// half the characters, but a browser keeps it percent-encoded, so an app that shows it that way
/// shows a longer link.
///
/// # Errors
///
/// Returns an error when the trip is not open or has more than a link can carry.
// Each flag is one checkbox of the share dialog, so a struct would only rename them.
#[allow(clippy::fn_params_excessive_bools)]
#[tauri::command]
#[specta::specta]
pub fn share_trip(
    state: State<'_, AppState>,
    trip_id: TripId,
    legs: Vec<SnapshotLeg>,
    include_alternatives: bool,
    include_prices: bool,
    name_places_by_station: bool,
    compact: bool,
) -> Result<SharedTrip, CommandError> {
    let snapshot = snapshot_of_open_trip(&state, trip_id, &legs)?;
    share_snapshot(
        snapshot,
        LinkOptions {
            include_alternatives,
            include_prices,
            name_places_by_station,
            compact,
        },
    )
}

/// Asks where to save the trip as a file and writes it there, for the viewer's open button, for
/// email and for `AirDrop`. Returns the path of the saved file, or `None` when the traveller
/// cancelled.
///
/// # Errors
///
/// Returns an error when the trip is not open or the file cannot be written.
#[tauri::command]
#[specta::specta]
pub async fn export_trip_file(
    app: AppHandle,
    state: State<'_, AppState>,
    trip_id: TripId,
    legs: Vec<SnapshotLeg>,
) -> Result<Option<String>, CommandError> {
    let snapshot = snapshot_of_open_trip(&state, trip_id, &legs)?;
    let file_name = trip_file_name(&snapshot.trip.name);
    let chosen_path = std::env::var_os(TRIP_FILE_PATH_VARIABLE)
        .map(PathBuf::from)
        .or_else(|| {
            app.dialog()
                .file()
                .set_title("Save trip file")
                .set_file_name(&file_name)
                .add_filter("BTT trip", &["btt"])
                .blocking_save_file()
                .and_then(|file_path| file_path.into_path().ok())
        });
    let Some(path) = chosen_path else {
        return Ok(None);
    };
    let json = trip_file_json(snapshot)?;
    std::fs::write(&path, json).map_err(|error| CommandError::Sharing {
        message: error.to_string(),
    })?;
    Ok(Some(path.to_string_lossy().into_owned()))
}
