//! Writes the dictionary of trains that links share with the web viewer.
//!
//! Run with `cargo run --release -p btt-planner --example train_dictionary -- <bundle>
//! <dictionary file> [train count]`. The dictionary file holds one train per line as its operator,
//! category and number, separated by tabs. The tool keeps the lines the file already has, in
//! their order, because a link refers to a train by its line. It then appends the trains of the
//! bundle that the file lacks, until the file has the train count. Without a train count it
//! appends all of them.
//!
//! A train is the combination of operator, category and number. The trains that a traveller
//! plans a long trip with come first: those outside the regional categories that cover at least
//! `LONG_DISTANCE_KILOMETERS` between their first and last stop. The trains that run on the most
//! days come first within that group, because a link spends fewer bits on an entry early in the
//! file. All other trains follow in the same order.

use std::collections::{HashMap, HashSet};

use btt_planner::REGIONAL_CATEGORIES;
use btt_planner::operators::OperatorTable;
use btt_timetable::{BundleTripIndex, LoadedTimetableBundle, RaptorRouteIndex};

/// Least distance in kilometers between the first and the last stop of a long-distance train.
const LONG_DISTANCE_KILOMETERS: f64 = 120.0;

/// What the tool learns about one train.
#[derive(Default)]
struct TrainFacts {
    /// Whether the category of the train is a regional one.
    regional: bool,
    /// Days on which the train runs, summed over its trips.
    days: u64,
    /// Furthest distance between the first and last stop of one of its routes, in kilometers.
    span_kilometers: f64,
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let bundle_path = arguments.next().expect("pass the bundle");
    let dictionary_path = arguments.next().expect("pass the dictionary file");
    let wanted: usize = arguments.next().map_or(usize::MAX, |count| {
        count.parse().expect("the number of trains is a number")
    });

    let bundle = LoadedTimetableBundle::open(bundle_path.as_ref()).expect("the bundle opens");
    let operators = OperatorTable::bundled();

    let mut facts_by_train: HashMap<String, TrainFacts> = HashMap::new();
    for route_index in 0..bundle.route_count() {
        let route = bundle.route(RaptorRouteIndex(
            u32::try_from(route_index).expect("route indices fit in u32"),
        ));
        if route.trip_count == 0 || route.stop_count == 0 {
            continue;
        }
        let first = bundle.station_coordinates(bundle.pattern_stop(&route, 0).station());
        let last =
            bundle.station_coordinates(bundle.pattern_stop(&route, route.stop_count - 1).station());
        let span_kilometers = first.haversine_distance_meters(last) / 1000.0;
        for trip_offset in 0..route.trip_count {
            let trip = BundleTripIndex(route.first_trip_index + trip_offset);
            let (category, number, agency) = bundle.trip_train_label(trip);
            let line = format!(
                "{}\t{}\t{}",
                operators.operator_of(agency, category),
                category,
                number
            );
            let facts = facts_by_train.entry(line).or_default();
            facts.regional = REGIONAL_CATEGORIES.contains(&category);
            facts.days += bundle
                .trip_service_days(trip)
                .0
                .iter()
                .map(|word| u64::from(word.count_ones()))
                .sum::<u64>();
            facts.span_kilometers = facts.span_kilometers.max(span_kilometers);
        }
    }

    let is_long_distance =
        |facts: &TrainFacts| !facts.regional && facts.span_kilometers >= LONG_DISTANCE_KILOMETERS;
    let mut ranked: Vec<(&String, &TrainFacts)> = facts_by_train.iter().collect();
    ranked.sort_by(|a, b| {
        let key = |(line, facts): &(&String, &TrainFacts)| {
            (
                !is_long_distance(facts),
                std::cmp::Reverse(facts.days),
                (*line).clone(),
            )
        };
        key(a).cmp(&key(b))
    });

    let mut lines: Vec<String> = std::fs::read_to_string(&dictionary_path)
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default();
    let mut known: HashSet<String> = lines.iter().cloned().collect();
    println!(
        "{} trains, {} long-distance, {} lines in the file",
        ranked.len(),
        ranked
            .iter()
            .filter(|(_, facts)| is_long_distance(facts))
            .count(),
        lines.len()
    );
    for (line, _) in ranked {
        if lines.len() >= wanted {
            break;
        }
        if known.insert(line.clone()) {
            lines.push(line.clone());
        }
    }
    std::fs::write(&dictionary_path, lines.join("\n") + "\n").expect("the dictionary is written");
    println!("wrote {} trains", lines.len());
}
