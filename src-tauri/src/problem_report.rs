//! Problem reports: a ZIP file with a trip and everything needed to plan it again, so an odd
//! route or a bad ticket price can be reproduced and analysed.

// Tauri passes `State` and other arguments by value, which the command signatures must match.
#![allow(clippy::needless_pass_by_value)]

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use btt_model::{GeoCoordinates, LegPlanningResult, Trip, TripId, TripPlaceId};
use btt_planner::{LegExplanation, LegPlanningInput, leg_chain_inputs};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use specta::Type;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::commands::CommandError;
use crate::state::AppState;
use crate::ticket_report::{LiveQuoteLogEntry, TicketPlanRecord};

/// Environment variable that saves reports to a fixed path without asking, for automated tests.
const REPORT_PATH_VARIABLE: &str = "BTT_PROBLEM_REPORT_PATH";

/// What the map showed when the traveller made the report.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ProblemReportView {
    /// The place selected in the list or on the map, if any.
    pub selected_place_id: Option<TripPlaceId>,
    /// Centre of the map.
    pub map_center: GeoCoordinates,
    /// Zoom level of the map.
    pub map_zoom: f64,
}

/// Contents of `report.json`: the note and the circumstances of the report.
#[derive(Serialize)]
struct ReportSummary {
    /// The traveller's description of the problem.
    note: String,
    /// When the report was made, in RFC 3339 format.
    created_at: String,
    /// Version of the app.
    app_version: &'static str,
    /// Operating system the app runs on.
    operating_system: &'static str,
    /// Which planner planned the legs.
    backend: &'static str,
    /// The timetable bundle, when the planner uses one.
    bundle: Option<BundleSummary>,
}

/// Identifies the timetable bundle a report was made with.
#[derive(Serialize)]
struct BundleSummary {
    /// File name of the bundle.
    file_name: String,
    /// SHA-256 of the bundle file, in hexadecimal.
    sha256: String,
}

/// One leg in `legs.json`.
#[derive(Serialize)]
struct ReportedLeg {
    /// What the planner received for the leg.
    input: LegPlanningInput,
    /// The leg's key in the app's cache of planned legs.
    cache_key: u64,
    /// The result the app has stored for the leg, which the traveller saw.
    stored_result: Option<LegPlanningResult>,
    /// The leg planned again now on its own, with the candidate stations and the cost of every
    /// journey. A leg of a chain joined by same-day stops is planned with the chain in the app,
    /// so its explanation can differ from the stored result.
    explanation: LegExplanation,
}

/// Contents of `tickets.json`: the ticket plan the traveller saw and the live price lookups.
#[derive(Serialize)]
struct TicketsReport {
    /// The last ticket plan of the trip with what it was made from, or `None` when the Tickets
    /// screen was not opened.
    plan: Option<TicketPlanRecord>,
    /// The most recent live price lookups, of every trip, with the answers of the booking site.
    live_quote_lookups: Vec<LiveQuoteLogEntry>,
    /// The number of live prices the price model has learned from, with those of the seed run.
    price_model_observations: u32,
}

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
    let file_name = format!(
        "btt-problem-report-{}.zip",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    );
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
    let namespace = planning_backend.cache_namespace();
    let chains = leg_chain_inputs(&trip);
    let mut legs = Vec::new();
    for chain in &chains {
        for (index, identified) in chain.legs.iter().enumerate() {
            let cache_key = chain.leg_cache_key(namespace, index);
            legs.push(ReportedLeg {
                stored_result: state.storage.cached_leg_result(cache_key.0)?,
                explanation: planning_backend.explain_leg(&identified.input).await,
                input: identified.input.clone(),
                cache_key: cache_key.0,
            });
        }
    }
    let summary = ReportSummary {
        note,
        created_at: chrono::Local::now().to_rfc3339(),
        app_version: env!("CARGO_PKG_VERSION"),
        operating_system: std::env::consts::OS,
        backend: planning_backend.kind_name(),
        bundle: state.bundle_path.as_deref().and_then(bundle_summary),
    };
    let tickets = TicketsReport {
        price_model_observations: state.fare_data.price_model.read().observation_count(),
        plan: state
            .ticket_plans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&trip_id)
            .cloned(),
        live_quote_lookups: state
            .live_quote_log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
    };
    let files = [
        ("report.json", json(&summary)?),
        ("trip.json", btt_storage::trip_to_document(&trip)?),
        ("legs.json", json(&legs)?),
        ("view.json", json(&view)?),
        ("tickets.json", json(&tickets)?),
        ("log.txt", state.recent_log_lines.text()),
    ];
    write_zip(&path, &files).map_err(|error| CommandError::Storage {
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

/// Serializes a value as indented JSON, which people read in these reports.
fn json(value: &impl Serialize) -> Result<String, CommandError> {
    serde_json::to_string_pretty(value).map_err(|error| CommandError::Storage {
        message: error.to_string(),
    })
}

/// Writes text files into a new ZIP file.
fn write_zip(path: &Path, files: &[(&str, String)]) -> zip::result::ZipResult<()> {
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path)?);
    for (name, contents) in files {
        writer.start_file(*name, zip::write::SimpleFileOptions::default())?;
        writer.write_all(contents.as_bytes())?;
    }
    writer.finish()?;
    Ok(())
}
