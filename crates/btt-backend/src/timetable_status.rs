//! What the app tells the traveller about its timetable and the ones on the server.

use btt_planner::{JourneyPricer, TimetableBundleRouter};
use btt_timetable::format::BUNDLE_FORMAT_VERSION;
use btt_timetable::manifest::{ManifestEntry, TimetableManifest, last_valid_day};
use chrono::NaiveDate;
use semver::Version;
use serde::Serialize;
use specta::Type;

/// Days of validity left below which the app warns that the timetable runs out.
const WARNING_DAYS_LEFT: i32 = 14;

/// Facts about the bundle the planner loaded at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledBundle {
    /// Version of the data as `yyyymmddNN`.
    pub version: u32,
    /// First day the bundle covers.
    pub valid_from: NaiveDate,
    /// Last day the bundle covers, inclusive.
    pub valid_until: NaiveDate,
}

/// The bundle the planner uses, as the frontend shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct InstalledTimetable {
    /// Version of the data as `yyyymmddNN`.
    pub version: u32,
    /// First day the bundle covers.
    pub valid_from: NaiveDate,
    /// Last day the bundle covers, inclusive.
    pub valid_until: NaiveDate,
    /// Days from today to the last day. Negative once the bundle has expired.
    pub days_left: i32,
    /// Whether `days_left` is low enough to warn about.
    pub running_out: bool,
}

/// The newest bundle on the server, as the frontend shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct AvailableTimetable {
    /// Version of the data as `yyyymmddNN`.
    pub version: u32,
    /// Last day the bundle covers, inclusive.
    pub valid_until: NaiveDate,
    /// Size of the download in bytes.
    pub size_bytes: u32,
    /// Whether this is newer than the installed bundle, or whether none is installed.
    pub newer: bool,
    /// Whether this app can read the bundle.
    pub readable: bool,
    /// The oldest app version that reads the bundle.
    pub minimum_app_version: String,
}

/// What the app knows about its timetable and the ones on the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct TimetableStatus {
    /// The bundle the planner uses, or `None` when the app plans without one.
    pub installed: Option<InstalledTimetable>,
    /// The newest bundle on the server. `None` until a check succeeds, or when the server lists
    /// no valid bundle.
    pub available: Option<AvailableTimetable>,
    /// Whether the app checks for a newer timetable on startup.
    pub checks_enabled: bool,
    /// Why the last check failed, if it did.
    pub check_error: Option<String>,
}

/// The step of a download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
pub enum DownloadStage {
    /// Bytes are arriving from the server.
    Downloading,
    /// The download is complete and is being unpacked and checked.
    Unpacking,
}

/// How far a download has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
pub struct DownloadProgress {
    /// The step the download is in.
    pub stage: DownloadStage,
    /// Bytes received so far.
    pub received_bytes: u32,
    /// Bytes the download has in total.
    pub total_bytes: u32,
}

/// Returns whether this version of the app reads the bundle of a manifest entry.
pub fn app_can_read(entry: &ManifestEntry, app_version: &str) -> bool {
    let (Ok(app), Ok(minimum)) = (
        Version::parse(app_version),
        Version::parse(&entry.minimum_app_version),
    ) else {
        return false;
    };
    entry.format_version == BUNDLE_FORMAT_VERSION && app >= minimum
}

/// Returns the newest bundle in the manifest that still covers some day from `today` on.
pub fn newest_entry(manifest: &TimetableManifest, today: NaiveDate) -> Option<&ManifestEntry> {
    manifest
        .bundles
        .iter()
        .filter(|entry| entry.valid_until >= today)
        .max_by_key(|entry| entry.version)
}

/// Collects what the frontend shows about the timetable.
pub fn build_status(
    installed: Option<&InstalledBundle>,
    manifest: Option<&TimetableManifest>,
    inputs: StatusInputs<'_>,
) -> TimetableStatus {
    let installed_status = installed.map(|bundle| {
        let days_left =
            i32::try_from((bundle.valid_until - inputs.today).num_days()).unwrap_or(i32::MIN);
        InstalledTimetable {
            version: bundle.version,
            valid_from: bundle.valid_from,
            valid_until: bundle.valid_until,
            days_left,
            running_out: days_left < WARNING_DAYS_LEFT,
        }
    });
    let available = manifest
        .and_then(|manifest| newest_entry(manifest, inputs.today))
        .map(|entry| AvailableTimetable {
            version: entry.version,
            valid_until: entry.valid_until,
            size_bytes: u32::try_from(entry.size_bytes).unwrap_or(u32::MAX),
            newer: installed.is_none_or(|bundle| entry.version > bundle.version),
            readable: app_can_read(entry, inputs.app_version),
            minimum_app_version: entry.minimum_app_version.clone(),
        });
    TimetableStatus {
        installed: installed_status,
        available,
        checks_enabled: inputs.checks_enabled,
        check_error: inputs.check_error.map(str::to_owned),
    }
}

/// The inputs of `build_status` that do not come from the bundle or the manifest.
#[derive(Debug, Clone, Copy)]
pub struct StatusInputs<'a> {
    /// Today in UTC.
    pub today: NaiveDate,
    /// Version of this app.
    pub app_version: &'a str,
    /// Whether the check on startup is on.
    pub checks_enabled: bool,
    /// Why the last check failed, if it did.
    pub check_error: Option<&'a str>,
}

/// Returns the facts about the bundle a router plans on.
pub fn installed_bundle_of<P: JourneyPricer>(router: &TimetableBundleRouter<P>) -> InstalledBundle {
    InstalledBundle {
        version: router.bundle_version(),
        valid_from: router.validity_start(),
        valid_until: last_valid_day(router.validity_start(), router.validity_days()),
    }
}

/// A listed operator whose trains stop before the end of the timetable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct OperatorEndDate {
    /// The name the app shows to the traveller.
    pub name: String,
    /// The country of the operator's home network, as an ISO 3166 code.
    pub country: String,
    /// The last day on which one of the operator's trains departs from its first stop.
    pub last_departure_date: NaiveDate,
}

/// A timetable feed that the planner's bundle was built from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct FeedCredit {
    /// The name of the feed, such as `de_DELFI`.
    pub name: String,
    /// The license of the feed, or `unknown` when its publisher states none.
    pub license: String,
    /// The address of the feed or its terms, or an empty text when none is known.
    pub url: String,
}
