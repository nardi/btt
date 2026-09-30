//! Plans MOTIS's benchmark queries as legs with the plain search and with the cost-aware
//! search, and compares the chosen journeys, their costs and the planning times.
//!
//! Run with `cargo run --release -p btt-planner --example compare_cost_search -- <bundle>
//! <queries.txt> <responses.txt>`, with the files `compare_motis` reads. Each query becomes a
//! daytime leg on its date with the default windows, and an overnight leg with the default
//! bedtime and wake-up time.

use std::sync::Arc;
use std::time::{Duration, Instant};

use btt_geo::TimeZoneFinder;
use btt_model::{
    DurationMinutes, GeoCoordinates, Journey, LegPlanningResult, LegTravelMode,
    OvernightLegOptions, StationSearchRadius, TripPreferences,
};
use btt_planner::{LegEndpoint, LegPlanningInput, TimetableBundleRouter};
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// The parts of a MOTIS plan response the comparison reads.
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

/// How the cost-aware search's plan compares with the plain one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    SameJourney,
    Cheaper,
    SameCost,
    Costlier,
    OnlyPlain,
    OnlyCostAware,
    Neither,
}

/// Results per travel mode.
#[derive(Default)]
struct Tally {
    outcomes: Vec<(Outcome, i64, String)>,
    plain_times: Vec<Duration>,
    cost_aware_times: Vec<Duration>,
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let bundle_path = arguments.next().expect("pass the bundle");
    let queries_path = arguments.next().expect("pass queries.txt");
    let responses_path = arguments.next().expect("pass responses.txt");
    // Only the first this many queries run, when given.
    let limit: usize = arguments
        .next()
        .and_then(|limit| limit.parse().ok())
        .unwrap_or(usize::MAX);

    let time_zone_finder = Arc::new(TimeZoneFinder::new());
    let plain = TimetableBundleRouter::open(bundle_path.as_ref(), Arc::clone(&time_zone_finder))
        .expect("bundle loads");
    let cost_aware = TimetableBundleRouter::open(bundle_path.as_ref(), time_zone_finder)
        .expect("bundle loads")
        .with_cost_aware_search(true);
    let preferences = TripPreferences::default();

    let queries = std::fs::read_to_string(&queries_path).expect("queries read");
    let responses = std::fs::read_to_string(&responses_path).expect("responses read");
    let mut daytime = Tally::default();
    let mut overnight = Tally::default();
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
        let description = format!("{} to {} on {}", from.name, to.name, time.date_naive());
        for (travel_mode, tally) in [
            (LegTravelMode::Daytime, &mut daytime),
            (
                LegTravelMode::Overnight(OvernightLegOptions::DEFAULT),
                &mut overnight,
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
            let plain_result = plain.plan_leg(&input, || false);
            tally.plain_times.push(started.elapsed());
            let started = Instant::now();
            let cost_aware_result = cost_aware.plan_leg(&input, || false);
            tally.cost_aware_times.push(started.elapsed());
            let (outcome, difference) = compare(&plain_result, &cost_aware_result);
            tally
                .outcomes
                .push((outcome, difference, description.clone()));
        }
    }
    print_summary("Daytime legs", &mut daytime);
    print_summary("Overnight legs", &mut overnight);
}

/// Compares the chosen journeys of two plans. Returns the outcome and how much the cost-aware
/// plan's journey costs less, in minutes.
fn compare(plain: &LegPlanningResult, cost_aware: &LegPlanningResult) -> (Outcome, i64) {
    match (plain, cost_aware) {
        (LegPlanningResult::Found(plain), LegPlanningResult::Found(cost_aware)) => {
            let saving =
                i64::from(plain.best_journey_cost) - i64::from(cost_aware.best_journey_cost);
            let outcome = if same_trains(&plain.chosen_journey, &cost_aware.chosen_journey) {
                Outcome::SameJourney
            } else if saving > 0 {
                Outcome::Cheaper
            } else if saving == 0 {
                Outcome::SameCost
            } else {
                Outcome::Costlier
            };
            (outcome, saving)
        }
        (LegPlanningResult::Found(_), LegPlanningResult::NoJourneyFound(_)) => {
            (Outcome::OnlyPlain, 0)
        }
        (LegPlanningResult::NoJourneyFound(_), LegPlanningResult::Found(_)) => {
            (Outcome::OnlyCostAware, 0)
        }
        _ => (Outcome::Neither, 0),
    }
}

/// Returns whether two journeys ride the same trains at the same times.
fn same_trains(left: &Journey, right: &Journey) -> bool {
    left.rides()
        .map(|ride| {
            (
                &ride.train_label.number,
                ride.departure_time,
                ride.arrival_time,
            )
        })
        .eq(right.rides().map(|ride| {
            (
                &ride.train_label.number,
                ride.departure_time,
                ride.arrival_time,
            )
        }))
}

/// Prints the count per outcome, the queries where the costs differ, and the planning times.
fn print_summary(title: &str, tally: &mut Tally) {
    println!("\n{title}");
    tally
        .outcomes
        .sort_by_key(|(outcome, saving, _)| (*outcome, -saving));
    let mut index = 0;
    while index < tally.outcomes.len() {
        let outcome = tally.outcomes[index].0;
        let count = tally.outcomes[index..]
            .iter()
            .take_while(|(other, _, _)| *other == outcome)
            .count();
        println!("  {outcome:?}: {count}");
        if matches!(
            outcome,
            Outcome::Cheaper | Outcome::Costlier | Outcome::OnlyPlain
        ) {
            for (_, saving, description) in tally.outcomes[index..index + count].iter().take(15) {
                println!("    {saving:>5} min  {description}");
            }
        }
        index += count;
    }
    for (name, times) in [
        ("plain", &mut tally.plain_times),
        ("cost-aware", &mut tally.cost_aware_times),
    ] {
        times.sort_unstable();
        let percentile = |fraction: f64| {
            // Rounding to an index of a short list stays well within `usize`.
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let position = ((times.len().saturating_sub(1)) as f64 * fraction).round() as usize;
            times.get(position).copied().unwrap_or_default()
        };
        let total: Duration = times.iter().sum();
        println!(
            "  {name} planning: median {:?}, 90th percentile {:?}, largest {:?}, total {:?}",
            percentile(0.5),
            percentile(0.9),
            percentile(1.0),
            total
        );
    }
}
