//! Data the search derives from a bundle once, when the bundle loads.

use btt_timetable::{
    BundleStationIndex, BundleTripIndex, Footpath, LoadedTimetableBundle, RaptorRouteIndex,
    ServiceDayBitset,
};

use crate::lower_bound::LowerBoundGraph;

/// Everything a search needs besides the bundle itself. Build it once per bundle and share it
/// between queries.
#[derive(Debug)]
pub struct SearchIndex {
    /// Lower bounds on travel times, which let the search skip hopeless stations.
    pub(crate) lower_bounds: LowerBoundGraph,
    /// Days on which any trip of each route runs, so trip lookups can skip the other days
    /// without looking at single trips, as nigiri does.
    pub(crate) service_days_by_route: Vec<ServiceDayBitset>,
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
        let station_count = bundle.station_count();
        let mut footpaths_shortest_first = Vec::new();
        let mut footpath_offsets = Vec::with_capacity(station_count + 1);
        // (to station, from station, minutes) of every footpath, for the incoming rows.
        let mut incoming: Vec<(u32, u32, u16)> = Vec::new();
        for station_index in 0..station_count {
            let station = BundleStationIndex(u32::try_from(station_index).unwrap_or(u32::MAX));
            footpath_offsets
                .push(u32::try_from(footpaths_shortest_first.len()).unwrap_or(u32::MAX));
            let mut row: Vec<Footpath> = bundle.footpaths_from(station).collect();
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
            lower_bounds: LowerBoundGraph::new(bundle),
            service_days_by_route,
            footpaths_shortest_first,
            footpath_offsets,
            incoming_footpaths,
            incoming_offsets,
        }
    }

    /// Returns the footpaths that start at a station, shortest first.
    pub(crate) fn footpaths_shortest_first(&self, station: BundleStationIndex) -> &[Footpath] {
        let start = self.footpath_offsets[station.as_usize()] as usize;
        let end = self.footpath_offsets[station.as_usize() + 1] as usize;
        &self.footpaths_shortest_first[start..end]
    }

    /// Returns the footpaths that end at a station, as the station they start at and their
    /// minutes.
    pub(crate) fn incoming_footpaths(
        &self,
        station: BundleStationIndex,
    ) -> &[(BundleStationIndex, u16)] {
        let start = self.incoming_offsets[station.as_usize()] as usize;
        let end = self.incoming_offsets[station.as_usize() + 1] as usize;
        &self.incoming_footpaths[start..end]
    }
}
