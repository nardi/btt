//! Looks up the time zone of a position and converts local times to UTC.

use btt_model::{GeoCoordinates, UnixTimeMinutes};
use chrono::{LocalResult, NaiveDate, NaiveDateTime, TimeDelta, TimeZone};
use chrono_tz::Tz;

/// Returns the UTC moment of a local time on a date in a time zone.
///
/// `minutes_after_local_midnight` may pass 1,440 to reach into the next day, which window ends
/// that cross midnight need. A local time that daylight saving time skips maps to the moment
/// the clocks jump forward. A local time that repeats when the clocks go back maps to the
/// earlier of the two moments.
pub fn local_time_to_unix_minutes(
    date: NaiveDate,
    minutes_after_local_midnight: u32,
    time_zone: Tz,
) -> UnixTimeMinutes {
    let local =
        NaiveDateTime::from(date) + TimeDelta::minutes(i64::from(minutes_after_local_midnight));
    // Daylight saving gaps last at most an hour, so stepping forward in minutes ends quickly.
    let utc_seconds = (0..=120)
        .find_map(|shift_minutes| {
            match time_zone.from_local_datetime(&(local + TimeDelta::minutes(shift_minutes))) {
                LocalResult::Single(moment) | LocalResult::Ambiguous(moment, _) => {
                    Some(moment.timestamp())
                }
                LocalResult::None => None,
            }
        })
        .unwrap_or_else(|| local.and_utc().timestamp());
    let utc_minutes = utc_seconds.div_euclid(60);
    UnixTimeMinutes(u32::try_from(utc_minutes).unwrap_or(if utc_minutes < 0 {
        0
    } else {
        u32::MAX
    }))
}

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

    fn date(month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, month, day).unwrap()
    }

    fn utc_minutes(month: u32, day: u32, hour: u32, minute: u32) -> UnixTimeMinutes {
        let moment = date(month, day)
            .and_hms_opt(hour, minute, 0)
            .unwrap()
            .and_utc();
        UnixTimeMinutes(u32::try_from(moment.timestamp() / 60).unwrap())
    }

    #[test]
    fn converts_summer_time_and_times_past_midnight() {
        let amsterdam = Tz::Europe__Amsterdam;
        assert_eq!(
            local_time_to_unix_minutes(date(10, 1), 8 * 60, amsterdam),
            utc_minutes(10, 1, 6, 0)
        );
        assert_eq!(
            local_time_to_unix_minutes(date(10, 1), 25 * 60, amsterdam),
            utc_minutes(10, 1, 23, 0)
        );
    }

    #[test]
    fn skipped_local_time_maps_to_the_jump() {
        let skipped = local_time_to_unix_minutes(date(3, 29), 2 * 60 + 30, Tz::Europe__Amsterdam);
        assert_eq!(skipped, utc_minutes(3, 29, 1, 0));
    }

    #[test]
    fn repeated_local_time_maps_to_the_earlier_moment() {
        let repeated = local_time_to_unix_minutes(date(10, 25), 2 * 60 + 30, Tz::Europe__Amsterdam);
        assert_eq!(repeated, utc_minutes(10, 25, 0, 30));
    }
}
