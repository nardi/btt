//! Minutes of the ways between a place and a station, or between two stations, per way of
//! moving.
//!
//! The walk formula is the one the builder uses for the footpaths of a bundle. The search keeps
//! a footpath table per way of moving, and builds the tables for cycling and for a wheelchair
//! from the station coordinates when a search first needs them.

/// How the traveller moves outside the trains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FootpathMode {
    /// On foot, with local transport for longer distances.
    #[default]
    Walk,
    /// By bike at any distance.
    Cycle,
    /// In a wheelchair, with local transport for longer distances.
    Wheel,
}

/// Longest distance that a traveller walks or wheels. Beyond it, the estimate is local
/// transport.
pub const LONGEST_WALK_METERS: f64 = 1000.0;

/// Factor from the straight line distance to the distance along the streets.
const DETOUR_FACTOR: f64 = 1.3;

/// Walking speed in meters per minute, which is 5 km/h.
const WALK_METERS_PER_MINUTE: f64 = 83.3;

/// Cycling speed in meters per minute, which is 15 km/h.
const CYCLE_METERS_PER_MINUTE: f64 = 250.0;

/// Minutes to unlock and lock a bike and to take it to and from the platform.
const CYCLE_HANDLING_MINUTES: f64 = 2.0;

/// Speed of a wheelchair user in meters per minute, which is 3.5 km/h.
const WHEEL_METERS_PER_MINUTE: f64 = 58.0;

/// Fixed minutes of a ride on local transport, such as the wait for a tram.
const LOCAL_TRANSPORT_MINUTES: f64 = 15.0;

/// Speed of local transport in meters per minute, which is 15 km/h.
const LOCAL_TRANSPORT_METERS_PER_MINUTE: f64 = 250.0;

/// Extra minutes for a wheelchair user on local transport, for lifts and ramps.
const WHEEL_LOCAL_TRANSPORT_EXTRA_MINUTES: f64 = 5.0;

/// Returns whether a way of a distance uses local transport. A bike never does.
pub fn uses_local_transport(mode: FootpathMode, distance_meters: f64) -> bool {
    mode != FootpathMode::Cycle && distance_meters >= LONGEST_WALK_METERS
}

/// Returns the minutes of a way over a straight line distance.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn way_minutes(mode: FootpathMode, distance_meters: f64) -> u16 {
    let local_transport =
        LOCAL_TRANSPORT_MINUTES + distance_meters / LOCAL_TRANSPORT_METERS_PER_MINUTE;
    let minutes = match mode {
        FootpathMode::Walk if distance_meters < LONGEST_WALK_METERS => {
            distance_meters * DETOUR_FACTOR / WALK_METERS_PER_MINUTE
        }
        FootpathMode::Walk => local_transport,
        FootpathMode::Cycle => {
            CYCLE_HANDLING_MINUTES + distance_meters * DETOUR_FACTOR / CYCLE_METERS_PER_MINUTE
        }
        FootpathMode::Wheel if distance_meters < LONGEST_WALK_METERS => {
            distance_meters * DETOUR_FACTOR / WHEEL_METERS_PER_MINUTE
        }
        FootpathMode::Wheel => local_transport + WHEEL_LOCAL_TRANSPORT_EXTRA_MINUTES,
    };
    // The search radius of a few tens of kilometers keeps the minutes far below `u16::MAX`.
    minutes.ceil() as u16
}

/// Returns the minutes of a footpath between two stations. A footpath takes at least one
/// minute, as in the builder.
pub fn footpath_minutes(mode: FootpathMode, distance_meters: f64) -> u16 {
    way_minutes(mode, distance_meters).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_walk_matches_the_builder_formula() {
        assert_eq!(way_minutes(FootpathMode::Walk, 500.0), 8);
        assert_eq!(way_minutes(FootpathMode::Walk, 2000.0), 23);
    }

    #[test]
    fn a_bike_cycles_at_every_distance() {
        assert_eq!(way_minutes(FootpathMode::Cycle, 500.0), 5);
        assert_eq!(way_minutes(FootpathMode::Cycle, 5000.0), 28);
        assert!(!uses_local_transport(FootpathMode::Cycle, 5000.0));
    }

    #[test]
    fn a_wheelchair_is_never_faster_than_a_walk() {
        for distance in [0.0, 100.0, 999.0, 1000.0, 3000.0, 20_000.0] {
            assert!(
                way_minutes(FootpathMode::Wheel, distance)
                    >= way_minutes(FootpathMode::Walk, distance)
            );
        }
        assert_eq!(way_minutes(FootpathMode::Wheel, 500.0), 12);
        assert_eq!(way_minutes(FootpathMode::Wheel, 2000.0), 28);
    }
}
