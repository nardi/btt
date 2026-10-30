//! Portable mode keeps the data of the app in a folder next to the app, instead of in the
//! profile of the user. The choice is saved in a settings file in the profile, because the app
//! needs it before it opens the database.

// Tauri passes `AppHandle` by value, which the command signatures must match.
#![allow(clippy::needless_pass_by_value)]

use std::fmt::Display;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};

use crate::commands::CommandError;

/// Name of the settings file in the configuration folder of the user profile.
const SETTINGS_FILE_NAME: &str = "location.json";

/// Name of the folder next to the app that holds the data in portable mode.
const DATA_DIRECTORY_NAME: &str = "data";

/// Environment variable that the `AppImage` runtime sets to the path of the `.AppImage` file.
/// The executable itself runs from a temporary mount, so its own location says nothing.
const APPIMAGE_VARIABLE: &str = "APPIMAGE";

/// Name of the file that proves a folder is writable. The check removes it again.
const WRITE_CHECK_FILE_NAME: &str = ".btt-write-check";

/// Where the app keeps its data, as saved in the settings file.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocationSettings {
    /// Whether the data sits in the `data` folder next to the app.
    pub portable: bool,
    /// Whether the user answered the first-launch question of the portable build.
    pub answered: bool,
    /// Folder with the data from before a change of location. The next start copies it into the
    /// data folder of the new location.
    pub pending_move_from: Option<PathBuf>,
}

/// What the settings page and the first-launch question need to know about the data location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct DataLocationStatus {
    /// Whether the portable build still has to ask where to keep the data.
    pub question_pending: bool,
    /// Whether the data sits next to the app.
    pub portable: bool,
    /// Whether the app must restart before a changed location takes effect.
    pub restart_pending: bool,
}

/// Returns whether this build is the portable build, which compiles with the `portable-build`
/// feature.
pub fn is_portable_build() -> bool {
    cfg!(feature = "portable-build")
}

/// Returns the path of the settings file in the configuration folder.
pub fn settings_path(configuration_directory: &Path) -> PathBuf {
    configuration_directory.join(SETTINGS_FILE_NAME)
}

/// Reads the settings file. A missing or unreadable file gives the defaults.
pub fn load_settings(configuration_directory: &Path) -> LocationSettings {
    fs::read_to_string(settings_path(configuration_directory))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Writes the settings file and creates the configuration folder when it is missing.
pub fn save_settings(
    configuration_directory: &Path,
    settings: &LocationSettings,
) -> io::Result<()> {
    fs::create_dir_all(configuration_directory)?;
    let text = serde_json::to_string_pretty(settings).map_err(io::Error::other)?;
    fs::write(settings_path(configuration_directory), text)
}

/// Returns the data folder of the app with `settings`, given the data folder of the profile.
pub fn data_directory(
    settings: &LocationSettings,
    profile_data_directory: &Path,
) -> io::Result<PathBuf> {
    if settings.portable {
        portable_data_directory()
    } else {
        Ok(profile_data_directory.to_path_buf())
    }
}

/// Returns the `data` folder next to the running app.
pub fn portable_data_directory() -> io::Result<PathBuf> {
    let executable = std::env::current_exe()?;
    let appimage = std::env::var_os(APPIMAGE_VARIABLE).map(PathBuf::from);
    portable_data_directory_for(&executable, appimage.as_deref())
}

/// Returns the `data` folder next to the app that `executable` belongs to.
fn portable_data_directory_for(executable: &Path, appimage: Option<&Path>) -> io::Result<PathBuf> {
    app_directory(executable, appimage)
        .map(|directory| directory.join(DATA_DIRECTORY_NAME))
        .ok_or_else(|| io::Error::other("The folder of the app is unknown."))
}

/// Returns the folder that the user sees the app in.
fn app_directory(executable: &Path, appimage: Option<&Path>) -> Option<PathBuf> {
    if let Some(appimage) = appimage {
        return appimage.parent().map(Path::to_path_buf);
    }
    let executable_directory = executable.parent()?;
    // The executable of a macOS app sits in `Name.app/Contents/MacOS`, and the user sees the
    // folder that holds `Name.app`.
    if executable_directory.ends_with("Contents/MacOS") {
        let bundle = executable_directory.parent()?.parent()?;
        if bundle
            .extension()
            .is_some_and(|extension| extension == "app")
        {
            return bundle.parent().map(Path::to_path_buf);
        }
    }
    Some(executable_directory.to_path_buf())
}

/// Copies the data of a change of location into `data_directory`, then clears the request.
///
/// A failed copy is logged, and the request is cleared anyway. The old folder keeps its files,
/// so the app can start and the user can change the location again.
pub fn apply_pending_move(configuration_directory: &Path, data_directory: &Path) -> io::Result<()> {
    let mut settings = load_settings(configuration_directory);
    let Some(source_directory) = settings.pending_move_from.take() else {
        return Ok(());
    };
    if source_directory.as_path() != data_directory
        && let Err(error) = copy_directory_contents(&source_directory, data_directory)
    {
        tracing::error!(%error, "Could not copy the data to the new location.");
    }
    save_settings(configuration_directory, &settings)
}

/// Copies the files and folders of `source` into `target`, and replaces the files that exist in
/// both.
pub fn copy_directory_contents(source: &Path, target: &Path) -> io::Result<()> {
    if !source.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target_path = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory_contents(&entry.path(), &target_path)?;
        } else {
            fs::copy(entry.path(), target_path)?;
        }
    }
    Ok(())
}

/// Checks that the app can write files into `directory`, and creates the folder when it is
/// missing.
pub fn ensure_writable(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let check_path = directory.join(WRITE_CHECK_FILE_NAME);
    fs::write(&check_path, b"")?;
    fs::remove_file(check_path)
}

/// Returns what the app knows about the data location.
#[tauri::command]
#[specta::specta]
pub fn data_location_status(app: AppHandle) -> Result<DataLocationStatus, CommandError> {
    let settings = load_settings(&configuration_directory(&app)?);
    Ok(DataLocationStatus {
        question_pending: is_portable_build() && !settings.answered,
        portable: settings.portable,
        restart_pending: settings.pending_move_from.is_some(),
    })
}

/// Saves whether the data sits next to the app. The data moves at the next start.
#[tauri::command]
#[specta::specta]
pub fn set_portable_mode(app: AppHandle, enabled: bool) -> Result<(), CommandError> {
    let configuration_directory = configuration_directory(&app)?;
    let profile_data_directory = profile_data_directory(&app)?;
    let mut settings = load_settings(&configuration_directory);
    let current_directory =
        data_directory(&settings, &profile_data_directory).map_err(location_failure)?;

    settings.portable = enabled;
    settings.answered = true;
    let target_directory =
        data_directory(&settings, &profile_data_directory).map_err(location_failure)?;
    if enabled {
        ensure_writable(&target_directory).map_err(location_failure)?;
    }
    // A request that is still waiting for a restart keeps the folder it started from, because the
    // data of the running app is still in that folder.
    if current_directory != target_directory && settings.pending_move_from.is_none() {
        settings.pending_move_from = Some(current_directory);
    }
    save_settings(&configuration_directory, &settings).map_err(location_failure)
}

/// Restarts the app, so that a changed data location takes effect.
#[tauri::command]
#[specta::specta]
pub fn restart_app(app: AppHandle) {
    app.restart();
}

/// Returns the configuration folder of the user profile.
fn configuration_directory(app: &AppHandle) -> Result<PathBuf, CommandError> {
    app.path().app_config_dir().map_err(location_failure)
}

/// Returns the data folder of the user profile.
fn profile_data_directory(app: &AppHandle) -> Result<PathBuf, CommandError> {
    app.path().app_data_dir().map_err(location_failure)
}

/// Turns a failure of the data location into the error of the frontend.
fn location_failure(error: impl Display) -> CommandError {
    CommandError::DataLocation {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns an empty folder that is unique to the test, so parallel tests do not share files.
    fn empty_directory(test_name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("btt-portable-{}-{test_name}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn missing_settings_give_the_defaults() {
        let directory = empty_directory("missing-settings");
        assert_eq!(load_settings(&directory), LocationSettings::default());
    }

    #[test]
    fn unreadable_settings_give_the_defaults() {
        let directory = empty_directory("unreadable-settings");
        fs::write(settings_path(&directory), "not json").unwrap();
        assert_eq!(load_settings(&directory), LocationSettings::default());
    }

    #[test]
    fn saved_settings_load_again() {
        let directory = empty_directory("saved-settings");
        let settings = LocationSettings {
            portable: true,
            answered: true,
            pending_move_from: Some(PathBuf::from("/old/data")),
        };
        save_settings(&directory, &settings).unwrap();
        assert_eq!(load_settings(&directory), settings);
    }

    #[test]
    fn app_directory_of_a_plain_executable_is_its_folder() {
        let executable = Path::new("/apps/BigTrainTrip/btt-app.exe");
        assert_eq!(
            app_directory(executable, None),
            Some(PathBuf::from("/apps/BigTrainTrip"))
        );
    }

    #[test]
    fn app_directory_of_a_macos_bundle_is_the_folder_of_the_bundle() {
        let executable = Path::new("/Applications/BigTrainTrip.app/Contents/MacOS/btt-app");
        assert_eq!(
            app_directory(executable, None),
            Some(PathBuf::from("/Applications"))
        );
    }

    #[test]
    fn app_directory_of_an_appimage_is_the_folder_of_the_file() {
        let executable = Path::new("/tmp/.mount_BigTraXYZ/usr/bin/btt-app");
        let appimage = Path::new("/home/user/Apps/BigTrainTrip.AppImage");
        assert_eq!(
            app_directory(executable, Some(appimage)),
            Some(PathBuf::from("/home/user/Apps"))
        );
    }

    #[test]
    fn copy_replaces_files_and_keeps_the_folders() {
        let source = empty_directory("copy-source");
        let target = empty_directory("copy-target");
        fs::create_dir_all(source.join("bundles")).unwrap();
        fs::write(source.join("btt.sqlite3"), "new").unwrap();
        fs::write(source.join("bundles/timetable.bundle"), "bundle").unwrap();
        fs::write(target.join("btt.sqlite3"), "old").unwrap();

        copy_directory_contents(&source, &target).unwrap();

        assert_eq!(
            fs::read_to_string(target.join("btt.sqlite3")).unwrap(),
            "new"
        );
        assert_eq!(
            fs::read_to_string(target.join("bundles/timetable.bundle")).unwrap(),
            "bundle"
        );
    }

    #[test]
    fn copy_from_a_missing_folder_does_nothing() {
        let source = empty_directory("copy-missing").join("absent");
        let target = empty_directory("copy-missing-target");
        copy_directory_contents(&source, &target).unwrap();
        assert!(!target.join("btt.sqlite3").exists());
    }

    #[test]
    fn pending_move_copies_the_data_and_clears_the_request() {
        let configuration = empty_directory("move-configuration");
        let source = empty_directory("move-source");
        let target = empty_directory("move-target");
        fs::write(source.join("btt.sqlite3"), "trips").unwrap();
        let settings = LocationSettings {
            portable: true,
            answered: true,
            pending_move_from: Some(source),
        };
        save_settings(&configuration, &settings).unwrap();

        apply_pending_move(&configuration, &target).unwrap();

        assert_eq!(
            fs::read_to_string(target.join("btt.sqlite3")).unwrap(),
            "trips"
        );
        assert_eq!(load_settings(&configuration).pending_move_from, None);
    }

    #[test]
    fn ensure_writable_creates_the_folder_and_leaves_no_file() {
        let directory = empty_directory("writable").join("data");
        ensure_writable(&directory).unwrap();
        assert!(directory.is_dir());
        assert!(!directory.join(WRITE_CHECK_FILE_NAME).exists());
    }
}
