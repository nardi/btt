//! Writes the rail graph that the web viewer uses to draw the line of a ride along the tracks.
//!
//! Run with `cargo run --release -p btt-planner --example rail_graph -- <bundle> <graph file>`.
//! A snapshot holds only the stations a ride starts and ends at, and the viewer finds the
//! stations in between the way the desktop app does, with a graph of the stations that follow
//! each other on a route. The file holds that graph in a compact binary form:
//!
//! - one byte for the version of the format, which is 2,
//! - the categories of trains that serve a region, as a count and then a length and the UTF-8
//!   bytes of each,
//! - the categories of all trains that run on an edge, most common first, written the same way,
//! - the stations, as a count and then for each station the difference in latitude and in
//!   longitude to the one before, in steps of 100 microdegrees as zigzag numbers. The stations
//!   follow a Hilbert curve, so neighbours in the file are neighbours on the map and the
//!   differences are small,
//! - the edges, as a count and then for each edge the difference between its first station and
//!   the first station of the edge before, the difference between its two stations, the number
//!   of categories of trains that run on the edge, and the position of each in the list of
//!   categories.
//!
//! The edges are sorted by their first station, which is the one with the lower number. Every
//! number is a base-128 number with the low group first. The file holds no names, so nothing in a
//! link refers to it, and it can be written again from any bundle.

use std::collections::{BTreeMap, BTreeSet};

use btt_planner::REGIONAL_CATEGORIES;
use btt_timetable::{BundleStationIndex, BundleTripIndex, LoadedTimetableBundle, RaptorRouteIndex};

/// Size of the steps that positions are written in, in microdegrees: about 11 meters.
const STEP_MICRODEGREES: i64 = 100;

/// Side of the grid that the Hilbert curve runs over, as a power of two. The longitude has 3.6
/// million steps, which fit in 2^22.
const HILBERT_GRID: u64 = 1 << 22;

/// Returns a position in microdegrees in steps, rounded to the nearest step.
fn steps_of(microdegrees: i32) -> i64 {
    (i64::from(microdegrees) + STEP_MICRODEGREES / 2).div_euclid(STEP_MICRODEGREES)
}

/// Returns the place of a point on the Hilbert curve over the grid.
fn hilbert_key(mut x: u64, mut y: u64) -> u64 {
    let mut key = 0;
    let mut half = HILBERT_GRID / 2;
    while half > 0 {
        let right = u64::from(x & half > 0);
        let up = u64::from(y & half > 0);
        key += half * half * ((3 * right) ^ up);
        // Turn the quadrant so that the curve enters it the same way in every one.
        if up == 0 {
            if right == 1 {
                x = HILBERT_GRID - 1 - x;
                y = HILBERT_GRID - 1 - y;
            }
            std::mem::swap(&mut x, &mut y);
        }
        half /= 2;
    }
    key
}

/// Writes a number as groups of 7 bits, the low group first, with a high bit that says another
/// group follows.
fn push_number(bytes: &mut Vec<u8>, mut value: u64) {
    loop {
        let group = u8::try_from(value & 0x7f).unwrap_or(0);
        value >>= 7;
        if value == 0 {
            bytes.push(group);
            return;
        }
        bytes.push(group | 0x80);
    }
}

/// Writes a signed number so that small values of either sign are short.
fn push_signed_number(bytes: &mut Vec<u8>, value: i64) {
    push_number(bytes, ((value << 1) ^ (value >> 63)).cast_unsigned());
}

/// The edges of a bundle: every pair of consecutive stops of a route, with the categories of the
/// trains that run on it. A pair is written with its lower station number first.
type Edges = BTreeMap<(u32, u32), BTreeSet<String>>;

/// Reads the edges from the stop patterns of the routes of a bundle.
fn read_edges(bundle: &LoadedTimetableBundle) -> Edges {
    let mut edges = Edges::new();
    for route_index in 0..bundle.route_count() {
        let route = bundle.route(RaptorRouteIndex(
            u32::try_from(route_index).expect("route indices fit in u32"),
        ));
        if route.trip_count == 0 {
            continue;
        }
        let (category, _, _) = bundle.trip_train_label(BundleTripIndex(route.first_trip_index));
        for position in 1..route.stop_count {
            let before = bundle.pattern_stop(&route, position - 1).station().0;
            let after = bundle.pattern_stop(&route, position).station().0;
            if before != after {
                edges
                    .entry((before.min(after), before.max(after)))
                    .or_default()
                    .insert(category.to_owned());
            }
        }
    }
    edges
}

/// Returns the position of a station of the bundle in steps of latitude and longitude.
fn position_of(bundle: &LoadedTimetableBundle, station: u32) -> (i64, i64) {
    let coordinates = bundle.station_coordinates(BundleStationIndex(station));
    (
        steps_of(coordinates.latitude.microdegrees()),
        steps_of(coordinates.longitude.microdegrees()),
    )
}

/// Returns the stations that an edge touches, in the order of the Hilbert curve.
fn stations_in_curve_order(bundle: &LoadedTimetableBundle, edges: &Edges) -> Vec<u32> {
    let mut stations: Vec<u32> = edges
        .keys()
        .flat_map(|&(first, second)| [first, second])
        .collect();
    stations.sort_unstable();
    stations.dedup();
    stations.sort_by_key(|&station| {
        let (latitude, longitude) = position_of(bundle, station);
        let x = u64::try_from(latitude + 900_000).unwrap_or(0);
        let y = u64::try_from(longitude + 1_800_000).unwrap_or(0);
        (hilbert_key(x, y), station)
    });
    stations
}

/// Returns the categories of the edges, the most common first, so that the common ones take one
/// byte in the file.
fn categories_by_use(edges: &Edges) -> Vec<&str> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for categories in edges.values() {
        for category in categories {
            *counts.entry(category).or_default() += 1;
        }
    }
    let mut categories: Vec<(&str, usize)> = counts.into_iter().collect();
    categories.sort_by_key(|&(name, count)| (std::cmp::Reverse(count), name));
    categories.into_iter().map(|(name, _)| name).collect()
}

/// Writes a list of texts as a count and then each text as a length and its bytes.
fn push_texts<'a>(bytes: &mut Vec<u8>, texts: impl ExactSizeIterator<Item = &'a str>) {
    push_number(bytes, texts.len() as u64);
    for text in texts {
        push_number(bytes, text.len() as u64);
        bytes.extend_from_slice(text.as_bytes());
    }
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let bundle_path = arguments.next().expect("pass the bundle");
    let graph_path = arguments.next().expect("pass the graph file");
    let bundle = LoadedTimetableBundle::open(bundle_path.as_ref()).expect("the bundle opens");

    let edges = read_edges(&bundle);
    let stations = stations_in_curve_order(&bundle, &edges);
    let node_of: BTreeMap<u32, u32> = stations
        .iter()
        .enumerate()
        .map(|(node, &station)| (station, u32::try_from(node).expect("nodes fit in u32")))
        .collect();
    let categories = categories_by_use(&edges);
    let category_position: BTreeMap<&str, usize> = categories
        .iter()
        .enumerate()
        .map(|(position, &name)| (name, position))
        .collect();

    let mut bytes = vec![2_u8];
    push_texts(&mut bytes, REGIONAL_CATEGORIES.iter().copied());
    push_texts(&mut bytes, categories.iter().copied());
    push_number(&mut bytes, stations.len() as u64);
    let (mut previous_latitude, mut previous_longitude) = (0, 0);
    for &station in &stations {
        let (latitude, longitude) = position_of(&bundle, station);
        push_signed_number(&mut bytes, latitude - previous_latitude);
        push_signed_number(&mut bytes, longitude - previous_longitude);
        (previous_latitude, previous_longitude) = (latitude, longitude);
    }
    let mut node_edges: Vec<(u32, u32, Vec<usize>)> = edges
        .iter()
        .map(|(&(first, second), edge_categories)| {
            let (first, second) = (node_of[&first], node_of[&second]);
            let mut positions: Vec<usize> = edge_categories
                .iter()
                .map(|category| category_position[category.as_str()])
                .collect();
            positions.sort_unstable();
            (first.min(second), first.max(second), positions)
        })
        .collect();
    node_edges.sort_unstable();
    push_number(&mut bytes, node_edges.len() as u64);
    let mut previous_first = 0;
    for (first, second, positions) in &node_edges {
        push_number(&mut bytes, u64::from(first - previous_first));
        push_number(&mut bytes, u64::from(second - first));
        push_number(&mut bytes, positions.len() as u64);
        for &position in positions {
            push_number(&mut bytes, position as u64);
        }
        previous_first = *first;
    }
    std::fs::write(&graph_path, &bytes).expect("the graph is written");
    println!(
        "{} stations, {} edges, {} categories, {} bytes",
        stations.len(),
        node_edges.len(),
        categories.len(),
        bytes.len()
    );
}
