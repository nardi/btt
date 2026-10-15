//! Sharing a trip with the web viewer, as a link or as a file.

// Tauri passes `State` and other arguments by value, which the command signatures must match.
#![allow(clippy::needless_pass_by_value)]

use std::path::PathBuf;

use btt_model::{
    LinkAlphabet, SNAPSHOT_SCHEMA_VERSION, SnapshotLeg, SnapshotLinkError, StoredSnapshot, TripId,
    TripSnapshot, encode_snapshot_link_in,
};
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::commands::{CommandError, today_utc};
use crate::state::AppState;

/// Web address of the viewer, which GitHub Pages serves from the repository.
const VIEWER_URL: &str = "https://nardilam.github.io/btt/";

/// Link length in characters above which some messengers cut the link.
const LONG_LINK_CHARACTERS: usize = 8 * 1024;

/// Environment variable that saves the trip file to a fixed path without asking, for automated
/// tests.
const TRIP_FILE_PATH_VARIABLE: &str = "BTT_TRIP_FILE_PATH";

/// A trip written as a link for the viewer.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SharedTrip {
    /// The whole web address, with the trip after the `#`.
    pub url: String,
    /// Length of the address in characters.
    pub link_characters: u32,
    /// Whether the address is long enough that some messengers cut it.
    pub long_for_messengers: bool,
    /// The same trip as the text of a QR code, written in the alphabet of the alphanumeric mode of
    /// a QR code, which keeps the code smaller than the link would. `None` when the trip is too
    /// big for it.
    pub qr: Option<QrText>,
}

/// What a QR code holds: the web address of the viewer and the trip.
#[derive(Debug, Clone, Serialize, Type)]
pub struct QrText {
    /// The web address of the viewer with the `#`, which goes in a segment of bytes.
    pub address: String,
    /// The trip, which goes in a segment of alphanumeric characters.
    pub payload: String,
}

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
        state.planning_backend.bundle_version(),
    ))
}

/// Writes an open trip with the journeys the frontend shows as a link for the web viewer.
///
/// The link holds the trip after the `#`, so a browser never sends it to a server. It holds no
/// traveller profiles. It holds the chosen journey of every leg, and the alternatives too when
/// `include_alternatives` is set, which makes the link about three times as long. A `compact`
/// link is written in a bigger alphabet and has less than half the characters, but a browser
/// keeps it percent-encoded, so an app that shows it that way shows a longer link.
///
/// # Errors
///
/// Returns an error when the trip is not open or has more than a link can carry.
#[tauri::command]
#[specta::specta]
pub fn share_trip(
    state: State<'_, AppState>,
    trip_id: TripId,
    legs: Vec<SnapshotLeg>,
    include_alternatives: bool,
    compact: bool,
) -> Result<SharedTrip, CommandError> {
    let mut snapshot = snapshot_of_open_trip(&state, trip_id, &legs)?;
    if !include_alternatives {
        snapshot = snapshot.chosen_journeys_only();
    }
    let write = |alphabet| {
        encode_snapshot_link_in(&snapshot, alphabet).map_err(|error| match error {
            SnapshotLinkError::TooLarge => CommandError::TripTooLargeForLink,
            other => CommandError::Sharing {
                message: other.to_string(),
            },
        })
    };
    let alphabet = if compact {
        LinkAlphabet::Compact
    } else {
        LinkAlphabet::Fragment
    };
    let link = write(alphabet)?;
    let qr_link = write(LinkAlphabet::QrCode).ok();
    let url = format!("{VIEWER_URL}#{}", link.payload);
    let link_characters = u32::try_from(url.chars().count()).unwrap_or(u32::MAX);
    Ok(SharedTrip {
        long_for_messengers: url.chars().count() > LONG_LINK_CHARACTERS,
        url,
        link_characters,
        qr: qr_link.map(|qr_link| QrText {
            address: format!("{VIEWER_URL}#"),
            payload: qr_link.payload,
        }),
    })
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
    let file_name = format!("{}.btt", file_stem(&snapshot.trip.name));
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
    let document = StoredSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        snapshot,
    };
    let json = serde_json::to_string(&document).map_err(|error| CommandError::Sharing {
        message: error.to_string(),
    })?;
    std::fs::write(&path, json).map_err(|error| CommandError::Sharing {
        message: error.to_string(),
    })?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Returns a trip name as the name of a file, without the characters file systems reject.
fn file_stem(name: &str) -> String {
    let stem: String = name
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == ' ' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect();
    let stem = stem.trim();
    if stem.is_empty() {
        "trip".to_owned()
    } else {
        stem.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_name_drops_the_characters_file_systems_reject() {
        assert_eq!(file_stem("Wien / Roma: 2026"), "Wien _ Roma_ 2026");
        assert_eq!(file_stem("  "), "trip");
        assert_eq!(file_stem("Zürich"), "Zürich");
    }
}
