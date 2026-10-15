//! Writes the dictionary of stations that links and the web viewer share.
//!
//! Run with `cargo run --release -p btt-planner --example station_dictionary -- <bundle>
//! <dictionary file> <station count>`. The dictionary file holds one station per line as its
//! name, latitude and longitude in microdegrees and time zone, separated by tabs. The tool keeps
//! the lines the file already has, in their order, because a link refers to a station by its
//! line. It then appends the busiest stations of the bundle that the file lacks, until the file
//! has the station count.
//!
//! The busiest stations are the ones the most trains of long-distance categories stop at. Local
//! trains would otherwise rank every suburban station above a border station.

use std::collections::HashSet;

use btt_timetable::{BundleTripIndex, LoadedTimetableBundle, RaptorRouteIndex};

/// Categories of trains that serve a region and not a country, which the ranking leaves out.
const REGIONAL_CATEGORIES: [&str; 22] = [
    "S",
    "R",
    "RB",
    "RE",
    "REX",
    "IRE",
    "TER",
    "TE",
    "Os",
    "L",
    "U",
    "X",
    "Bus",
    "IC Bus",
    "Zug",
    "Train",
    "Regionalzug",
    "RegioExpress",
    "Tog",
    "Regionaltog",
    "STUB",
    "FLX",
];

fn main() {
    let mut arguments = std::env::args().skip(1);
    let bundle_path = arguments.next().expect("pass the bundle");
    let dictionary_path = arguments.next().expect("pass the dictionary file");
    let wanted: usize = arguments
        .next()
        .expect("pass the number of stations")
        .parse()
        .expect("the number of stations is a number");

    let bundle = LoadedTimetableBundle::open(bundle_path.as_ref()).expect("the bundle opens");
    let station_count = bundle.station_count();

    // Count the long-distance trips that stop at each station.
    let mut trips_at_station = vec![0_u32; station_count];
    for route_index in 0..bundle.route_count() {
        let route = bundle.route(RaptorRouteIndex(
            u32::try_from(route_index).expect("route indices fit in u32"),
        ));
        if route.trip_count == 0 {
            continue;
        }
        let (category, _, _) = bundle.trip_train_label(BundleTripIndex(route.first_trip_index));
        if REGIONAL_CATEGORIES.contains(&category) {
            continue;
        }
        for position in 0..route.stop_count {
            let stop = bundle.pattern_stop(&route, position);
            if stop.boarding_allowed() || stop.alighting_allowed() {
                trips_at_station[stop.station().as_usize()] += route.trip_count;
            }
        }
    }

    let mut lines: Vec<String> = std::fs::read_to_string(&dictionary_path)
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default();
    let known: HashSet<String> = lines.iter().cloned().collect();
    let mut ranked: Vec<usize> = (0..station_count)
        .filter(|index| trips_at_station[*index] > 0)
        .collect();
    ranked.sort_by_key(|index| std::cmp::Reverse(trips_at_station[*index]));
    println!(
        "{} stations, {} with long-distance trains, {} lines in the file",
        station_count,
        ranked.len(),
        lines.len()
    );
    let mut known = known;
    for index in ranked {
        if lines.len() >= wanted {
            break;
        }
        let station = btt_timetable::BundleStationIndex(
            u32::try_from(index).expect("station indices fit in u32"),
        );
        let coordinates = bundle.station_coordinates(station);
        let line = format!(
            "{}\t{}\t{}\t{}",
            bundle.station_name(station),
            coordinates.latitude.microdegrees(),
            coordinates.longitude.microdegrees(),
            bundle.station_time_zone(station)
        );
        if known.insert(line.clone()) {
            lines.push(line);
        }
    }
    std::fs::write(&dictionary_path, lines.join("\n") + "\n").expect("the dictionary is written");
    println!("wrote {} stations", lines.len());
}
