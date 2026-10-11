//! Plans the benchmark queries of MOTIS as legs, a daytime and a night leg for each, and records
//! or checks the chosen journeys and the planning times.
//!
//! Run `cargo run --release -p btt-planner --example benchmark_legs -- record <bundle>
//! <queries.txt> <responses.txt> <results.tsv>` to write the chosen journey, its cost and the
//! planning time of each leg. Run it again with `check` in place of `record` after a change to
//! the planner. The check lists every leg whose chosen journey or cost differs from the file,
//! and compares the planning times.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Instant;

use btt_geo::TimeZoneFinder;
use btt_model::{
    DurationMinutes, GeoCoordinates, LegPlanningResult, LegTravelMode, OvernightLegOptions,
    StationSearchRadius, TripPreferences,
};
use btt_planner::{LegEndpoint, LegPlanningInput, NoPrice, TimetableBundleRouter};
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// The parts of a MOTIS plan response the benchmark reads.
#[derive(Deserialize)]
struct MotisResponse {
    from: Option<MotisPlace>,
    to: Option<MotisPlace>,
}

#[derive(Deserialize)]
struct MotisPlace {
    name: String,
    lat: f64,
    lon: f64,
}

/// What a leg planned to.
struct LegResult {
    /// The leg's description, with its travel mode.
    description: String,
    /// The chosen journey as the trains and times of its rides, and its cost.
    journey: String,
    /// Milliseconds spent planning.
    milliseconds: f64,
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let mode = arguments.next().expect("pass record or check");
    let bundle_path = arguments.next().expect("pass the bundle");
    let queries_path = arguments.next().expect("pass queries.txt");
    let responses_path = arguments.next().expect("pass responses.txt");
    let results_path = arguments.next().expect("pass the results file");
    let limit: usize = arguments
        .next()
        .and_then(|limit| limit.parse().ok())
        .unwrap_or(usize::MAX);

    let router = TimetableBundleRouter::open(
        bundle_path.as_ref(),
        Arc::new(TimeZoneFinder::new()),
        NoPrice,
    )
    .expect("bundle loads");
    let results = plan_queries(&router, &queries_path, &responses_path, limit);
    match mode.as_str() {
        "record" => {
            let mut text = String::new();
            for result in &results {
                // Writing to a string never fails.
                let _ = writeln!(
                    text,
                    "{}\t{}\t{:.1}",
                    result.description, result.journey, result.milliseconds
                );
            }
            std::fs::write(&results_path, text).expect("results write");
            print_times("recorded", &results);
        }
        "check" => check(&results, &results_path),
        other => panic!("unknown mode {other}"),
    }
}

/// Plans every query as a daytime and as a night leg.
fn plan_queries(
    router: &TimetableBundleRouter,
    queries_path: &str,
    responses_path: &str,
    limit: usize,
) -> Vec<LegResult> {
    let preferences = TripPreferences::default();
    let queries = std::fs::read_to_string(queries_path).expect("queries read");
    let responses = std::fs::read_to_string(responses_path).expect("responses read");
    let mut results = Vec::new();
    for (query, response) in queries.lines().zip(responses.lines()).take(limit) {
        let Some(time) = query
            .split(['?', '&'])
            .find_map(|part| part.strip_prefix("time="))
            .and_then(|time| time.parse::<DateTime<Utc>>().ok())
        else {
            continue;
        };
        let Ok(MotisResponse {
            from: Some(from),
            to: Some(to),
        }) = serde_json::from_str::<MotisResponse>(response)
        else {
            continue;
        };
        let (Ok(from_coordinates), Ok(to_coordinates)) = (
            GeoCoordinates::from_degrees(from.lat, from.lon),
            GeoCoordinates::from_degrees(to.lat, to.lon),
        ) else {
            continue;
        };
        let endpoint = |coordinates, time_window| LegEndpoint {
            coordinates,
            station_search_radius: StationSearchRadius::DEFAULT,
            time_window,
        };
        for (name, travel_mode) in [
            ("day", LegTravelMode::Daytime),
            (
                "night",
                LegTravelMode::Overnight(OvernightLegOptions::DEFAULT),
            ),
        ] {
            let input = LegPlanningInput {
                departure: endpoint(from_coordinates, preferences.default_windows.departure),
                arrival: endpoint(to_coordinates, preferences.default_windows.arrival),
                departure_date: time.date_naive(),
                travel_mode,
                minimum_transfer_time: DurationMinutes(10),
                pass_policy: preferences.pass_policy,
                transfer_aversion: preferences.transfer_aversion,
                chosen_journey: None,
            };
            let started = Instant::now();
            let result = router.plan_leg(&input, || false);
            let milliseconds = started.elapsed().as_secs_f64() * 1000.0;
            results.push(LegResult {
                description: format!(
                    "{} to {} on {}, {name}",
                    from.name,
                    to.name,
                    time.date_naive()
                ),
                journey: describe(&result),
                milliseconds,
            });
        }
    }
    results
}

/// Describes the chosen journey of a result by the number, the departure and the arrival of
/// each ride, with the journey's cost.
fn describe(result: &LegPlanningResult) -> String {
    match result {
        LegPlanningResult::NoJourneyFound(reason) => format!("none {reason:?}"),
        LegPlanningResult::Found(plan) => {
            let rides: Vec<String> = plan
                .chosen_journey
                .rides()
                .map(|ride| {
                    format!(
                        "{} {}-{}",
                        ride.train_label.number, ride.departure_time.0, ride.arrival_time.0
                    )
                })
                .collect();
            format!("{} | cost {}", rides.join(", "), plan.best_journey_cost)
        }
    }
}

/// Compares the results with the recorded file.
fn check(results: &[LegResult], results_path: &str) {
    let recorded = std::fs::read_to_string(results_path).expect("results read");
    let recorded: Vec<Vec<&str>> = recorded
        .lines()
        .map(|line| line.split('\t').collect())
        .collect();
    let mut differences = 0;
    let mut recorded_results = Vec::new();
    for (result, line) in results.iter().zip(&recorded) {
        if line[0] != result.description || line[1] != result.journey {
            differences += 1;
            if differences <= 200 {
                println!(
                    "{}\n  recorded: {}\n  now:      {}",
                    result.description, line[1], result.journey
                );
            }
        }
        recorded_results.push(LegResult {
            description: line[0].to_owned(),
            journey: line[1].to_owned(),
            milliseconds: line[2].parse().unwrap_or(0.0),
        });
    }
    println!("{differences} of {} legs differ", results.len());
    print_times("recorded", &recorded_results);
    print_times("now", results);
}

/// Prints the median, the 90th percentile and the largest planning time, and the total, for
/// the daytime and the night legs.
fn print_times(name: &str, results: &[LegResult]) {
    for mode in ["day", "night"] {
        let mut times: Vec<f64> = results
            .iter()
            .filter(|result| result.description.ends_with(mode))
            .map(|result| result.milliseconds)
            .collect();
        times.sort_by(f64::total_cmp);
        let at = |fraction: f64| {
            // Rounding to an index of a short list stays well within `usize`.
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let position = (times.len().saturating_sub(1) as f64 * fraction).round() as usize;
            times.get(position).copied().unwrap_or_default()
        };
        println!(
            "  {name} {mode}: median {:.1} ms, 90th percentile {:.1} ms, largest {:.1} ms, total {:.1} s",
            at(0.5),
            at(0.9),
            at(1.0),
            times.iter().sum::<f64>() / 1000.0
        );
    }
}
