//! Finds the stations a train passes without stopping, so the map can draw its line along the
//! tracks instead of straight across the country.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use btt_model::GeoCoordinates;
use btt_timetable::{BundleStationIndex, LoadedTimetableBundle, RaptorRouteIndex};
use rustc_hash::FxHashMap;

/// Categories of trains that serve a region and not a country. The ranking of stations leaves
/// them out, and a hop of the rail graph that only they run on is not a long-distance hop.
pub const REGIONAL_CATEGORIES: [&str; 22] = [
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

/// Hops shorter than this keep the plain curve between their two stops.
const SHORTEST_SEARCHED_HOP_METERS: u32 = 8_000;

/// Longest edge a path may use at first. Longer edges belong to fast trains that skip the
/// stations the path is after. A high-speed line has few stations along it, so when no path
/// with short edges exists, a second search allows edges of up to half the hop.
const LONGEST_REGIONAL_EDGE_METERS: u32 = 25_000;

/// Longest accepted path, in percent of the straight distance of the hop. Longer paths most
/// likely follow another line, such as an old line next to a high-speed line.
const LONGEST_PATH_PERCENT: u32 = 115;

/// Smallest distance between two points of a path. Station coordinates sit beside the track,
/// so closer points make the drawn line wiggle.
const SHORTEST_POINT_SPACING_METERS: f64 = 2_000.0;

/// Most stations one search settles before it gives up, which bounds the time per hop.
const MAXIMUM_SETTLED_STATIONS: usize = 20_000;

/// An undirected graph with an edge between every two stations that follow each other on any
/// route of a bundle. The bundle holds only rail routes, so the edges follow tracks.
#[derive(Debug)]
pub(crate) struct StationHopGraph {
    /// Coordinates of each station, by station index.
    coordinates: Vec<GeoCoordinates>,
    /// Row `s` of `edges` runs from `row_offsets[s]` to `row_offsets[s + 1]`.
    row_offsets: Vec<u32>,
    /// Neighbouring stations with the straight distance to them in meters.
    edges: Vec<(u32, u32)>,
}

impl StationHopGraph {
    /// Builds the graph from the stop patterns of every route of a bundle.
    pub(crate) fn new(bundle: &LoadedTimetableBundle) -> Self {
        let coordinates = (0..bundle.station_count())
            .map(|station| {
                bundle.station_coordinates(BundleStationIndex(
                    u32::try_from(station).unwrap_or(u32::MAX),
                ))
            })
            .collect();
        let mut hops = Vec::new();
        for route_index in 0..bundle.route_count() {
            let route = bundle.route(RaptorRouteIndex(
                u32::try_from(route_index).unwrap_or(u32::MAX),
            ));
            for position in 1..route.stop_count {
                let from = bundle.pattern_stop(&route, position - 1).station();
                let to = bundle.pattern_stop(&route, position).station();
                hops.push((from.0, to.0));
            }
        }
        Self::from_hops(coordinates, hops)
    }

    /// Builds the graph from station coordinates and pairs of consecutive stops.
    pub(crate) fn from_hops(coordinates: Vec<GeoCoordinates>, hops: Vec<(u32, u32)>) -> Self {
        let distance = |from: u32, to: u32| {
            meters(coordinates[from as usize].haversine_distance_meters(coordinates[to as usize]))
        };
        // Each hop becomes an edge in both directions, sorted by station so the rows are
        // contiguous and duplicates from routes that share the hop sit next to each other.
        let mut directed: Vec<(u32, u32)> = hops
            .into_iter()
            .filter(|&(from, to)| {
                from != to
                    && (from as usize) < coordinates.len()
                    && (to as usize) < coordinates.len()
            })
            .flat_map(|(from, to)| [(from, to), (to, from)])
            .collect();
        directed.sort_unstable();
        directed.dedup();
        let mut row_offsets = vec![0u32; coordinates.len() + 1];
        for &(from, _) in &directed {
            row_offsets[from as usize + 1] += 1;
        }
        for station in 0..coordinates.len() {
            row_offsets[station + 1] += row_offsets[station];
        }
        let edges = directed
            .iter()
            .map(|&(from, to)| (to, distance(from, to)))
            .collect();
        Self {
            coordinates,
            row_offsets,
            edges,
        }
    }

    /// Returns the coordinates of the stations a train most likely passes between two
    /// consecutive stops, in travel order and at least 2 km apart. Returns nothing when the
    /// hop is short or no path over short edges stays close to the straight line.
    pub(crate) fn passed_station_coordinates(
        &self,
        from: BundleStationIndex,
        to: BundleStationIndex,
    ) -> Vec<GeoCoordinates> {
        let (Some(&start), Some(&end)) = (
            self.coordinates.get(from.as_usize()),
            self.coordinates.get(to.as_usize()),
        ) else {
            return Vec::new();
        };
        let hop_meters = meters(start.haversine_distance_meters(end));
        if hop_meters < SHORTEST_SEARCHED_HOP_METERS {
            return Vec::new();
        }
        let half_hop = hop_meters / 2;
        let regional_path = self.shortest_path(
            from.0,
            to.0,
            hop_meters,
            half_hop.min(LONGEST_REGIONAL_EDGE_METERS),
        );
        let path = match regional_path {
            Some(path) => path,
            None if half_hop > LONGEST_REGIONAL_EDGE_METERS => {
                match self.shortest_path(from.0, to.0, hop_meters, half_hop) {
                    Some(path) => path,
                    None => return Vec::new(),
                }
            }
            None => return Vec::new(),
        };
        let mut kept = Vec::new();
        let mut previous = start;
        for &station in &path[1..path.len() - 1] {
            let point = self.coordinates[station as usize];
            if previous.haversine_distance_meters(point) >= SHORTEST_POINT_SPACING_METERS
                && point.haversine_distance_meters(end) >= SHORTEST_POINT_SPACING_METERS
            {
                kept.push(point);
                previous = point;
            }
        }
        kept
    }

    /// Finds the shortest path between two stations with A*, over edges of at most
    /// `longest_edge` meters. Returns the stations of the path including both
    /// ends, or `None` when every path is longer than `LONGEST_PATH_PERCENT` of the hop.
    fn shortest_path(
        &self,
        from: u32,
        to: u32,
        hop_meters: u32,
        longest_edge: u32,
    ) -> Option<Vec<u32>> {
        let longest_path = hop_meters * LONGEST_PATH_PERCENT / 100;
        let end = self.coordinates[to as usize];
        let remaining = |station: u32| {
            meters(self.coordinates[station as usize].haversine_distance_meters(end))
        };
        // Path length so far and the station it came from, by station.
        let mut reached: FxHashMap<u32, (u32, u32)> = FxHashMap::default();
        reached.insert(from, (0, from));
        let mut queue = BinaryHeap::from([Reverse((remaining(from), 0u32, from))]);
        let mut settled = 0;
        while let Some(Reverse((_, length, station))) = queue.pop() {
            if station == to {
                let mut path = vec![to];
                let mut current = to;
                while current != from {
                    current = reached[&current].1;
                    path.push(current);
                }
                path.reverse();
                return Some(path);
            }
            if reached
                .get(&station)
                .is_some_and(|&(best, _)| best < length)
            {
                continue;
            }
            settled += 1;
            if settled > MAXIMUM_SETTLED_STATIONS {
                return None;
            }
            let row = self.row_offsets[station as usize] as usize
                ..self.row_offsets[station as usize + 1] as usize;
            for &(neighbour, edge_meters) in &self.edges[row] {
                if edge_meters > longest_edge {
                    continue;
                }
                let neighbour_length = length + edge_meters;
                // The straight distance to the end is a lower bound, so a station whose path
                // plus that bound exceeds the limit can't be on an accepted path.
                let estimate = neighbour_length + remaining(neighbour);
                if estimate > longest_path
                    || reached
                        .get(&neighbour)
                        .is_some_and(|&(best, _)| best <= neighbour_length)
                {
                    continue;
                }
                reached.insert(neighbour, (neighbour_length, station));
                queue.push(Reverse((estimate, neighbour_length, neighbour)));
            }
        }
        None
    }
}

/// Rounds a distance to whole meters. Distances between stations stay far below `u32::MAX`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn meters(distance_meters: f64) -> u32 {
    distance_meters.round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stations along a line east of 50° N, 7° E, at the given kilometers.
    fn stations_at_kilometers(kilometers: &[f64]) -> Vec<GeoCoordinates> {
        // One degree of longitude at 50° N is about 71.7 km.
        kilometers
            .iter()
            .map(|km| GeoCoordinates::from_degrees(50.0, 7.0 + km / 71.7).unwrap())
            .collect()
    }

    /// A graph, a hop and the stations the hop passes. The web viewer runs the same search, and
    /// reads these cases from `ui/viewer/src/fixtures/hop-paths.json`.
    struct HopCase {
        name: &'static str,
        coordinates: Vec<GeoCoordinates>,
        hops: Vec<(u32, u32)>,
        from: u32,
        to: u32,
        passed: Vec<GeoCoordinates>,
    }

    /// Returns the cases of the tests below.
    fn hop_cases() -> Vec<HopCase> {
        let regional = stations_at_kilometers(&[0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0]);
        let fast = stations_at_kilometers(&[0.0, 30.0, 60.0, 90.0]);
        let mut detour = stations_at_kilometers(&[0.0, 15.0, 30.0]);
        detour[1] = GeoCoordinates::from_degrees(50.18, 7.0 + 15.0 / 71.7).unwrap();
        let long_edge = stations_at_kilometers(&[0.0, 20.0, 30.0]);
        vec![
            // A regional line stops every 5 km from station 0 to station 6, and a fast train runs
            // from station 0 to station 6 directly. Points closer than 2 km to the one before are
            // dropped by the caller, which keeps all of these.
            HopCase {
                name: "a fast hop passes the stations of the regional line",
                hops: vec![(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 6), (0, 6)],
                from: 0,
                to: 6,
                passed: regional[1..6].to_vec(),
                coordinates: regional,
            },
            // Only fast trains run here, stopping every 30 km, which is above the regional limit
            // but below half of the 90 km hop.
            HopCase {
                name: "a high-speed hop passes the stations of other fast trains",
                hops: vec![(0, 1), (1, 2), (2, 3), (0, 3)],
                from: 0,
                to: 3,
                passed: fast[1..3].to_vec(),
                coordinates: fast,
            },
            // The only other way from 0 to 2 goes 20 km north, far more than 15% longer.
            HopCase {
                name: "a path with a long detour is rejected",
                hops: vec![(0, 1), (1, 2), (0, 2)],
                from: 0,
                to: 2,
                passed: Vec::new(),
                coordinates: detour,
            },
            // Station 1 lies on the straight line, but its edge from station 0 is 20 km long,
            // which is more than half of the 30 km hop.
            HopCase {
                name: "edges longer than half the hop are skipped",
                hops: vec![(0, 1), (1, 2), (0, 2)],
                from: 0,
                to: 2,
                passed: Vec::new(),
                coordinates: long_edge,
            },
        ]
    }

    #[test]
    fn the_cases_of_the_hop_search_give_their_stations() {
        for case in hop_cases() {
            let graph = StationHopGraph::from_hops(case.coordinates.clone(), case.hops.clone());
            let passed = graph.passed_station_coordinates(
                BundleStationIndex(case.from),
                BundleStationIndex(case.to),
            );
            assert_eq!(passed, case.passed, "{}", case.name);
        }
    }

    /// Checks the cases that the web viewer reads against the ones above, or writes them when
    /// `UPDATE_VIEWER_FIXTURES` is set.
    #[test]
    fn the_hop_cases_of_the_viewer_are_current() {
        let degrees = |points: &[GeoCoordinates]| -> Vec<[f64; 2]> {
            points
                .iter()
                .map(|point| [point.latitude.degrees(), point.longitude.degrees()])
                .collect()
        };
        let fresh = serde_json::Value::Array(
            hop_cases()
                .iter()
                .map(|case| {
                    serde_json::json!({
                        "name": case.name,
                        "coordinates": degrees(&case.coordinates),
                        "hops": case.hops,
                        "from": case.from,
                        "to": case.to,
                        "passed": degrees(&case.passed),
                    })
                })
                .collect(),
        );
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../ui/viewer/src/fixtures/hop-paths.json");
        if std::env::var_os("UPDATE_VIEWER_FIXTURES").is_some() {
            std::fs::write(&path, serde_json::to_string_pretty(&fresh).unwrap() + "\n").unwrap();
            return;
        }
        let committed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_default())
                .unwrap_or_default();
        assert_eq!(
            committed, fresh,
            "ui/viewer/src/fixtures/hop-paths.json is out of date. Run `UPDATE_VIEWER_FIXTURES=1 cargo test -p btt-planner the_hop_cases`."
        );
    }
}
