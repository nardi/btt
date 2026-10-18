//! The list of downloadable bundles that the timetable workflow publishes.
//!
//! The workflow writes the list as `timetable.json` and the app reads it to find a newer bundle.
//! Both sides use these types, so a change to a field shows up in the compiler on both.

use chrono::{Days, NaiveDate};
use serde::{Deserialize, Serialize};

/// The oldest app version that reads bundles of `BUNDLE_FORMAT_VERSION`.
///
/// Raise it together with `BUNDLE_FORMAT_VERSION` to the version of the release that first
/// reads the new layout. It does not follow the version of the builder, because a new app
/// release without a new layout should not force anyone to update.
pub const MINIMUM_APP_VERSION: &str = "0.1.0";

/// Returns the last day of a validity window, which counts its first day.
pub fn last_valid_day(start: NaiveDate, days: u16) -> NaiveDate {
    start
        .checked_add_days(Days::new(u64::from(days.saturating_sub(1))))
        .unwrap_or(start)
}

/// One bundle that can be downloaded.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    /// Version of the data as `yyyymmddNN`, the same as `BundleHeader::bundle_version`.
    pub version: u32,
    /// Layout version of the bundle.
    pub format_version: u32,
    /// First day the bundle covers.
    pub valid_from: NaiveDate,
    /// Last day the bundle covers, inclusive.
    pub valid_until: NaiveDate,
    /// Address of the compressed bundle.
    pub url: String,
    /// Size of the compressed file in bytes.
    pub size_bytes: u64,
    /// SHA-256 of the compressed file as lowercase hexadecimal.
    pub sha256: String,
    /// The oldest app version that can read the bundle.
    pub minimum_app_version: String,
}

/// Every bundle that is still valid, as published in `timetable.json`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct TimetableManifest {
    /// The downloadable bundles, in no particular order.
    pub bundles: Vec<ManifestEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_valid_day_counts_the_first_day() {
        let start = NaiveDate::from_ymd_opt(2026, 10, 11).unwrap();
        assert_eq!(last_valid_day(start, 1), start);
        assert_eq!(
            last_valid_day(start, 90),
            NaiveDate::from_ymd_opt(2027, 1, 8).unwrap()
        );
    }

    #[test]
    fn an_entry_serializes_with_iso_dates() {
        let entry = ManifestEntry {
            version: 2_026_101_101,
            format_version: 2,
            valid_from: NaiveDate::from_ymd_opt(2026, 10, 11).unwrap(),
            valid_until: NaiveDate::from_ymd_opt(2027, 1, 8).unwrap(),
            url: "https://example.org/timetable.bundle.zst".to_owned(),
            size_bytes: 54_000_000,
            sha256: "ab".repeat(32),
            minimum_app_version: MINIMUM_APP_VERSION.to_owned(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains(r#""valid_from":"2026-10-11""#));
        assert!(json.contains(r#""valid_until":"2027-01-08""#));
        assert_eq!(serde_json::from_str::<ManifestEntry>(&json).unwrap(), entry);
    }
}
