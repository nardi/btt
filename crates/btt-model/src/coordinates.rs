//! Fixed-point geographic coordinates and distances between them.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::bounded::ValueOutOfRangeError;

/// Microdegrees in one degree.
const MICRODEGREES_PER_DEGREE: f64 = 1_000_000.0;

/// Mean Earth radius used for great-circle distances.
const EARTH_RADIUS_METERS: f64 = 6_371_008.8;

/// Defines a coordinate axis stored in millionths of a degree.
///
/// Values serialize as decimal degrees, which is what map libraries and JSON readers expect, and
/// deserialization rounds to the nearest microdegree.
macro_rules! coordinate_axis {
    ($(#[$type_meta:meta])* $type_name:ident, $limit_degrees:expr) => {
        $(#[$type_meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Type)]
        #[specta(type = specta_typescript::Number)]
        pub struct $type_name {
            microdegrees: i32,
        }

        impl $type_name {
            /// Largest absolute value in microdegrees.
            pub const LIMIT_MICRODEGREES: i32 = $limit_degrees * 1_000_000;

            /// Checks the range and wraps a value in microdegrees.
            ///
            /// # Errors
            ///
            /// Returns an error when the value is outside the range of the axis.
            pub const fn from_microdegrees(microdegrees: i32) -> Result<Self, ValueOutOfRangeError> {
                if microdegrees < -Self::LIMIT_MICRODEGREES || microdegrees > Self::LIMIT_MICRODEGREES {
                    return Err(ValueOutOfRangeError {
                        type_name: stringify!($type_name),
                        value: microdegrees as i64,
                        minimum: -(Self::LIMIT_MICRODEGREES as i64),
                        maximum: Self::LIMIT_MICRODEGREES as i64,
                    });
                }
                Ok(Self { microdegrees })
            }

            /// Rounds decimal degrees to the nearest microdegree and checks the range.
            ///
            /// # Errors
            ///
            /// Returns an error when the value is not finite or outside the range of the axis.
            #[allow(clippy::cast_possible_truncation)]
            pub fn from_degrees(degrees: f64) -> Result<Self, ValueOutOfRangeError> {
                let limit = f64::from(Self::LIMIT_MICRODEGREES);
                let microdegrees = (degrees * MICRODEGREES_PER_DEGREE).round();
                if !microdegrees.is_finite() || microdegrees.abs() > limit {
                    return Err(ValueOutOfRangeError {
                        type_name: stringify!($type_name),
                        // Non-finite input is reported as zero, the message names the type and range.
                        value: if microdegrees.is_finite() { microdegrees as i64 } else { 0 },
                        minimum: -i64::from(Self::LIMIT_MICRODEGREES),
                        maximum: i64::from(Self::LIMIT_MICRODEGREES),
                    });
                }
                Ok(Self { microdegrees: microdegrees as i32 })
            }

            /// Returns the value in millionths of a degree.
            pub const fn microdegrees(self) -> i32 {
                self.microdegrees
            }

            /// Returns the value in decimal degrees.
            pub fn degrees(self) -> f64 {
                f64::from(self.microdegrees) / MICRODEGREES_PER_DEGREE
            }
        }

        impl Serialize for $type_name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_f64(self.degrees())
            }
        }

        impl<'de> Deserialize<'de> for $type_name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let degrees = f64::deserialize(deserializer)?;
                Self::from_degrees(degrees).map_err(serde::de::Error::custom)
            }
        }
    };
}

coordinate_axis! {
    /// Latitude from -90 to 90 degrees, stored in millionths of a degree.
    ///
    /// A microdegree of latitude is about 11 cm, far more precise than a place or station needs.
    Latitude, 90
}

coordinate_axis! {
    /// Longitude from -180 to 180 degrees, stored in millionths of a degree.
    Longitude, 180
}

/// A point on the Earth's surface.
///
/// Fixed-point axes make this type `Hash` and `Eq`, so two coordinates that print the same also
/// hash the same. Separate types for the two axes mean a swapped pair does not compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct GeoCoordinates {
    /// North-south position.
    pub latitude: Latitude,
    /// East-west position.
    pub longitude: Longitude,
}

impl GeoCoordinates {
    /// Builds coordinates from decimal degrees.
    ///
    /// # Errors
    ///
    /// Returns an error when either value is not finite or outside its range.
    pub fn from_degrees(latitude: f64, longitude: f64) -> Result<Self, ValueOutOfRangeError> {
        Ok(Self {
            latitude: Latitude::from_degrees(latitude)?,
            longitude: Longitude::from_degrees(longitude)?,
        })
    }

    /// Returns the great-circle distance to another point, using the haversine formula.
    pub fn haversine_distance_meters(self, other: Self) -> f64 {
        let latitude_a = self.latitude.degrees().to_radians();
        let latitude_b = other.latitude.degrees().to_radians();
        let latitude_delta = latitude_b - latitude_a;
        let longitude_delta = (other.longitude.degrees() - self.longitude.degrees()).to_radians();
        let half_chord = (latitude_delta / 2.0).sin().powi(2)
            + latitude_a.cos() * latitude_b.cos() * (longitude_delta / 2.0).sin().powi(2);
        2.0 * EARTH_RADIUS_METERS * half_chord.sqrt().asin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degrees_round_trip_through_microdegrees() {
        let latitude = Latitude::from_degrees(52.378_902_4).unwrap();
        assert_eq!(latitude.microdegrees(), 52_378_902);
        assert!(Latitude::from_degrees(90.5).is_err());
        assert!(Longitude::from_degrees(180.0).is_ok());
        assert!(Longitude::from_degrees(f64::NAN).is_err());
    }

    #[test]
    fn json_uses_decimal_degrees() {
        let amsterdam = GeoCoordinates::from_degrees(52.378_9, 4.900_3).unwrap();
        let json = serde_json::to_string(&amsterdam).unwrap();
        assert_eq!(json, r#"{"latitude":52.3789,"longitude":4.9003}"#);
        let parsed: GeoCoordinates = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, amsterdam);
    }

    #[test]
    fn haversine_distance_matches_known_city_pair() {
        let amsterdam = GeoCoordinates::from_degrees(52.378_9, 4.900_3).unwrap();
        let berlin = GeoCoordinates::from_degrees(52.525_1, 13.369_4).unwrap();
        let distance_kilometers = amsterdam.haversine_distance_meters(berlin) / 1000.0;
        assert!(
            (570.0..580.0).contains(&distance_kilometers),
            "{distance_kilometers}"
        );
    }
}
