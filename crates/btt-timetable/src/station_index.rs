//! Finds the stations near a place.

use btt_model::GeoCoordinates;
use rstar::primitives::GeomWithData;
use rstar::{AABB, RTree};
use smallvec::SmallVec;

use crate::format::BundleStationIndex;
use crate::loaded::LoadedTimetableBundle;

/// Meters in one degree of latitude, used to turn a radius into a search box.
const METERS_PER_DEGREE_OF_LATITUDE: f64 = 111_320.0;

/// A station position as longitude and latitude in degrees, with its bundle index.
type StationPoint = GeomWithData<[f64; 2], BundleStationIndex>;

/// A station near a place, with its straight-line distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NearbyStation {
    /// The station.
    pub station: BundleStationIndex,
    /// Great-circle distance from the place, in meters.
    pub distance_meters: f64,
}

/// An R-tree over all station positions of a bundle. Built at load time, not stored.
pub struct StationIndex {
    /// Station positions.
    tree: RTree<StationPoint>,
}

impl StationIndex {
    /// Builds the index over every station of a bundle.
    pub fn new(bundle: &LoadedTimetableBundle) -> Self {
        let points = (0..bundle.station_count())
            .filter_map(|index| u32::try_from(index).ok())
            .map(|index| {
                let station = BundleStationIndex(index);
                let coordinates = bundle.station_coordinates(station);
                StationPoint::new(
                    [
                        coordinates.longitude.degrees(),
                        coordinates.latitude.degrees(),
                    ],
                    station,
                )
            })
            .collect();
        Self {
            tree: RTree::bulk_load(points),
        }
    }

    /// Returns the stations within a radius of a place, closest first.
    ///
    /// The tree works in degrees, so the search uses a box that surely contains the circle and
    /// then filters by great-circle distance.
    pub fn stations_within(
        &self,
        bundle: &LoadedTimetableBundle,
        center: GeoCoordinates,
        radius_meters: f64,
    ) -> SmallVec<[NearbyStation; 16]> {
        let latitude = center.latitude.degrees();
        let longitude = center.longitude.degrees();
        let latitude_margin = radius_meters / METERS_PER_DEGREE_OF_LATITUDE;
        // A degree of longitude shrinks with the cosine of the latitude. The cosine is bounded
        // away from zero, because no railway runs within a few degrees of a pole.
        let longitude_margin = latitude_margin / latitude.to_radians().cos().max(0.05);
        let search_box = AABB::from_corners(
            [longitude - longitude_margin, latitude - latitude_margin],
            [longitude + longitude_margin, latitude + latitude_margin],
        );
        let mut nearby: SmallVec<[NearbyStation; 16]> = self
            .tree
            .locate_in_envelope(search_box)
            .map(|point| NearbyStation {
                station: point.data,
                distance_meters: center
                    .haversine_distance_meters(bundle.station_coordinates(point.data)),
            })
            .filter(|nearby| nearby.distance_meters <= radius_meters)
            .collect();
        nearby.sort_by(|left, right| left.distance_meters.total_cmp(&right.distance_meters));
        nearby
    }
}
