//! Problem reports: a ZIP file with a trip and everything needed to plan it again, so an odd
//! route or a bad ticket price can be reproduced and analysed. The desktop app and the web app
//! fill in the parts that only they have, such as the log or the live price lookups.

use std::io::{Cursor, Write};

use btt_model::{
    CommandError, GeoCoordinates, LegPlanningResult, TicketLeg, TicketPlan, TicketPreferences,
    Traveller, Trip, TripPlaceId,
};
use btt_planner::{
    JourneyPricer, LegCacheKey, LegExplanation, LegPlanningBackend, LegPlanningInput,
    leg_chain_inputs,
};
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use specta::Type;

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
pub struct ReportSummary {
    /// The traveller's description of the problem.
    pub note: String,
    /// When the report was made, in RFC 3339 format.
    pub created_at: String,
    /// Version of the app.
    pub app_version: &'static str,
    /// Operating system the app runs on, or `web` for the web app.
    pub operating_system: &'static str,
    /// Which planner planned the legs.
    pub backend: &'static str,
    /// Version of the timetable data as `yyyymmddNN`, when the planner uses a bundle.
    pub timetable_version: Option<u32>,
    /// The timetable bundle file, when the planner reads one from the disk.
    pub bundle: Option<BundleSummary>,
}

/// Identifies the timetable bundle file a report was made with.
#[derive(Serialize)]
pub struct BundleSummary {
    /// File name of the bundle.
    pub file_name: String,
    /// SHA-256 of the bundle file, in hexadecimal.
    pub sha256: String,
}

/// One leg in `legs.json`.
#[derive(Serialize)]
pub struct ReportedLeg {
    /// What the planner received for the leg.
    pub input: LegPlanningInput,
    /// The leg's key in the app's cache of planned legs.
    pub cache_key: u64,
    /// The result the app has stored for the leg, which the traveller saw.
    pub stored_result: Option<LegPlanningResult>,
    /// The leg planned again now on its own, with the candidate stations and the cost of every
    /// journey. A leg of a chain joined by same-day stops is planned with the chain in the app,
    /// so its explanation can differ from the stored result.
    pub explanation: LegExplanation,
}

/// The last ticket plan of a trip with what it was made from, so the prices can be reproduced.
#[derive(Debug, Clone, Serialize)]
pub struct TicketPlanRecord {
    /// When the plan was made, in RFC 3339 format.
    pub created_at: String,
    /// The chosen journey of each leg that went into the plan.
    pub legs: Vec<TicketLeg>,
    /// The traveller the plan is for, or `None` when it assumed an adult.
    pub traveller: Option<Traveller>,
    /// The ticket preferences of the trip.
    pub preferences: TicketPreferences,
    /// The plan the app showed, with the estimates and the booking links.
    pub plan: TicketPlan,
}

/// Contents of `tickets.json`: the ticket plan the traveller saw and the live price lookups.
/// `Lookup` is the type of one logged lookup, which only the desktop app makes.
#[derive(Serialize)]
pub struct TicketsReport<'a, Lookup: Serialize> {
    /// The last ticket plan of the trip, or `None` when the Tickets screen was not opened.
    pub plan: Option<TicketPlanRecord>,
    /// The most recent live price lookups, of every trip, with the answers of the booking site.
    pub live_quote_lookups: &'a [Lookup],
    /// The number of live prices the price model has learned from, with those of the seed run.
    pub price_model_observations: u32,
}

/// Returns the file name of a report made at `now`.
#[must_use]
pub fn report_file_name(now: DateTime<FixedOffset>) -> String {
    format!("btt-problem-report-{}.zip", now.format("%Y%m%d-%H%M%S"))
}

/// Plans each leg of the trip again with an explanation, next to the stored result that
/// `stored_result` returns for the cache key of the leg.
///
/// # Errors
///
/// Returns the error of `stored_result`.
pub async fn reported_legs<P: JourneyPricer>(
    backend: &LegPlanningBackend<P>,
    trip: &Trip,
    stored_result: impl Fn(LegCacheKey) -> Result<Option<LegPlanningResult>, CommandError>,
) -> Result<Vec<ReportedLeg>, CommandError> {
    let namespace = backend.cache_namespace();
    let mut legs = Vec::new();
    for chain in &leg_chain_inputs(trip) {
        for (index, identified) in chain.legs.iter().enumerate() {
            let cache_key = chain.leg_cache_key(namespace, index);
            legs.push(ReportedLeg {
                stored_result: stored_result(cache_key)?,
                explanation: backend.explain_leg(&identified.input).await,
                input: identified.input.clone(),
                cache_key: cache_key.0,
            });
        }
    }
    Ok(legs)
}

/// Serializes a value as indented JSON, which people read in these reports.
///
/// # Errors
///
/// Returns a storage error when the value cannot be serialized.
pub fn report_json(value: &impl Serialize) -> Result<String, CommandError> {
    serde_json::to_string_pretty(value).map_err(|error| CommandError::Storage {
        message: error.to_string(),
    })
}

/// Writes text files into a new ZIP archive and returns its bytes.
///
/// # Errors
///
/// Returns a storage error when the archive cannot be written.
pub fn report_zip(files: &[(&str, String)]) -> Result<Vec<u8>, CommandError> {
    let write = || -> zip::result::ZipResult<Vec<u8>> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, contents) in files {
            writer.start_file(*name, zip::write::SimpleFileOptions::default())?;
            writer.write_all(contents.as_bytes())?;
        }
        Ok(writer.finish()?.into_inner())
    };
    write().map_err(|error| CommandError::Storage {
        message: format!("could not write the report: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};

    use super::*;

    #[test]
    fn zip_holds_each_file_with_its_text() {
        let files = [
            ("report.json", "{\"note\": \"a\"}".to_owned()),
            ("log.txt", "line one\nline two\n".to_owned()),
        ];
        let bytes = report_zip(&files).expect("the archive is written");
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("the archive is valid");
        assert_eq!(archive.len(), 2);
        for (name, contents) in &files {
            let mut text = String::new();
            archive
                .by_name(name)
                .expect("the file is in the archive")
                .read_to_string(&mut text)
                .expect("the file is text");
            assert_eq!(&text, contents);
        }
    }

    #[test]
    fn file_name_has_the_time_of_the_report() {
        let moment = DateTime::parse_from_rfc3339("2026-03-04T05:06:07+01:00").expect("valid time");
        assert_eq!(
            report_file_name(moment),
            "btt-problem-report-20260304-050607.zip"
        );
    }
}
