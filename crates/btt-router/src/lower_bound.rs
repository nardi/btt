//! Lower bounds on the travel time from every station to the arrival place, used to prune labels
//! that can no longer beat the best arrival found so far.
//!
//! The graph follows nigiri, the routing core of MOTIS. Its edges are the fastest ride between
//! two consecutive stops of any route and every footpath. Transfer times are left out, so a
//! bound never exceeds the real remaining time.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use btt_timetable::{BundleStationIndex, LoadedTimetableBundle};

use crate::search::CandidateStation;

/// Value of a station that cannot reach the arrival place.
pub(crate) const UNREACHABLE: u32 = u32::MAX;

/// Edges of the lower bound graph, stored by the station they lead to, so a search can run
/// backwards from the arrival place, and by the station they start at, so one can run forwards
/// from the departure place.
#[derive(Debug)]
pub(crate) struct LowerBoundGraph {
    /// Row `s` of `incoming_edges` runs from `row_offsets[s]` to `row_offsets[s + 1]`.
    row_offsets: Vec<u32>,
    /// Stations with an edge into the row's station, with the fewest minutes of that edge.
    incoming_edges: Vec<(BundleStationIndex, u16)>,
    /// Row `s` of `outgoing_edges` runs from `outgoing_offsets[s]` to `outgoing_offsets[s + 1]`.
    outgoing_offsets: Vec<u32>,
    /// Stations with an edge from the row's station, with the fewest minutes of that edge.
    outgoing_edges: Vec<(BundleStationIndex, u16)>,
}

impl LowerBoundGraph {
    /// Builds the graph from every route and footpath of a bundle. This reads every stop event
    /// once, so it belongs with loading a bundle, not with a query.
    pub(crate) fn new(bundle: &LoadedTimetableBundle) -> Self {
        // (to station, from station, minutes), with duplicates that the sort below removes.
        let mut edges: Vec<(u32, u32, u16)> = Vec::new();
        for route_index in 0..bundle.route_count() {
            let raptor_route =
                btt_timetable::RaptorRouteIndex(u32::try_from(route_index).unwrap_or(u32::MAX));
            let route = bundle.route(raptor_route);
            for position in 1..route.stop_count {
                let fastest = (0..route.trip_count)
                    .map(|trip_offset| {
                        let departure = bundle
                            .stop_event(&route, trip_offset, position - 1)
                            .departure
                            .0;
                        let arrival = bundle.stop_event(&route, trip_offset, position).arrival.0;
                        arrival.saturating_sub(departure)
                    })
                    .min()
                    .unwrap_or(u16::MAX);
                let from = bundle.pattern_stop(&route, position - 1).station();
                let to = bundle.pattern_stop(&route, position).station();
                if from != to {
                    edges.push((to.0, from.0, fastest));
                }
            }
        }
        for station_index in 0..bundle.station_count() {
            let from = BundleStationIndex(u32::try_from(station_index).unwrap_or(u32::MAX));
            for footpath in bundle.footpaths_from(from) {
                edges.push((footpath.to_station.0, from.0, footpath.duration_minutes));
            }
        }
        // Sorting by station pair and then minutes puts the fastest edge of each pair first.
        edges.sort_unstable();
        edges.dedup_by_key(|&mut (to, from, _)| (to, from));
        let station_count = bundle.station_count();
        let (row_offsets, incoming_edges) = rows(station_count, &edges);
        let mut reversed: Vec<(u32, u32, u16)> = edges
            .iter()
            .map(|&(to, from, minutes)| (from, to, minutes))
            .collect();
        reversed.sort_unstable();
        let (outgoing_offsets, outgoing_edges) = rows(station_count, &reversed);
        Self {
            row_offsets,
            incoming_edges,
            outgoing_offsets,
            outgoing_edges,
        }
    }

    /// Returns the edges into a station.
    pub(crate) fn incoming(&self, station: usize) -> &[(BundleStationIndex, u16)] {
        let start = self.row_offsets[station] as usize;
        let end = self.row_offsets[station + 1] as usize;
        &self.incoming_edges[start..end]
    }

    /// Returns the edges out of a station.
    fn outgoing(&self, station: usize) -> &[(BundleStationIndex, u16)] {
        let start = self.outgoing_offsets[station] as usize;
        let end = self.outgoing_offsets[station + 1] as usize;
        &self.outgoing_edges[start..end]
    }

    /// Fills `bounds` with the fewest minutes from each station to the arrival place, starting
    /// from the arrival candidates and their times to the place. Stations that cannot reach the
    /// place get `UNREACHABLE`.
    pub(crate) fn compute_bounds(
        &self,
        arrival_candidates: &[CandidateStation],
        bounds: &mut Vec<u32>,
        queue: &mut BinaryHeap<Reverse<(u32, u32)>>,
    ) {
        shortest_minutes(
            self.row_offsets.len() - 1,
            arrival_candidates,
            u32::MAX,
            |station| self.incoming(station),
            bounds,
            queue,
        );
    }

    /// Fills `bounds` with the fewest minutes from the departure place to each station, starting
    /// from the departure candidates and their times from the place. Stations that take more
    /// than `limit` minutes, or that the place can't reach, get `UNREACHABLE`.
    pub(crate) fn compute_bounds_from(
        &self,
        departure_candidates: &[CandidateStation],
        limit: u32,
        bounds: &mut Vec<u32>,
        queue: &mut BinaryHeap<Reverse<(u32, u32)>>,
    ) {
        shortest_minutes(
            self.row_offsets.len() - 1,
            departure_candidates,
            limit,
            |station| self.outgoing(station),
            bounds,
            queue,
        );
    }
}

/// Puts the edges, sorted by their first station, into rows by that station. Returns the
/// offsets of the rows, with one more entry for the end, and the edges as the other station and
/// their minutes.
fn rows(
    station_count: usize,
    edges: &[(u32, u32, u16)],
) -> (Vec<u32>, Vec<(BundleStationIndex, u16)>) {
    let mut offsets = Vec::with_capacity(station_count + 1);
    let mut row_edges = Vec::with_capacity(edges.len());
    let mut next_edge = edges.iter().peekable();
    for station in 0..station_count {
        offsets.push(u32::try_from(row_edges.len()).unwrap_or(u32::MAX));
        while let Some(&&(first, other, minutes)) = next_edge.peek() {
            if first as usize != station {
                break;
            }
            row_edges.push((BundleStationIndex(other), minutes));
            next_edge.next();
        }
    }
    offsets.push(u32::try_from(row_edges.len()).unwrap_or(u32::MAX));
    (offsets, row_edges)
}

/// Runs Dijkstra's algorithm from the candidates, with their minutes as the start, along the
/// edges that `neighbours` returns. Fills `minutes` with the fewest minutes to each station, and
/// `UNREACHABLE` for stations past `limit` or out of reach.
fn shortest_minutes<'graph>(
    station_count: usize,
    candidates: &[CandidateStation],
    limit: u32,
    neighbours: impl Fn(usize) -> &'graph [(BundleStationIndex, u16)],
    minutes: &mut Vec<u32>,
    queue: &mut BinaryHeap<Reverse<(u32, u32)>>,
) {
    minutes.clear();
    minutes.resize(station_count, UNREACHABLE);
    queue.clear();
    for candidate in candidates {
        let index = candidate.station.as_usize();
        let start = u32::from(candidate.time_to_place_minutes);
        if start <= limit && start < minutes[index] {
            minutes[index] = start;
            queue.push(Reverse((start, candidate.station.0)));
        }
    }
    while let Some(Reverse((reached, station))) = queue.pop() {
        let station = station as usize;
        if reached > minutes[station] {
            continue;
        }
        for &(other, edge_minutes) in neighbours(station) {
            let through = reached + u32::from(edge_minutes);
            let slot = &mut minutes[other.as_usize()];
            if through <= limit && through < *slot {
                *slot = through;
                queue.push(Reverse((through, other.0)));
            }
        }
    }
}
