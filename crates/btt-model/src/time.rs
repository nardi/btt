//! Durations, times of day and daily time windows.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::bounded::bounded_integer;

/// Minutes in one day.
pub const MINUTES_PER_DAY: u16 = 1440;

/// A length of time in whole minutes, up to 65,535, which is about 45 days.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Type,
)]
#[serde(transparent)]
pub struct DurationMinutes(pub u16);

impl DurationMinutes {
    /// A duration of zero minutes.
    pub const ZERO: Self = Self(0);

    /// Builds a duration from whole hours and minutes, saturating at the largest value.
    pub const fn from_hours_and_minutes(hours: u16, minutes: u16) -> Self {
        Self(hours.saturating_mul(60).saturating_add(minutes))
    }

    /// Returns the number of minutes.
    pub const fn minutes(self) -> u16 {
        self.0
    }

    /// Adds two durations, saturating at the largest value.
    #[must_use]
    pub const fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

bounded_integer! {
    /// A time of day on the local clock of a place, from 00:00 to 23:59.
    LocalTimeOfDay { minutes_since_midnight: u16 } in 0, MINUTES_PER_DAY - 1
}

impl LocalTimeOfDay {
    /// 00:00 on the local clock.
    pub const MIDNIGHT: Self = Self {
        minutes_since_midnight: 0,
    };

    /// Builds a time of day from an hour and a minute.
    ///
    /// # Errors
    ///
    /// Returns an error when the hour is above 23 or the minute above 59.
    pub const fn from_hour_and_minute(
        hour: u16,
        minute: u16,
    ) -> Result<Self, crate::ValueOutOfRangeError> {
        if minute >= 60 {
            return Err(crate::ValueOutOfRangeError {
                type_name: "minute of LocalTimeOfDay",
                value: minute as i64,
                minimum: 0,
                maximum: 59,
            });
        }
        Self::new(hour.saturating_mul(60).saturating_add(minute))
    }
}

bounded_integer! {
    /// The length of a daily time window, from one minute to a whole day.
    DailyWindowLength { minutes: u16 } in 1, MINUTES_PER_DAY
}

/// A time window that recurs on every travel day, such as 08:00 to 12:00.
///
/// The window crosses midnight when `start` plus `length` passes 24:00. A start and a length make
/// that explicit, where a start and an end would make a window from 22:00 to 01:00 look like an
/// invalid one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct DailyTimeWindow {
    /// Local time at which the window opens.
    pub start: LocalTimeOfDay,
    /// How long the window stays open.
    pub length: DailyWindowLength,
}

impl DailyTimeWindow {
    /// Builds a window from its local start and end times. An end before the start means the
    /// window crosses midnight, and an end equal to the start means the whole day.
    pub fn from_start_and_end(start: LocalTimeOfDay, end: LocalTimeOfDay) -> Self {
        let start_minutes = start.minutes_since_midnight();
        let end_minutes = end.minutes_since_midnight();
        let length_minutes = if end_minutes > start_minutes {
            end_minutes - start_minutes
        } else {
            MINUTES_PER_DAY - start_minutes + end_minutes
        };
        // Both branches give a length between 1 and 1,440, so the range check is not needed.
        let length = DailyWindowLength {
            minutes: length_minutes,
        };
        Self { start, length }
    }

    /// Returns the minute after the start of the local day on which the window opens at which it
    /// closes. It is above 1,440 when the window crosses midnight.
    pub const fn end_minutes_after_opening_day_start(self) -> u16 {
        self.start.minutes_since_midnight() + self.length.minutes()
    }

    /// Returns whether the window crosses local midnight.
    pub const fn crosses_midnight(self) -> bool {
        self.end_minutes_after_opening_day_start() > MINUTES_PER_DAY
    }
}

/// Local time windows for departing and arriving on travel days.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct TravelDayWindows {
    /// Window in which the first train of a leg departs.
    pub departure: DailyTimeWindow,
    /// Window in which the last train of a leg arrives.
    pub arrival: DailyTimeWindow,
}

/// Minutes since the Unix epoch, 1970-01-01 00:00 UTC.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Type,
)]
#[serde(transparent)]
pub struct UnixTimeMinutes(pub u32);

#[cfg(test)]
mod tests {
    use super::*;

    fn time(hour: u16, minute: u16) -> LocalTimeOfDay {
        LocalTimeOfDay::from_hour_and_minute(hour, minute).unwrap()
    }

    #[test]
    fn local_time_of_day_rejects_values_past_midnight() {
        assert!(LocalTimeOfDay::new(1439).is_ok());
        assert!(LocalTimeOfDay::new(1440).is_err());
        assert!(LocalTimeOfDay::from_hour_and_minute(24, 0).is_err());
        assert!(LocalTimeOfDay::from_hour_and_minute(12, 60).is_err());
    }

    #[test]
    fn window_from_start_and_end_handles_midnight() {
        let evening = DailyTimeWindow::from_start_and_end(time(22, 0), time(1, 0));
        assert_eq!(evening.length.minutes(), 180);
        assert!(evening.crosses_midnight());

        let morning = DailyTimeWindow::from_start_and_end(time(8, 0), time(12, 0));
        assert_eq!(morning.length.minutes(), 240);
        assert!(!morning.crosses_midnight());

        let whole_day = DailyTimeWindow::from_start_and_end(time(5, 0), time(5, 0));
        assert_eq!(whole_day.length.minutes(), MINUTES_PER_DAY);
    }

    #[test]
    fn bounded_values_reject_out_of_range_json() {
        assert!(serde_json::from_str::<LocalTimeOfDay>("1500").is_err());
        assert!(serde_json::from_str::<DailyWindowLength>("0").is_err());
        let parsed: LocalTimeOfDay = serde_json::from_str("480").unwrap();
        assert_eq!(parsed, time(8, 0));
        assert_eq!(serde_json::to_string(&parsed).unwrap(), "480");
    }
}
