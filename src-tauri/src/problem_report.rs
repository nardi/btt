//! Problem reports: a ZIP file with a trip and everything needed to plan it again, so an odd
//! route or a bad ticket price can be reproduced and analysed.

// Tauri passes `State` and other arguments by value, which the command signatures must match.
#![allow(clippy::needless_pass_by_value)]

use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};

use btt_backend::problem_report::{
    BundleSummary, ProblemReportView, ReportSummary, TicketsReport, report_file_name, report_json,
    report_zip, reported_legs,
};
use btt_model::{Trip, TripId};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::state::AppState;
use btt_model::CommandError;

/// Environment variable that saves reports to a fixed path without asking, for automated tests.
const REPORT_PATH_VARIABLE: &str = "BTT_PROBLEM_REPORT_PATH";

/// Asks where to save a problem report for a trip and writes it there. Returns the path of the
/// saved file, or `None` when the traveller cancelled.
///
/// # Errors
///
/// Returns an error when the trip does not exist or the file cannot be written.
#[tauri::command]
#[specta::specta]
pub async fn export_problem_report(
    app: AppHandle,
    state: State<'_, AppState>,
    trip_id: TripId,
    note: String,
    view: ProblemReportView,
) -> Result<Option<String>, CommandError> {
    let trip = open_or_stored_trip(&state, trip_id)?;
    let now = chrono::Local::now().fixed_offset();
    let file_name = report_file_name(now);
    let chosen_path = std::env::var_os(REPORT_PATH_VARIABLE)
        .map(PathBuf::from)
        .or_else(|| {
            app.dialog()
                .file()
                .set_title("Save problem report")
                .set_file_name(&file_name)
                .add_filter("ZIP archive", &["zip"])
                .blocking_save_file()
                .and_then(|file_path| file_path.into_path().ok())
        });
    let Some(path) = chosen_path else {
        return Ok(None);
    };

    let planning_backend = state.planning_backend()?;
    let legs = reported_legs(&planning_backend, &trip, |cache_key| {
        Ok(state.storage.cached_leg_result(cache_key.0)?)
    })
    .await?;
    let summary = ReportSummary {
        note,
        created_at: now.to_rfc3339(),
        app_version: env!("CARGO_PKG_VERSION"),
        operating_system: std::env::consts::OS,
        backend: planning_backend.kind_name(),
        timetable_version: Some(planning_backend.bundle_version()).filter(|version| *version != 0),
        bundle: state.bundle_path().as_deref().and_then(bundle_summary),
    };
    let live_quote_lookups = state
        .live_quote_log
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let tickets = TicketsReport {
        price_model_observations: state.fare_data.price_model.read().observation_count(),
        plan: state
            .ticket_plans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&trip_id)
            .cloned(),
        live_quote_lookups: &live_quote_lookups,
    };
    let files = [
        ("report.json", report_json(&summary)?),
        ("trip.json", btt_storage::trip_to_document(&trip)?),
        ("legs.json", report_json(&legs)?),
        ("view.json", report_json(&view)?),
        ("tickets.json", report_json(&tickets)?),
        ("log.txt", state.recent_log_lines.text()),
    ];
    std::fs::write(&path, report_zip(&files)?).map_err(|error| CommandError::Storage {
        message: format!("could not write {}: {error}", path.display()),
    })?;
    tracing::info!(path = %path.display(), "saved a problem report");
    Ok(Some(path.display().to_string()))
}

/// Returns the trip as the app has it open, or else as it is stored.
fn open_or_stored_trip(state: &AppState, trip_id: TripId) -> Result<Trip, CommandError> {
    if let Some(open_trip) = state.open_trips().get(&trip_id) {
        return Ok(open_trip.trip.clone());
    }
    state
        .storage
        .load_trip(trip_id)?
        .ok_or(CommandError::TripNotFound(trip_id))
}

/// Names and hashes a bundle file. Returns `None` when it cannot be read.
fn bundle_summary(path: &Path) -> Option<BundleSummary> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let mut sha256 = String::with_capacity(64);
    for byte in hasher.finalize() {
        // Writing to a string never fails.
        let _ = write!(sha256, "{byte:02x}");
    }
    Some(BundleSummary {
        file_name: path.file_name()?.to_string_lossy().into_owned(),
        sha256,
    })
}
