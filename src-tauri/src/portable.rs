//! Portable mode keeps the data of the app in a folder next to the app, instead of in the
//! profile of the user.

use std::path::{Path, PathBuf};

/// Name of the file that turns on portable mode when it sits next to the app.
const MARKER_FILE_NAME: &str = "portable.txt";

/// Name of the folder next to the marker file that holds the data in portable mode.
const DATA_DIRECTORY_NAME: &str = "data";

/// Environment variable that the AppImage runtime sets to the path of the `.AppImage` file.
/// The executable itself runs from a temporary mount, so its own location says nothing.
const APPIMAGE_VARIABLE: &str = "APPIMAGE";

/// Returns the data directory of portable mode, or `None` when the app is installed.
///
/// Portable mode is on when `portable.txt` sits next to the app. Next to the app means beside
/// the executable on Windows and Linux, beside the `.app` bundle on macOS, and beside the
/// `.AppImage` file for an AppImage.
pub fn portable_data_directory() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let appimage = std::env::var_os(APPIMAGE_VARIABLE).map(PathBuf::from);
    data_directory_next_to_app(&executable, appimage.as_deref())
}

/// Returns the folder that holds the data when `portable.txt` sits next to the app.
fn data_directory_next_to_app(executable: &Path, appimage: Option<&Path>) -> Option<PathBuf> {
    let app_directory = app_directory(executable, appimage)?;
    app_directory
        .join(MARKER_FILE_NAME)
        .is_file()
        .then(|| app_directory.join(DATA_DIRECTORY_NAME))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates an empty folder that is unique to the test, so parallel tests do not share files.
    fn empty_directory(test_name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("btt-portable-{}-{test_name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    /// Creates a folder that holds the marker file.
    fn directory_with_marker(test_name: &str) -> PathBuf {
        let directory = empty_directory(test_name);
        std::fs::write(directory.join(MARKER_FILE_NAME), "").unwrap();
        directory
    }

    #[test]
    fn marker_beside_the_executable_turns_on_portable_mode() {
        let directory = directory_with_marker("beside-executable");
        let executable = directory.join("btt-app.exe");
        assert_eq!(
            data_directory_next_to_app(&executable, None),
            Some(directory.join("data"))
        );
    }

    #[test]
    fn no_marker_means_an_installed_app() {
        let directory = empty_directory("no-marker");
        let executable = directory.join("btt-app.exe");
        assert_eq!(data_directory_next_to_app(&executable, None), None);
    }

    #[test]
    fn marker_beside_the_macos_bundle_turns_on_portable_mode() {
        let directory = directory_with_marker("beside-bundle");
        let executable = directory.join("BigTrainTrip.app/Contents/MacOS/btt-app");
        assert_eq!(
            data_directory_next_to_app(&executable, None),
            Some(directory.join("data"))
        );
    }

    #[test]
    fn marker_inside_the_macos_bundle_is_ignored() {
        let directory = empty_directory("inside-bundle");
        let macos_directory = directory.join("BigTrainTrip.app/Contents/MacOS");
        std::fs::create_dir_all(&macos_directory).unwrap();
        std::fs::write(macos_directory.join(MARKER_FILE_NAME), "").unwrap();
        let executable = macos_directory.join("btt-app");
        assert_eq!(data_directory_next_to_app(&executable, None), None);
    }

    #[test]
    fn appimage_uses_the_folder_of_the_appimage_file() {
        let directory = directory_with_marker("appimage");
        let appimage = directory.join("BigTrainTrip.AppImage");
        // The executable runs from a mount that has no marker.
        let executable = Path::new("/tmp/.mount_BigTraXYZ/usr/bin/btt-app");
        assert_eq!(
            data_directory_next_to_app(executable, Some(&appimage)),
            Some(directory.join("data"))
        );
    }
}
