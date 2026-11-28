//! Sharing a trip with the web viewer, as a link or as a file.

// Tauri passes `State` and other arguments by value, which the command signatures must match.
#![allow(clippy::needless_pass_by_value)]

use std::path::PathBuf;

use btt_backend::sharing::{
    LinkOptions, SharedTrip, share_snapshot, trip_file_json, trip_file_name, trip_of_file_json,
};
use btt_model::{CommandError, SnapshotLeg, Trip, TripId, TripSnapshot};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::commands::today_utc;
use crate::state::AppState;

/// Environment variable that saves the trip file to a fixed path without asking, for automated
/// tests.
const TRIP_FILE_PATH_VARIABLE: &str = "BTT_TRIP_FILE_PATH";

/// Returns an open trip and its snapshot, from the legs as the frontend shows them.
fn snapshot_of_open_trip(
    state: &AppState,
    trip_id: TripId,
    legs: &[SnapshotLeg],
) -> Result<(Trip, TripSnapshot), CommandError> {
    let trip = state
        .open_trips()
        .get(&trip_id)
        .map(|open_trip| open_trip.trip.clone())
        .ok_or(CommandError::TripNotOpen(trip_id))?;
    let snapshot = TripSnapshot::new(
        &trip,
        legs,
        today_utc(),
        state.planning_backend()?.bundle_version(),
    );
    Ok((trip, snapshot))
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
    let (_, snapshot) = snapshot_of_open_trip(&state, trip_id, &legs)?;
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
/// email and for `AirDrop`. The app can open the file again with `import_trip_file`. Returns the path of the saved file, or `None` when the traveller
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
    let (trip, snapshot) = snapshot_of_open_trip(&state, trip_id, &legs)?;
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
    let json = trip_file_json(snapshot, &trip)?;
    std::fs::write(&path, json).map_err(|error| CommandError::Sharing {
        message: error.to_string(),
    })?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Reads the text of a `.btt` file and stores its trip as a new trip. Returns the new trip.
///
/// The frontend reads the file, so this command works the same for a file that the traveller
/// picked and for a file that was dropped on the window. See `trip_of_file_json` for what the
/// trip holds.
///
/// # Errors
///
/// Returns an error when the text is not a trip file, when a newer version of the app wrote it,
/// or when the trip cannot be stored.
#[tauri::command]
#[specta::specta]
pub fn import_trip_file(
    state: State<'_, AppState>,
    contents: String,
) -> Result<Trip, CommandError> {
    let trip = trip_of_file_json(&contents)?;
    state.storage.save_trip(&trip)?;
    Ok(trip)
}
