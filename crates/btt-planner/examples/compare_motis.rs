//! Runs MOTIS's benchmark queries on our router and compares the earliest arrivals and the
//! search times.
//!
//! Generate queries and responses with MOTIS first, on the same rail feeds as the bundle:
//! `motis generate`, then `motis batch -q queries.txt -r responses.txt`. Then run
//! `cargo run --release -p btt-planner --example compare_motis -- <bundle> <queries.txt> <responses.txt>`.
//!
//! Both sides start at a stop and end at a stop, so ways to and from stations don't count. Our
//! side uses the bundle station nearest to each MOTIS stop, and the same transfer time and
//! number of trains as MOTIS's defaults.

use std::num::NonZeroU8;
use std::time::{Duration, Instant};

use btt_model::GeoCoordinates;
use btt_router::{
    AllowEveryRide, CandidateStation, MinutesSinceBundleStart, RaptorLegQuery, RaptorSearchBuffers,
    SearchIndex, search_leg_journeys,
};
use btt_timetable::{LoadedTimetableBundle, StationIndex};
use chrono::{DateTime, NaiveTime, Utc};
use serde::Deserialize;

/// MOTIS's default time for a transfer at one stop.
const MOTIS_TRANSFER_MINUTES: u16 = 2;

/// Largest number of trains on our side. MOTIS finds journeys with more than ten transfers by
/// default, so the limit sits above that.
const MOTIS_MAXIMUM_TRAIN_RIDES: NonZeroU8 = NonZeroU8::new(16).unwrap();

/// Largest distance from a MOTIS stop to the bundle station that stands for it.
const LARGEST_STATION_DISTANCE_METERS: f64 = 300.0;

/// Distance within which our side may walk from a MOTIS stop to another station.
const WALKING_DISTANCE_METERS: f64 = 1000.0;

/// Departure window of the timed search, which matches MOTIS's default search window.
const TIMED_WINDOW_MINUTES: u32 = 120;

/// The parts of a MOTIS plan response the comparison reads.
#[derive(Deserialize)]
struct MotisResponse {
    #[serde(rename = "debugOutput")]
    debug_output: Option<MotisDebugOutput>,
    from: Option<MotisPlace>,
    to: Option<MotisPlace>,
    #[serde(default)]
    itineraries: Vec<MotisItinerary>,
}

#[derive(Deserialize)]
struct MotisDebugOutput {
    execute_time: Option<u64>,
}

#[derive(Deserialize)]
struct MotisPlace {
    name: String,
    lat: f64,
    lon: f64,
}

#[derive(Deserialize)]
struct MotisItinerary {
    #[serde(rename = "startTime")]
    start_time: DateTime<Utc>,
    #[serde(rename = "endTime")]
    end_time: DateTime<Utc>,
}

/// How our earliest arrival compares with MOTIS's for one query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Outcome {
    Equal,
    OursEarlier,
    OursLater,
    OnlyOurs,
    OnlyMotis,
    Neither,
    NoNearbyStation,
}

// The steps of one comparison share the bundle, the indexes and the buffers, which reads
// best as one function.
#[allow(clippy::too_many_lines)]
fn main() {
    let mut arguments = std::env::args().skip(1);
    let bundle_path = arguments.next().expect("pass the bundle");
    let queries_path = arguments.next().expect("pass queries.txt");
    let responses_path = arguments.next().expect("pass responses.txt");
    // Queries whose description contains this text get both sides printed in full.
    let shown = arguments.next();

    let bundle = LoadedTimetableBundle::open(bundle_path.as_ref()).expect("bundle loads");
    let search_index = SearchIndex::new(&bundle);
    let station_index = StationIndex::new(&bundle);
    let bundle_start = bundle.validity_start().and_time(NaiveTime::MIN).and_utc();
    let minutes_since_start =
        |moment: DateTime<Utc>| u32::try_from((moment - bundle_start).num_minutes()).unwrap_or(0);
    // Every station within walking distance of a stop, with the planner's walking estimate,
    // because MOTIS also walks from its stop to nearby stops. None when no station lies
    // within `LARGEST_STATION_DISTANCE_METERS`, which means the stop is missing from the bundle.
    let nearby_stations = |place: &MotisPlace| -> Option<Vec<CandidateStation>> {
        let coordinates = GeoCoordinates::from_degrees(place.lat, place.lon).ok()?;
        let nearby = station_index.stations_within(&bundle, coordinates, WALKING_DISTANCE_METERS);
        if nearby
            .first()
            .is_none_or(|nearest| nearest.distance_meters > LARGEST_STATION_DISTANCE_METERS)
        {
            return None;
        }
        Some(
            nearby
                .iter()
                .enumerate()
                .map(|(position, station)| CandidateStation {
                    station: station.station,
                    // The nearest station stands for the stop itself.
                    time_to_place_minutes: if position == 0 {
                        0
                    } else {
                        walking_minutes(station.distance_meters)
                    },
                })
                .collect(),
        )
    };
    let mut buffers = RaptorSearchBuffers::new();
    let mut search = |from: &[CandidateStation], to: &[CandidateStation], start: u32, end: u32| {
        let query = RaptorLegQuery {
            departure_candidates: from,
            arrival_candidates: to,
            departure_window: MinutesSinceBundleStart(start)..MinutesSinceBundleStart(end),
            minimum_transfer_minutes: MOTIS_TRANSFER_MINUTES,
            maximum_train_rides: MOTIS_MAXIMUM_TRAIN_RIDES,
        };
        let started = Instant::now();
        let journeys = search_leg_journeys(
            &bundle,
            &search_index,
            &query,
            &AllowEveryRide,
            &mut buffers,
            || false,
        );
        (journeys, started.elapsed())
    };

    let queries = std::fs::read_to_string(&queries_path).expect("queries read");
    let responses = std::fs::read_to_string(&responses_path).expect("responses read");
    let mut outcomes: Vec<(Outcome, i64, String)> = Vec::new();
    let mut our_times: Vec<Duration> = Vec::new();
    let mut motis_times: Vec<u64> = Vec::new();
    for (query, response) in queries.lines().zip(responses.lines()) {
        let Some(time) = query
            .split(['?', '&'])
            .find_map(|part| part.strip_prefix("time="))
            .and_then(|time| time.parse::<DateTime<Utc>>().ok())
        else {
            continue;
        };
        let raw_response = response;
        let Ok(response) = serde_json::from_str::<MotisResponse>(response) else {
            continue;
        };
        motis_times.extend(response.debug_output.and_then(|debug| debug.execute_time));
        let (Some(from_place), Some(to_place)) = (&response.from, &response.to) else {
            continue;
        };
        let description = format!("{} to {} at {time}", from_place.name, to_place.name);
        let (Some(from), Some(to)) = (nearby_stations(from_place), nearby_stations(to_place))
        else {
            outcomes.push((Outcome::NoNearbyStation, 0, description));
            continue;
        };
        let start = minutes_since_start(time);
        // MOTIS's earliest arrival, with the departure of that itinerary.
        let motis_earliest = response
            .itineraries
            .iter()
            .filter(|itinerary| itinerary.start_time >= time)
            .map(|itinerary| {
                (
                    minutes_since_start(itinerary.end_time),
                    minutes_since_start(itinerary.start_time),
                )
            })
            .min();
        let motis_arrival = motis_earliest.map(|(arrival, _)| arrival);

        let (_, timed) = search(&from, &to, start, start + TIMED_WINDOW_MINUTES);
        our_times.push(timed);

        // The comparison window reaches past the departure of MOTIS's earliest arrival, which
        // MOTIS finds by extending its interval, and a day when MOTIS found nothing.
        let end = motis_earliest.map_or(start + 1440, |(_, departure)| {
            departure.max(start + TIMED_WINDOW_MINUTES) + 60
        });
        let (journeys, _) = search(&from, &to, start, end);
        let our_arrival = journeys
            .journeys
            .iter()
            .filter(|journey| journey.departure.0 >= start)
            .map(|journey| journey.arrival.0)
            .min();
        let (outcome, difference) = compare_arrivals(our_arrival, motis_arrival);
        if shown
            .as_deref()
            .is_some_and(|text| description.contains(text))
        {
            print_both_sides(
                &bundle,
                &description,
                &journeys.journeys,
                raw_response,
                bundle_start,
            );
        }
        outcomes.push((outcome, difference, description));
    }

    print_summary(&outcomes, &mut our_times, &mut motis_times);
}

/// Prints how often each outcome occurred, the largest differences and the search times.
fn print_summary(
    outcomes: &[(Outcome, i64, String)],
    our_times: &mut [Duration],
    motis_times: &mut [u64],
) {
    println!("{} queries", outcomes.len());
    let mut counts = std::collections::BTreeMap::new();
    for (outcome, _, _) in outcomes {
        *counts.entry(*outcome).or_insert(0) += 1;
    }
    for (outcome, count) in &counts {
        println!("  {outcome:?}: {count}");
    }
    for outcome in [
        Outcome::OursLater,
        Outcome::OursEarlier,
        Outcome::OnlyMotis,
        Outcome::OnlyOurs,
    ] {
        let mut examples: Vec<_> = outcomes
            .iter()
            .filter(|(kind, ..)| *kind == outcome)
            .collect();
        examples.sort_by_key(|(_, difference, _)| -difference.abs());
        if !examples.is_empty() {
            println!("\n{outcome:?}, largest differences in minutes:");
            for (_, difference, description) in examples.iter().take(8) {
                println!("  {difference:>6}  {description}");
            }
        }
    }

    our_times.sort_unstable();
    motis_times.sort_unstable();
    let percentile = |sorted_count: usize, percent: usize| {
        (sorted_count * percent / 100).min(sorted_count.saturating_sub(1))
    };
    if !our_times.is_empty() && !motis_times.is_empty() {
        println!(
            "\nSearch time over a {TIMED_WINDOW_MINUTES} minute window, ours against MOTIS's execute_time:"
        );
        for percent in [50, 90, 99] {
            println!(
                "  p{percent}: {:>7.1} ms  {:>5} ms",
                our_times[percentile(our_times.len(), percent)].as_secs_f64() * 1000.0,
                motis_times[percentile(motis_times.len(), percent)]
            );
        }
    }
}

/// Returns how our earliest arrival compares with MOTIS's, and by how many minutes ours is
/// later, which is negative when ours is earlier.
fn compare_arrivals(ours: Option<u32>, motis: Option<u32>) -> (Outcome, i64) {
    match (ours, motis) {
        (Some(ours), Some(motis)) => {
            let difference = i64::from(ours) - i64::from(motis);
            let outcome = match difference {
                0 => Outcome::Equal,
                ..0 => Outcome::OursEarlier,
                _ => Outcome::OursLater,
            };
            (outcome, difference)
        }
        (Some(_), None) => (Outcome::OnlyOurs, 0),
        (None, Some(_)) => (Outcome::OnlyMotis, 0),
        (None, None) => (Outcome::Neither, 0),
    }
}

/// Prints our journeys and MOTIS's itineraries for one query, with trains and UTC times.
fn print_both_sides(
    bundle: &LoadedTimetableBundle,
    description: &str,
    journeys: &[btt_router::RaptorJourney],
    raw_response: &str,
    bundle_start: DateTime<Utc>,
) {
    let moment = |minutes: u32| {
        (bundle_start + chrono::Duration::minutes(i64::from(minutes)))
            .format("%d %H:%M")
            .to_string()
    };
    println!("\n{description}\n  ours:");
    for journey in journeys {
        let rides: Vec<String> = journey
            .rides
            .iter()
            .map(|ride| {
                let (category, number, _) = bundle.trip_train_label(ride.trip);
                format!(
                    "{category} {number} {} {} to {} {}",
                    bundle.station_name(ride.boarding_station),
                    moment(ride.departure.0),
                    bundle.station_name(ride.alighting_station),
                    moment(ride.arrival.0)
                )
            })
            .collect();
        println!("    {}", rides.join(" | "));
    }
    println!("  MOTIS:");
    let response: serde_json::Value = serde_json::from_str(raw_response).unwrap_or_default();
    for itinerary in response["itineraries"].as_array().into_iter().flatten() {
        let legs: Vec<String> = itinerary["legs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|leg| {
                let name = leg["displayName"]
                    .as_str()
                    .or(leg["routeShortName"].as_str())
                    .unwrap_or_else(|| leg["mode"].as_str().unwrap_or("?"));
                format!(
                    "{name} {} {} to {} {}",
                    leg["from"]["name"].as_str().unwrap_or("?"),
                    leg["startTime"].as_str().unwrap_or("?"),
                    leg["to"]["name"].as_str().unwrap_or("?"),
                    leg["endTime"].as_str().unwrap_or("?")
                )
            })
            .collect();
        println!("    {}", legs.join(" | "));
    }
}

/// Estimates the minutes of a walk, with the planner's formula for short distances.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn walking_minutes(distance_meters: f64) -> u16 {
    // Walks are at most `WALKING_DISTANCE_METERS`, so the minutes fit easily.
    (distance_meters * 1.3 / 83.3).ceil() as u16
}
