//! Looks up the time zone of a position.

use btt_model::GeoCoordinates;
use chrono_tz::Tz;

/// Finds IANA time zones from coordinates with boundary data built into the binary.
///
/// Building the finder parses the boundary data, so the app builds one and shares it.
pub struct TimeZoneFinder {
    /// Polygon index over the time zone boundaries.
    finder: tzf_rs::DefaultFinder,
}

impl TimeZoneFinder {
    /// Parses the built-in boundary data.
    pub fn new() -> Self {
        Self {
            finder: tzf_rs::DefaultFinder::new(),
        }
    }

    /// Returns the time zone at a position.
    ///
    /// Positions at sea or on a boundary the data does not cover fall back to UTC, which only
    /// happens for places no train reaches.
    pub fn time_zone_at(&self, coordinates: GeoCoordinates) -> Tz {
        let name = self.finder.get_tz_name(
            coordinates.longitude.degrees(),
            coordinates.latitude.degrees(),
        );
        name.parse().unwrap_or(Tz::UTC)
    }
}

impl Default for TimeZoneFinder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_zones_of_european_cities() {
        let finder = TimeZoneFinder::new();
        let at = |latitude, longitude| {
            finder.time_zone_at(GeoCoordinates::from_degrees(latitude, longitude).unwrap())
        };
        assert_eq!(at(52.378_9, 4.900_3), Tz::Europe__Amsterdam);
        assert_eq!(at(48.185_2, 16.376_1), Tz::Europe__Vienna);
        assert_eq!(at(51.531_2, -0.126_2), Tz::Europe__London);
        assert_eq!(at(37.975_4, 23.720_9), Tz::Europe__Athens);
    }
}
