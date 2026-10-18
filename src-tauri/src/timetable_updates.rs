//! Finding, downloading and installing timetable bundles.
//!
//! The timetable workflow publishes a manifest of the bundles that can be downloaded. The app
//! reads it, downloads the newest bundle that the app can read, checks it and puts it in the
//! `bundles` folder of the app data directory, where `find_bundle` picks it up on the next start.

// Tauri passes `State` and other arguments by value, which the command signatures must match.
#![allow(clippy::needless_pass_by_value)]

use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use btt_timetable::format::BUNDLE_FORMAT_VERSION;
use btt_timetable::manifest::{ManifestEntry, TimetableManifest};
use btt_timetable::{BundleLoadError, LoadedTimetableBundle};
use chrono::NaiveDate;
use semver::Version;
use serde::Serialize;
use sha2::{Digest, Sha256};
use specta::Type;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use thiserror::Error;

use crate::commands::{CommandError, today_utc};
use crate::state::AppState;

/// Where the timetable workflow publishes the manifest. The address never changes, because the
/// workflow replaces the file in the same release.
const MANIFEST_URL: &str =
    "https://github.com/nardi/btt/releases/download/timetable-index/timetable.json";

/// Environment variable that replaces `MANIFEST_URL`, for tests with a local server.
const MANIFEST_URL_VARIABLE: &str = "BTT_TIMETABLE_MANIFEST_URL";

/// Key of the setting that turns the check for a newer timetable on startup on or off.
const CHECKS_SETTING_KEY: &str = "timetable_checks_enabled";

/// Days of validity left below which the app warns that the timetable runs out.
const WARNING_DAYS_LEFT: i32 = 14;

/// Bundle files the app keeps: the newest and the one before it.
pub const KEPT_BUNDLE_FILES: usize = 2;

/// Seconds without any data after which a download counts as stalled.
const STALLED_DOWNLOAD_SECONDS: u64 = 30;

/// Errors from checking for, downloading and installing a timetable.
#[derive(Debug, Error)]
pub enum TimetableUpdateError {
    /// The server could not be reached or answered with an error.
    #[error("could not get the timetable from the server: {0}")]
    Request(#[from] reqwest::Error),
    /// A file could not be written or read.
    #[error("could not write the timetable to disk: {0}")]
    Io(#[from] io::Error),
    /// The download has another size than the manifest says.
    #[error("the download is {received} bytes, but the manifest says {expected}")]
    WrongSize {
        /// Bytes the server sent.
        received: u64,
        /// Bytes the manifest lists.
        expected: u64,
    },
    /// The download does not match the checksum in the manifest.
    #[error("the download does not match its checksum")]
    ChecksumMismatch,
    /// The unpacked file is not a bundle this app reads.
    #[error("the downloaded timetable cannot be used: {0}")]
    Unusable(#[from] BundleLoadError),
    /// The unpacked bundle has another version than the manifest entry.
    #[error("the downloaded timetable is version {found}, not {expected}")]
    WrongVersion {
        /// Version in the bundle.
        found: u32,
        /// Version in the manifest.
        expected: u32,
    },
    /// The newest bundle needs a newer app.
    #[error("the newest timetable needs version {0} of the app")]
    AppTooOld(String),
    /// The manifest lists no bundle that is still valid.
    #[error("there is no timetable to download")]
    NothingToDownload,
    /// A download is already running.
    #[error("a timetable is already downloading")]
    AlreadyDownloading,
}

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
    /// Whether a timetable was downloaded that the planner will only use after a restart.
    pub restart_needed: bool,
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

/// Returns the file name of the bundle with a version.
pub fn bundle_file_name(version: u32) -> String {
    format!("timetable-{version}.bundle")
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
    let restart_needed = inputs
        .downloaded_version
        .is_some_and(|downloaded| installed.is_none_or(|bundle| downloaded > bundle.version));
    TimetableStatus {
        installed: installed_status,
        available,
        checks_enabled: inputs.checks_enabled,
        restart_needed,
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
    /// Version of a bundle downloaded during this run, if any.
    pub downloaded_version: Option<u32>,
    /// Why the last check failed, if it did.
    pub check_error: Option<&'a str>,
}

/// Deletes the bundle files beyond the newest `kept` ones, and any leftovers of a download that
/// stopped half way. The last remaining bundle is never deleted, even when it has expired.
///
/// # Errors
///
/// Returns an error when the folder cannot be read or a file cannot be deleted.
pub fn prune_bundles(directory: &Path, kept: usize) -> io::Result<()> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut bundles: Vec<(u32, PathBuf)> = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let is_temporary = Path::new(name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"));
        if name.starts_with("timetable-") && is_temporary {
            std::fs::remove_file(&path)?;
        } else if let Some(version) = name
            .strip_prefix("timetable-")
            .and_then(|rest| rest.strip_suffix(".bundle"))
            .and_then(|version| version.parse().ok())
        {
            bundles.push((version, path));
        }
    }
    bundles.sort_by_key(|(version, _)| std::cmp::Reverse(*version));
    for (_, path) in bundles.into_iter().skip(kept.max(1)) {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Returns the SHA-256 digest as lowercase hexadecimal.
fn hex_digest(hasher: Sha256) -> String {
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            // Writing to a string never fails.
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Downloads the bundle of a manifest entry into `directory` and returns its path.
///
/// The compressed file streams to `<name>.zst.tmp` while its SHA-256 is computed. Once its size
/// and checksum match, it unpacks to `<name>.tmp`, which loads as a trial. Only then does it get
/// its final name, so `find_bundle` never sees a partial file. Every temporary file is deleted
/// when the download fails.
///
/// # Errors
///
/// Returns an error when the request fails, the download does not match the manifest, the
/// bundle does not load, or a file cannot be written.
pub async fn download_bundle(
    client: &reqwest::Client,
    entry: &ManifestEntry,
    directory: &Path,
    progress: &mut impl FnMut(DownloadProgress),
) -> Result<PathBuf, TimetableUpdateError> {
    std::fs::create_dir_all(directory)?;
    let final_path = directory.join(bundle_file_name(entry.version));
    let compressed_path = directory.join(format!("timetable-{}.zst.tmp", entry.version));
    let unpacked_path = directory.join(format!("timetable-{}.bundle.tmp", entry.version));
    let result = fetch_and_unpack(
        client,
        entry,
        &compressed_path,
        &unpacked_path,
        &final_path,
        progress,
    )
    .await;
    // After a success the unpacked file has been renamed, so only failures leave files behind.
    let _ = std::fs::remove_file(&compressed_path);
    let _ = std::fs::remove_file(&unpacked_path);
    result.map(|()| final_path)
}

/// Does the work of `download_bundle`, which deletes the temporary files afterwards.
async fn fetch_and_unpack(
    client: &reqwest::Client,
    entry: &ManifestEntry,
    compressed_path: &Path,
    unpacked_path: &Path,
    final_path: &Path,
    progress: &mut impl FnMut(DownloadProgress),
) -> Result<(), TimetableUpdateError> {
    let total_bytes = u32::try_from(entry.size_bytes).unwrap_or(u32::MAX);
    let mut response = client.get(&entry.url).send().await?.error_for_status()?;
    let mut file = BufWriter::new(File::create(compressed_path)?);
    let mut hasher = Sha256::new();
    let mut received: u64 = 0;
    while let Some(chunk) = response.chunk().await? {
        received += chunk.len() as u64;
        // A server that sends more than the manifest lists is not sending this bundle.
        if received > entry.size_bytes {
            return Err(TimetableUpdateError::WrongSize {
                received,
                expected: entry.size_bytes,
            });
        }
        file.write_all(&chunk)?;
        hasher.update(&chunk);
        progress(DownloadProgress {
            stage: DownloadStage::Downloading,
            received_bytes: u32::try_from(received).unwrap_or(u32::MAX),
            total_bytes,
        });
    }
    file.flush()?;
    drop(file);
    if received != entry.size_bytes {
        return Err(TimetableUpdateError::WrongSize {
            received,
            expected: entry.size_bytes,
        });
    }
    if !hex_digest(hasher).eq_ignore_ascii_case(&entry.sha256) {
        return Err(TimetableUpdateError::ChecksumMismatch);
    }
    progress(DownloadProgress {
        stage: DownloadStage::Unpacking,
        received_bytes: total_bytes,
        total_bytes,
    });
    let compressed_path = compressed_path.to_owned();
    let unpacked_path = unpacked_path.to_owned();
    let final_path = final_path.to_owned();
    let version = entry.version;
    // Unpacking and validating take a while and block, so they stay off the async threads.
    tokio::task::spawn_blocking(move || {
        unpack_and_install(&compressed_path, &unpacked_path, &final_path, version)
    })
    .await
    .map_err(io::Error::other)?
}

/// Unpacks a compressed bundle, loads it as a trial and renames it to its final name.
fn unpack_and_install(
    compressed_path: &Path,
    unpacked_path: &Path,
    final_path: &Path,
    version: u32,
) -> Result<(), TimetableUpdateError> {
    let mut output = BufWriter::new(File::create(unpacked_path)?);
    zstd::stream::copy_decode(BufReader::new(File::open(compressed_path)?), &mut output)?;
    output.flush()?;
    drop(output);
    let loaded = LoadedTimetableBundle::open(unpacked_path)?;
    let found = loaded.bundle_version();
    // The mapping must end before the rename, because Windows refuses to rename a mapped file.
    drop(loaded);
    if found != version {
        return Err(TimetableUpdateError::WrongVersion {
            found,
            expected: version,
        });
    }
    std::fs::rename(unpacked_path, final_path)?;
    Ok(())
}

/// What a session learned from the server.
#[derive(Default)]
struct UpdateSession {
    /// The manifest of the last successful check.
    manifest: Option<TimetableManifest>,
    /// Why the last check failed, if it did.
    check_error: Option<String>,
    /// Version of a bundle downloaded during this run.
    downloaded_version: Option<u32>,
    /// Whether a download is running.
    downloading: bool,
}

/// Checks for timetables, downloads them and remembers what it learned during this run.
pub struct TimetableUpdates {
    /// HTTP client for the manifest and the downloads.
    http_client: reqwest::Client,
    /// Address of the manifest.
    manifest_url: String,
    /// The `bundles` folder of the app data directory.
    bundles_directory: PathBuf,
    /// What the checks and downloads of this run found.
    session: Mutex<UpdateSession>,
}

impl TimetableUpdates {
    /// Creates the updater for the `bundles` folder of an app data directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be built.
    pub fn new(data_directory: &Path, user_agent: &str) -> Result<Self, reqwest::Error> {
        let http_client = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(15))
            // A download takes minutes, so only a stall ends it and not the total time.
            .read_timeout(Duration::from_secs(STALLED_DOWNLOAD_SECONDS))
            .build()?;
        Ok(Self {
            http_client,
            manifest_url: std::env::var(MANIFEST_URL_VARIABLE)
                .unwrap_or_else(|_| MANIFEST_URL.to_owned()),
            bundles_directory: data_directory.join("bundles"),
            session: Mutex::new(UpdateSession::default()),
        })
    }

    /// Returns the `bundles` folder.
    pub fn bundles_directory(&self) -> &Path {
        &self.bundles_directory
    }

    /// Locks the session. Nothing in it breaks when another holder panics.
    fn session(&self) -> MutexGuard<'_, UpdateSession> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Fetches the manifest and remembers it, or the reason it failed.
    pub async fn check(&self) {
        let result = async {
            self.http_client
                .get(&self.manifest_url)
                .timeout(Duration::from_secs(30))
                .send()
                .await?
                .error_for_status()?
                .json::<TimetableManifest>()
                .await
        }
        .await;
        let mut session = self.session();
        match result {
            Ok(manifest) => {
                session.manifest = Some(manifest);
                session.check_error = None;
            }
            Err(error) => {
                tracing::warn!(%error, "could not check for a newer timetable");
                session.check_error = Some(TimetableUpdateError::from(error).to_string());
            }
        }
    }

    /// Downloads and installs the newest bundle of the last check.
    ///
    /// # Errors
    ///
    /// Returns an error when no check has found a bundle, the app is too old for it, a download
    /// is already running, or the download fails.
    pub async fn download_newest(
        &self,
        today: NaiveDate,
        app_version: &str,
        progress: &mut impl FnMut(DownloadProgress),
    ) -> Result<(), TimetableUpdateError> {
        let entry = {
            let mut session = self.session();
            if session.downloading {
                return Err(TimetableUpdateError::AlreadyDownloading);
            }
            let entry = session
                .manifest
                .as_ref()
                .and_then(|manifest| newest_entry(manifest, today))
                .cloned()
                .ok_or(TimetableUpdateError::NothingToDownload)?;
            if !app_can_read(&entry, app_version) {
                return Err(TimetableUpdateError::AppTooOld(entry.minimum_app_version));
            }
            session.downloading = true;
            entry
        };
        let result =
            download_bundle(&self.http_client, &entry, &self.bundles_directory, progress).await;
        let mut session = self.session();
        session.downloading = false;
        if result.is_ok() {
            session.downloaded_version = Some(entry.version);
        }
        result.map(|_| ())
    }

    /// Returns what the app knows about its timetable.
    pub fn status(
        &self,
        installed: Option<&InstalledBundle>,
        checks_enabled: bool,
        today: NaiveDate,
    ) -> TimetableStatus {
        let session = self.session();
        build_status(
            installed,
            session.manifest.as_ref(),
            StatusInputs {
                today,
                app_version: env!("CARGO_PKG_VERSION"),
                checks_enabled,
                downloaded_version: session.downloaded_version,
                check_error: session.check_error.as_deref(),
            },
        )
    }
}

/// Returns whether the check for a newer timetable on startup is on. It is on until the user
/// turns it off.
fn checks_enabled(state: &AppState) -> bool {
    state
        .storage
        .setting(CHECKS_SETTING_KEY)
        .ok()
        .flatten()
        .is_none_or(|value| value != "false")
}

/// Returns what the app knows about its timetable, without any request.
#[tauri::command]
#[specta::specta]
pub fn timetable_status(state: State<'_, AppState>) -> TimetableStatus {
    state.timetable_updates.status(
        state.installed_bundle.as_ref(),
        checks_enabled(&state),
        today_utc(),
    )
}

/// Asks the server for the list of timetables and returns the status with the answer. A failed
/// request does not fail the command. The status carries the reason instead.
///
/// Async, so the request does not block the window.
#[tauri::command]
#[specta::specta]
pub async fn check_timetable(state: State<'_, AppState>) -> Result<TimetableStatus, CommandError> {
    state.timetable_updates.check().await;
    Ok(timetable_status(state))
}

/// Downloads the newest timetable and reports its progress through `progress`. The planner uses
/// it after the next start of the app.
///
/// Async, so the download does not block the window.
#[tauri::command]
#[specta::specta]
pub async fn download_timetable(
    state: State<'_, AppState>,
    progress: Channel<DownloadProgress>,
) -> Result<TimetableStatus, CommandError> {
    state
        .timetable_updates
        .download_newest(today_utc(), env!("CARGO_PKG_VERSION"), &mut |update| {
            // The channel only fails when the window has closed, and then nobody needs the update.
            let _ = progress.send(update);
        })
        .await
        .map_err(|error| CommandError::Timetable {
            message: error.to_string(),
        })?;
    Ok(timetable_status(state))
}

/// Turns the check for a newer timetable on startup on or off.
#[tauri::command]
#[specta::specta]
pub fn set_timetable_checks(state: State<'_, AppState>, enabled: bool) -> Result<(), CommandError> {
    state
        .storage
        .set_setting(CHECKS_SETTING_KEY, if enabled { "true" } else { "false" })?;
    Ok(())
}

/// Restarts the app, so the planner loads a timetable that was just downloaded.
#[tauri::command]
#[specta::specta]
pub fn restart_app(app: AppHandle) {
    app.restart();
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::net::TcpListener;

    use btt_timetable::BundleBuilder;
    use btt_timetable::manifest::MINIMUM_APP_VERSION;

    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    /// Returns a fresh folder for one test.
    fn test_directory(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("btt-updates-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn entry(version: u32, valid_until: NaiveDate) -> ManifestEntry {
        ManifestEntry {
            version,
            format_version: BUNDLE_FORMAT_VERSION,
            valid_from: date(2026, 1, 1),
            valid_until,
            url: String::new(),
            size_bytes: 1,
            sha256: String::new(),
            minimum_app_version: MINIMUM_APP_VERSION.to_owned(),
        }
    }

    fn inputs(today: NaiveDate) -> StatusInputs<'static> {
        StatusInputs {
            today,
            app_version: "0.1.0",
            checks_enabled: true,
            downloaded_version: None,
            check_error: None,
        }
    }

    #[test]
    fn an_app_reads_a_bundle_of_its_format_from_the_minimum_version_on() {
        let mut bundle = entry(1, date(2027, 1, 1));
        bundle.minimum_app_version = "0.2.0".to_owned();
        assert!(!app_can_read(&bundle, "0.1.9"));
        assert!(app_can_read(&bundle, "0.2.0"));
        assert!(app_can_read(&bundle, "1.0.0"));
        bundle.minimum_app_version = "0.1.0".to_owned();
        bundle.format_version = BUNDLE_FORMAT_VERSION + 1;
        assert!(!app_can_read(&bundle, "1.0.0"));
    }

    #[test]
    fn the_newest_entry_skips_expired_bundles() {
        let manifest = TimetableManifest {
            bundles: vec![
                entry(3, date(2026, 10, 1)),
                entry(2, date(2026, 12, 1)),
                entry(1, date(2026, 11, 1)),
            ],
        };
        assert_eq!(
            newest_entry(&manifest, date(2026, 10, 18)).unwrap().version,
            2
        );
        assert!(newest_entry(&manifest, date(2027, 1, 1)).is_none());
    }

    #[test]
    fn the_status_warns_before_the_installed_bundle_expires() {
        let installed = InstalledBundle {
            version: 5,
            valid_from: date(2026, 10, 1),
            valid_until: date(2026, 10, 30),
        };
        let status = build_status(Some(&installed), None, inputs(date(2026, 10, 18)));
        let installed_status = status.installed.unwrap();
        assert_eq!(installed_status.days_left, 12);
        assert!(installed_status.running_out);
        let status = build_status(Some(&installed), None, inputs(date(2026, 10, 10)));
        assert!(!status.installed.unwrap().running_out);
        let status = build_status(Some(&installed), None, inputs(date(2026, 11, 2)));
        assert_eq!(status.installed.unwrap().days_left, -3);
    }

    #[test]
    fn the_status_offers_a_bundle_only_when_it_is_newer_or_none_is_installed() {
        let manifest = TimetableManifest {
            bundles: vec![entry(7, date(2027, 1, 1))],
        };
        let installed = InstalledBundle {
            version: 7,
            valid_from: date(2026, 10, 1),
            valid_until: date(2027, 1, 1),
        };
        let today = date(2026, 10, 18);
        let same = build_status(Some(&installed), Some(&manifest), inputs(today));
        assert!(!same.available.unwrap().newer);
        let older = InstalledBundle {
            version: 6,
            ..installed
        };
        let newer = build_status(Some(&older), Some(&manifest), inputs(today));
        assert!(newer.available.unwrap().newer);
        let none = build_status(None, Some(&manifest), inputs(today));
        assert!(none.available.unwrap().newer);
        assert!(none.installed.is_none());
    }

    #[test]
    fn the_status_marks_a_bundle_the_app_is_too_old_for() {
        let mut bundle = entry(7, date(2027, 1, 1));
        bundle.minimum_app_version = "9.0.0".to_owned();
        let manifest = TimetableManifest {
            bundles: vec![bundle],
        };
        let status = build_status(None, Some(&manifest), inputs(date(2026, 10, 18)));
        let available = status.available.unwrap();
        assert!(!available.readable);
        assert_eq!(available.minimum_app_version, "9.0.0");
    }

    #[test]
    fn a_download_that_the_planner_has_not_loaded_needs_a_restart() {
        let installed = InstalledBundle {
            version: 5,
            valid_from: date(2026, 10, 1),
            valid_until: date(2027, 1, 1),
        };
        let mut status_inputs = inputs(date(2026, 10, 18));
        assert!(!build_status(Some(&installed), None, status_inputs).restart_needed);
        status_inputs.downloaded_version = Some(6);
        assert!(build_status(Some(&installed), None, status_inputs).restart_needed);
        assert!(build_status(None, None, status_inputs).restart_needed);
    }

    #[test]
    fn pruning_keeps_the_two_newest_bundles_and_removes_leftovers() {
        let directory = test_directory("prune");
        for name in [
            "timetable-2026100101.bundle",
            "timetable-2026101001.bundle",
            "timetable-2026101801.bundle",
            "timetable-2026101801.bundle.tmp",
            "timetable-2026101801.zst.tmp",
            "notes.txt",
        ] {
            std::fs::write(directory.join(name), b"x").unwrap();
        }
        prune_bundles(&directory, KEPT_BUNDLE_FILES).unwrap();
        let mut left: Vec<String> = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "notes.txt",
                "timetable-2026101001.bundle",
                "timetable-2026101801.bundle"
            ]
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn pruning_never_removes_the_last_bundle() {
        let directory = test_directory("prune-last");
        std::fs::write(directory.join("timetable-2026101801.bundle"), b"x").unwrap();
        prune_bundles(&directory, 0).unwrap();
        assert!(directory.join("timetable-2026101801.bundle").exists());
        std::fs::remove_dir_all(&directory).unwrap();
        // A folder that does not exist yet is fine.
        prune_bundles(&directory, KEPT_BUNDLE_FILES).unwrap();
    }

    /// How the test server answers one path.
    struct Route {
        path: &'static str,
        body: Vec<u8>,
        status: u16,
        /// Bytes the headers promise beyond the body, so the connection ends early.
        missing_bytes: usize,
    }

    /// Opens a local port and returns it with its address, so a route can name its own server.
    fn bind() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        (listener, address)
    }

    /// Serves fixed answers on a local port and returns its address.
    fn serve(routes: Vec<Route>) -> String {
        let (listener, address) = bind();
        serve_on(listener, routes);
        address
    }

    /// Answers the requests of a listener from a thread that ends with the test process.
    fn serve_on(listener: TcpListener, routes: Vec<Route>) {
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => request.extend_from_slice(&buffer[..read]),
                    }
                }
                let request = String::from_utf8_lossy(&request);
                let path = request.split_whitespace().nth(1).unwrap_or_default();
                let route = routes.iter().find(|route| route.path == path);
                let (status, body, missing) = route.map_or((404, &[][..], 0), |r| {
                    (r.status, &r.body[..], r.missing_bytes)
                });
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len() + missing
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body);
            }
        });
    }

    /// Builds an empty bundle with a version and compresses it, as the timetable workflow does.
    fn compressed_bundle(directory: &Path, version: u32) -> Vec<u8> {
        let bundle = BundleBuilder::new(date(2026, 10, 18), 90, version)
            .unwrap()
            .build()
            .unwrap();
        let path = directory.join("source.bundle");
        LoadedTimetableBundle::write_bundle(&bundle, &path).unwrap();
        let compressed = zstd::stream::encode_all(File::open(&path).unwrap(), 3).unwrap();
        std::fs::remove_file(path).unwrap();
        compressed
    }

    fn entry_for(url: String, compressed: &[u8], version: u32) -> ManifestEntry {
        let mut hasher = Sha256::new();
        hasher.update(compressed);
        ManifestEntry {
            url,
            size_bytes: compressed.len() as u64,
            sha256: hex_digest(hasher),
            ..entry(version, date(2027, 1, 15))
        }
    }

    fn files_in(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn a_download_is_checked_unpacked_and_given_its_final_name() {
        let directory = test_directory("download");
        let compressed = compressed_bundle(&directory, 2_026_101_801);
        let base = serve(vec![Route {
            path: "/bundle",
            body: compressed.clone(),
            status: 200,
            missing_bytes: 0,
        }]);
        let entry = entry_for(format!("{base}/bundle"), &compressed, 2_026_101_801);
        let bundles = directory.join("bundles");
        let mut reports = Vec::new();

        let path = download_bundle(&reqwest::Client::new(), &entry, &bundles, &mut |update| {
            reports.push(update);
        })
        .await
        .unwrap();

        assert_eq!(path, bundles.join("timetable-2026101801.bundle"));
        assert_eq!(files_in(&bundles), ["timetable-2026101801.bundle"]);
        assert_eq!(
            LoadedTimetableBundle::open(&path).unwrap().bundle_version(),
            2_026_101_801
        );
        let last = reports.last().unwrap();
        assert_eq!(last.stage, DownloadStage::Unpacking);
        assert!(reports.iter().any(|report| {
            report.stage == DownloadStage::Downloading
                && report.received_bytes == report.total_bytes
        }));
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[tokio::test]
    async fn a_download_with_the_wrong_checksum_leaves_nothing_behind() {
        let directory = test_directory("checksum");
        let compressed = compressed_bundle(&directory, 2_026_101_801);
        let base = serve(vec![Route {
            path: "/bundle",
            body: compressed.clone(),
            status: 200,
            missing_bytes: 0,
        }]);
        let mut entry = entry_for(format!("{base}/bundle"), &compressed, 2_026_101_801);
        entry.sha256 = "0".repeat(64);
        let bundles = directory.join("bundles");

        let error = download_bundle(&reqwest::Client::new(), &entry, &bundles, &mut |_| {})
            .await
            .unwrap_err();

        assert!(matches!(error, TimetableUpdateError::ChecksumMismatch));
        assert_eq!(files_in(&bundles), Vec::<String>::new());
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[tokio::test]
    async fn a_download_that_ends_early_leaves_nothing_behind() {
        let directory = test_directory("truncated");
        let compressed = compressed_bundle(&directory, 2_026_101_801);
        let base = serve(vec![Route {
            path: "/bundle",
            body: compressed[..compressed.len() / 2].to_vec(),
            status: 200,
            missing_bytes: compressed.len() - compressed.len() / 2,
        }]);
        let entry = entry_for(format!("{base}/bundle"), &compressed, 2_026_101_801);
        let bundles = directory.join("bundles");

        let result = download_bundle(&reqwest::Client::new(), &entry, &bundles, &mut |_| {}).await;

        assert!(result.is_err());
        assert_eq!(files_in(&bundles), Vec::<String>::new());
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[tokio::test]
    async fn a_bundle_that_does_not_load_is_never_installed() {
        let directory = test_directory("garbage");
        let garbage = zstd::stream::encode_all(&b"this is not a bundle"[..], 3).unwrap();
        let base = serve(vec![Route {
            path: "/bundle",
            body: garbage.clone(),
            status: 200,
            missing_bytes: 0,
        }]);
        let entry = entry_for(format!("{base}/bundle"), &garbage, 2_026_101_801);
        let bundles = directory.join("bundles");

        let error = download_bundle(&reqwest::Client::new(), &entry, &bundles, &mut |_| {})
            .await
            .unwrap_err();

        assert!(matches!(error, TimetableUpdateError::Unusable(_)));
        assert_eq!(files_in(&bundles), Vec::<String>::new());
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[tokio::test]
    async fn a_bundle_with_another_version_than_the_manifest_is_never_installed() {
        let directory = test_directory("version");
        let compressed = compressed_bundle(&directory, 2_026_101_802);
        let base = serve(vec![Route {
            path: "/bundle",
            body: compressed.clone(),
            status: 200,
            missing_bytes: 0,
        }]);
        let entry = entry_for(format!("{base}/bundle"), &compressed, 2_026_101_801);
        let bundles = directory.join("bundles");

        let error = download_bundle(&reqwest::Client::new(), &entry, &bundles, &mut |_| {})
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            TimetableUpdateError::WrongVersion {
                found: 2_026_101_802,
                expected: 2_026_101_801
            }
        ));
        assert_eq!(files_in(&bundles), Vec::<String>::new());
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[tokio::test]
    async fn a_missing_file_on_the_server_is_an_error_and_keeps_the_old_bundle() {
        let directory = test_directory("missing");
        let bundles = directory.join("bundles");
        std::fs::create_dir_all(&bundles).unwrap();
        std::fs::write(bundles.join("timetable-2026100101.bundle"), b"old").unwrap();
        let base = serve(Vec::new());
        let entry = entry_for(format!("{base}/bundle"), b"x", 2_026_101_801);

        let error = download_bundle(&reqwest::Client::new(), &entry, &bundles, &mut |_| {})
            .await
            .unwrap_err();

        assert!(matches!(error, TimetableUpdateError::Request(_)));
        assert_eq!(files_in(&bundles), ["timetable-2026100101.bundle"]);
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[tokio::test]
    async fn a_check_remembers_the_manifest_and_a_download_installs_its_newest_bundle() {
        let directory = test_directory("session");
        let compressed = compressed_bundle(&directory, 2_026_101_801);
        // One server answers for the manifest and the bundle, and the manifest names the server.
        let (listener, base) = bind();
        let manifest = TimetableManifest {
            bundles: vec![entry_for(
                format!("{base}/bundle"),
                &compressed,
                2_026_101_801,
            )],
        };
        serve_on(
            listener,
            vec![
                Route {
                    path: "/bundle",
                    body: compressed.clone(),
                    status: 200,
                    missing_bytes: 0,
                },
                Route {
                    path: "/timetable.json",
                    body: serde_json::to_vec(&manifest).unwrap(),
                    status: 200,
                    missing_bytes: 0,
                },
            ],
        );
        let mut updates = TimetableUpdates::new(&directory, "test").unwrap();
        updates.manifest_url = format!("{base}/timetable.json");
        let today = date(2026, 10, 18);

        updates.check().await;
        let before = updates.status(None, true, today);
        assert!(before.check_error.is_none());
        assert!(before.available.as_ref().unwrap().newer);
        assert!(!before.restart_needed);

        updates
            .download_newest(today, "0.1.0", &mut |_| {})
            .await
            .unwrap();

        let after = updates.status(None, true, today);
        assert!(after.restart_needed);
        assert_eq!(
            files_in(updates.bundles_directory()),
            ["timetable-2026101801.bundle"]
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }

    /// Downloads the bundle that a real manifest lists and opens it with the planner. It needs
    /// a server, so it only runs on request:
    ///
    /// `node tools/serve-timetable.js <builder output>`, then in another terminal
    /// `BTT_TIMETABLE_MANIFEST_URL=http://127.0.0.1:8787/timetable.json cargo test -p btt-app
    /// a_published_bundle -- --ignored`.
    #[tokio::test]
    #[ignore = "needs a server from tools/serve-timetable.js"]
    async fn a_published_bundle_downloads_and_opens_with_the_planner() {
        let directory = test_directory("published");
        let updates = TimetableUpdates::new(&directory, "test").unwrap();
        updates.check().await;
        let today = chrono::Utc::now().date_naive();
        let status = updates.status(None, true, today);
        assert_eq!(status.check_error, None);
        let mut last_stage = None;
        updates
            .download_newest(today, env!("CARGO_PKG_VERSION"), &mut |progress| {
                last_stage = Some(progress.stage);
            })
            .await
            .unwrap();
        assert_eq!(last_stage, Some(DownloadStage::Unpacking));
        let installed = files_in(updates.bundles_directory());
        assert_eq!(installed.len(), 1);
        let router = btt_planner::TimetableBundleRouter::open(
            &updates.bundles_directory().join(&installed[0]),
            std::sync::Arc::new(btt_geo::TimeZoneFinder::new()),
            btt_planner::NoPrice,
        )
        .unwrap();
        assert!(installed[0].contains(&router.bundle_version().to_string()));
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[tokio::test]
    async fn a_failed_check_is_reported_in_the_status() {
        let directory = test_directory("failed-check");
        let mut updates = TimetableUpdates::new(&directory, "test").unwrap();
        updates.manifest_url = format!("{}/timetable.json", serve(Vec::new()));

        updates.check().await;

        let status = updates.status(None, true, date(2026, 10, 18));
        assert!(status.check_error.is_some());
        assert!(status.available.is_none());
        let error = updates
            .download_newest(date(2026, 10, 18), "0.1.0", &mut |_| {})
            .await
            .unwrap_err();
        assert!(matches!(error, TimetableUpdateError::NothingToDownload));
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
