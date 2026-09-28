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
/// backwards from the arrival place.
#[derive(Debug)]
pub(crate) struct LowerBoundGraph {
    /// Row `s` of `incoming_edges` runs from `row_offsets[s]` to `row_offsets[s + 1]`.
    row_offsets: Vec<u32>,
    /// Stations with an edge into the row's station, with the fewest minutes of that edge.
    incoming_edges: Vec<(BundleStationIndex, u16)>,
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
        let mut row_offsets = Vec::with_capacity(station_count + 1);
        let mut incoming_edges = Vec::with_capacity(edges.len());
        let mut next_edge = edges.iter().peekable();
        for station in 0..station_count {
            row_offsets.push(u32::try_from(incoming_edges.len()).unwrap_or(u32::MAX));
            while let Some(&&(to, from, minutes)) = next_edge.peek() {
                if to as usize != station {
                    break;
                }
                incoming_edges.push((BundleStationIndex(from), minutes));
                next_edge.next();
            }
        }
        row_offsets.push(u32::try_from(incoming_edges.len()).unwrap_or(u32::MAX));
        Self {
            row_offsets,
            incoming_edges,
        }
    }

    /// Returns the edges into a station.
    fn incoming(&self, station: usize) -> &[(BundleStationIndex, u16)] {
        let start = self.row_offsets[station] as usize;
        let end = self.row_offsets[station + 1] as usize;
        &self.incoming_edges[start..end]
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
        let station_count = self.row_offsets.len() - 1;
        bounds.clear();
        bounds.resize(station_count, UNREACHABLE);
        queue.clear();
        for candidate in arrival_candidates {
            let index = candidate.station.as_usize();
            let minutes = u32::from(candidate.time_to_place_minutes);
            if minutes < bounds[index] {
                bounds[index] = minutes;
                queue.push(Reverse((minutes, candidate.station.0)));
            }
        }
        while let Some(Reverse((minutes, station))) = queue.pop() {
            let station = station as usize;
            if minutes > bounds[station] {
                continue;
            }
            for &(from, edge_minutes) in self.incoming(station) {
                let through = minutes + u32::from(edge_minutes);
                let slot = &mut bounds[from.as_usize()];
                if through < *slot {
                    *slot = through;
                    queue.push(Reverse((through, from.0)));
                }
            }
        }
    }
}
