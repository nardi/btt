//! Data the search derives from a bundle once, when the bundle loads.

use btt_timetable::{
    BundleStationIndex, BundleTripIndex, Footpath, LoadedTimetableBundle, RaptorRouteIndex,
    ServiceDayBitset,
};

use std::sync::OnceLock;

use crate::lower_bound::LowerBoundGraph;
use crate::ways::{FootpathMode, footpath_minutes};

/// Everything a search needs besides the bundle itself. Build it once per bundle and share it
/// between queries.
#[derive(Debug)]
pub struct SearchIndex {
    /// Lower bounds on travel times, which let the search skip hopeless stations. A wheelchair
    /// is never faster than a walk, so wheelchair searches use these bounds too.
    pub(crate) lower_bounds: LowerBoundGraph,
    /// Lower bounds for searches by bike, built when the first one runs.
    cycle_lower_bounds: OnceLock<LowerBoundGraph>,
    /// Days on which any trip of each route runs, so trip lookups can skip the other days
    /// without looking at single trips, as nigiri does.
    pub(crate) service_days_by_route: Vec<ServiceDayBitset>,
    /// The footpaths of the bundle, which are walks.
    walk_footpaths: FootpathTable,
    /// The footpaths by bike, built when the first search by bike runs.
    cycle_footpaths: OnceLock<FootpathTable>,
    /// The footpaths in a wheelchair, built when the first search in a wheelchair runs.
    wheel_footpaths: OnceLock<FootpathTable>,
}

/// The footpaths between stations for one way of moving, in two orders.
#[derive(Debug)]
pub(crate) struct FootpathTable {
    /// The footpaths of every station, shortest first, so a walk from a station can stop at the
    /// first footpath that is too long. Row `s` runs from `footpath_offsets[s]` to
    /// `footpath_offsets[s + 1]`.
    footpaths_shortest_first: Vec<Footpath>,
    /// Where each station's row starts in `footpaths_shortest_first`, with one more entry for
    /// the end.
    footpath_offsets: Vec<u32>,
    /// The footpaths that end at every station, as the station they start at and their minutes,
    /// for searches that run backwards from the arrival place.
    incoming_footpaths: Vec<(BundleStationIndex, u16)>,
    /// Where each station's row starts in `incoming_footpaths`, with one more entry for the end.
    incoming_offsets: Vec<u32>,
}

impl FootpathTable {
    /// Builds the table from the station pairs of the bundle footpaths. The minutes of each
    /// pair are the bundle minutes for a walk, and else follow from the distance between the
    /// two stations.
    fn new(bundle: &LoadedTimetableBundle, mode: FootpathMode) -> Self {
        let station_count = bundle.station_count();
        let mut footpaths_shortest_first = Vec::new();
        let mut footpath_offsets = Vec::with_capacity(station_count + 1);
        // (to station, from station, minutes) of every footpath, for the incoming rows.
        let mut incoming: Vec<(u32, u32, u16)> = Vec::new();
        for station_index in 0..station_count {
            let station = BundleStationIndex(u32::try_from(station_index).unwrap_or(u32::MAX));
            footpath_offsets
                .push(u32::try_from(footpaths_shortest_first.len()).unwrap_or(u32::MAX));
            let mut row: Vec<Footpath> = bundle
                .footpaths_from(station)
                .map(|footpath| Footpath {
                    duration_minutes: mode_minutes(bundle, mode, station, footpath),
                    ..footpath
                })
                .collect();
            row.sort_by_key(|footpath| footpath.duration_minutes);
            for footpath in &row {
                incoming.push((footpath.to_station.0, station.0, footpath.duration_minutes));
            }
            footpaths_shortest_first.extend(row);
        }
        footpath_offsets.push(u32::try_from(footpaths_shortest_first.len()).unwrap_or(u32::MAX));
        incoming.sort_unstable();
        let mut incoming_footpaths = Vec::with_capacity(incoming.len());
        let mut incoming_offsets = Vec::with_capacity(station_count + 1);
        let mut next = incoming.iter().peekable();
        for station_index in 0..station_count {
            incoming_offsets.push(u32::try_from(incoming_footpaths.len()).unwrap_or(u32::MAX));
            while let Some(&&(to, from, minutes)) = next.peek() {
                if to as usize != station_index {
                    break;
                }
                incoming_footpaths.push((BundleStationIndex(from), minutes));
                next.next();
            }
        }
        incoming_offsets.push(u32::try_from(incoming_footpaths.len()).unwrap_or(u32::MAX));
        Self {
            footpaths_shortest_first,
            footpath_offsets,
            incoming_footpaths,
            incoming_offsets,
        }
    }

    /// Returns the footpaths that start at a station, shortest first.
    pub(crate) fn shortest_first(&self, station: BundleStationIndex) -> &[Footpath] {
        let start = self.footpath_offsets[station.as_usize()] as usize;
        let end = self.footpath_offsets[station.as_usize() + 1] as usize;
        &self.footpaths_shortest_first[start..end]
    }

    /// Returns the footpaths that end at a station, as the station they start at and their
    /// minutes.
    pub(crate) fn incoming(&self, station: BundleStationIndex) -> &[(BundleStationIndex, u16)] {
        let start = self.incoming_offsets[station.as_usize()] as usize;
        let end = self.incoming_offsets[station.as_usize() + 1] as usize;
        &self.incoming_footpaths[start..end]
    }
}

/// Returns the minutes of a bundle footpath for a way of moving.
pub(crate) fn mode_minutes(
    bundle: &LoadedTimetableBundle,
    mode: FootpathMode,
    from: BundleStationIndex,
    footpath: Footpath,
) -> u16 {
    if mode == FootpathMode::Walk {
        return footpath.duration_minutes;
    }
    let distance = bundle
        .station_coordinates(from)
        .haversine_distance_meters(bundle.station_coordinates(footpath.to_station));
    footpath_minutes(mode, distance)
}

impl SearchIndex {
    /// Builds the index of a bundle. This reads every stop event once.
    pub fn new(bundle: &LoadedTimetableBundle) -> Self {
        let service_days_by_route =
            (0..bundle.route_count())
                .map(|route_index| {
                    let route = bundle.route(RaptorRouteIndex(
                        u32::try_from(route_index).unwrap_or(u32::MAX),
                    ));
                    (0..route.trip_count).fold(ServiceDayBitset::default(), |days, trip_offset| {
                        days.union(bundle.trip_service_days(BundleTripIndex(
                            route.first_trip_index + trip_offset,
                        )))
                    })
                })
                .collect();
        Self {
            lower_bounds: LowerBoundGraph::new(bundle, FootpathMode::Walk),
            cycle_lower_bounds: OnceLock::new(),
            service_days_by_route,
            walk_footpaths: FootpathTable::new(bundle, FootpathMode::Walk),
            cycle_footpaths: OnceLock::new(),
            wheel_footpaths: OnceLock::new(),
        }
    }

    /// Returns the footpath table of a way of moving, and builds it on first use.
    pub(crate) fn footpaths(
        &self,
        bundle: &LoadedTimetableBundle,
        mode: FootpathMode,
    ) -> &FootpathTable {
        match mode {
            FootpathMode::Walk => &self.walk_footpaths,
            FootpathMode::Cycle => self
                .cycle_footpaths
                .get_or_init(|| FootpathTable::new(bundle, mode)),
            FootpathMode::Wheel => self
                .wheel_footpaths
                .get_or_init(|| FootpathTable::new(bundle, mode)),
        }
    }

    /// Returns the lower bound graph of a way of moving, and builds it on first use.
    pub(crate) fn lower_bounds_for(
        &self,
        bundle: &LoadedTimetableBundle,
        mode: FootpathMode,
    ) -> &LowerBoundGraph {
        match mode {
            FootpathMode::Walk | FootpathMode::Wheel => &self.lower_bounds,
            FootpathMode::Cycle => self
                .cycle_lower_bounds
                .get_or_init(|| LowerBoundGraph::new(bundle, mode)),
        }
    }
}
