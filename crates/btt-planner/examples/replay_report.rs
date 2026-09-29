//! Plans every leg of a problem report again on a timetable bundle and compares the result with
//! the one the traveller saw.
//!
//! Run with `cargo run --release -p btt-planner --example replay_report -- <report.zip> <bundle>`.
//! With the bundle the report was made with, a difference points at a change in the planner.

use std::fmt::Write as _;
use std::io::Read;
use std::sync::Arc;

use btt_geo::TimeZoneFinder;
use btt_model::{Journey, LegPlan, LegPlanningResult};
use btt_planner::{LegPlanningInput, TimetableBundleRouter};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The parts of `report.json` the replay reads.
#[derive(Deserialize)]
struct ReportSummary {
    note: String,
    bundle: Option<BundleSummary>,
}

#[derive(Deserialize)]
struct BundleSummary {
    file_name: String,
    sha256: String,
}

/// The parts of one entry of `legs.json` the replay reads.
#[derive(Deserialize)]
struct ReportedLeg {
    input: LegPlanningInput,
    stored_result: Option<LegPlanningResult>,
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let report_path = arguments.next().expect("pass the report ZIP");
    let bundle_path = arguments.next().expect("pass the bundle path");

    let mut archive =
        zip::ZipArchive::new(std::fs::File::open(&report_path).expect("report opens"))
            .expect("report is a ZIP file");
    let summary: ReportSummary =
        serde_json::from_str(&read_entry(&mut archive, "report.json")).expect("report.json parses");
    let legs: Vec<ReportedLeg> =
        serde_json::from_str(&read_entry(&mut archive, "legs.json")).expect("legs.json parses");

    println!("Note: {}", summary.note);
    let bundle_bytes = std::fs::read(&bundle_path).expect("bundle reads");
    let bundle_sha256: String =
        Sha256::digest(&bundle_bytes)
            .iter()
            .fold(String::new(), |mut hex, byte| {
                // Writing to a string never fails.
                let _ = write!(hex, "{byte:02x}");
                hex
            });
    match &summary.bundle {
        Some(reported) if reported.sha256 == bundle_sha256 => {
            println!("Bundle: the same as in the report ({})", reported.file_name);
        }
        Some(reported) => println!(
            "Bundle: differs from the report's {}, so differences can come from the timetable",
            reported.file_name
        ),
        None => println!("Bundle: the report was made without a bundle"),
    }

    let router = TimetableBundleRouter::open(bundle_path.as_ref(), Arc::new(TimeZoneFinder::new()))
        .expect("bundle loads");
    for (index, leg) in legs.iter().enumerate() {
        let replanned = router.plan_leg(&leg.input, || false);
        println!("\nLeg {}", index + 1);
        let stored = leg
            .stored_result
            .as_ref()
            .map_or_else(|| "not planned".to_owned(), describe_result);
        let now = describe_result(&replanned);
        println!("  in the report: {stored}");
        if stored == now {
            println!("  now:           the same");
        } else {
            println!("  now:           {now}");
        }
    }
}

/// Reads a text file from the report.
fn read_entry(archive: &mut zip::ZipArchive<std::fs::File>, name: &str) -> String {
    let mut text = String::new();
    archive
        .by_name(name)
        .unwrap_or_else(|_| panic!("the report has {name}"))
        .read_to_string(&mut text)
        .expect("entry reads as text");
    text
}

/// Describes a leg result by its chosen journey, such as "ICE 145 at 29864628".
fn describe_result(result: &LegPlanningResult) -> String {
    match result {
        LegPlanningResult::NoJourneyFound(reason) => format!("no journey ({reason:?})"),
        LegPlanningResult::Found(plan) => describe_journey(plan, &plan.chosen_journey),
    }
}

fn describe_journey(plan: &LegPlan, journey: &Journey) -> String {
    let trains: Vec<String> = journey
        .rides()
        .map(|ride| format!("{} {}", ride.train_label.category, ride.train_label.number))
        .collect();
    let first_station = plan
        .station_table
        .get(usize::from(journey.first_ride.boarding_station.0))
        .map_or("?", |station| station.name.as_str());
    format!(
        "{} from {first_station}, first departure at minute {} since 1970",
        trains.join(", "),
        journey.first_ride.departure_time.0
    )
}
