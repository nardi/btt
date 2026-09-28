//! Data the search derives from a bundle once, when the bundle loads.

use btt_timetable::{BundleTripIndex, LoadedTimetableBundle, RaptorRouteIndex, ServiceDayBitset};

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
            lower_bounds: LowerBoundGraph::new(bundle),
            service_days_by_route,
        }
    }
}
