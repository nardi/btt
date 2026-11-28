//! Writing a trip snapshot as a link for the web viewer or as a file.

use btt_model::{
    CommandError, LinkAlphabet, SNAPSHOT_SCHEMA_VERSION, SnapshotLinkError, StoredSnapshot, Trip,
    TripFileError, TripSnapshot, encode_snapshot_link_in,
};
use serde::Serialize;
use specta::Type;

/// Web address of the viewer, which GitHub Pages serves from the repository.
const VIEWER_URL: &str = "https://nardi.github.io/btt/";

/// Link length in characters above which some messengers cut the link.
const LONG_LINK_CHARACTERS: usize = 8 * 1024;

/// A trip written as a link for the viewer.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SharedTrip {
    /// The whole web address, with the trip after the `#`.
    pub url: String,
    /// Length of the address in characters.
    pub link_characters: u32,
    /// Whether the address is long enough that some messengers cut it.
    pub long_for_messengers: bool,
    /// The same trip as the text of a QR code, written in the alphabet of the alphanumeric mode of
    /// a QR code, which keeps the code smaller than the link would. `None` when the trip is too
    /// big for it.
    pub qr: Option<QrText>,
}

/// What a QR code holds: the web address of the viewer and the trip.
#[derive(Debug, Clone, Serialize, Type)]
pub struct QrText {
    /// The web address of the viewer with the `#`, which goes in a segment of bytes.
    pub address: String,
    /// The trip, which goes in a segment of alphanumeric characters.
    pub payload: String,
}

/// What a link holds beyond the chosen journey of every leg, and how it is written. Each field
/// is one checkbox of the share dialog.
#[derive(Debug, Clone, Copy)]
// The dialog has exactly these four choices, so a struct of flags is the clearest form.
#[allow(clippy::struct_excessive_bools)]
pub struct LinkOptions {
    /// Whether the link holds the alternative journeys of every leg, which makes it about three
    /// times as long.
    pub include_alternatives: bool,
    /// Whether the link holds the price estimates of the journeys.
    pub include_prices: bool,
    /// Whether the link leaves out the names that the city dictionary cannot shorten, and the
    /// viewer then names those places by their nearest station.
    pub name_places_by_station: bool,
    /// Whether the link is written in a bigger alphabet and has less than half the characters,
    /// although a browser keeps it percent-encoded.
    pub compact: bool,
}

/// Writes a snapshot as a link for the web viewer.
///
/// The link holds the trip after the `#`, so a browser never sends it to a server. It holds no
/// traveller profiles.
///
/// # Errors
///
/// Returns an error when the trip has more journeys than a link can carry.
pub fn share_snapshot(
    snapshot: TripSnapshot,
    options: LinkOptions,
) -> Result<SharedTrip, CommandError> {
    let mut snapshot = snapshot;
    if !options.include_alternatives {
        snapshot = snapshot.chosen_journeys_only();
    }
    if !options.include_prices {
        snapshot = snapshot.without_prices();
    }
    if options.name_places_by_station {
        snapshot = snapshot.without_place_names();
    }
    let write = |alphabet| {
        encode_snapshot_link_in(&snapshot, alphabet).map_err(|error| match error {
            SnapshotLinkError::TooLarge => CommandError::TripTooLargeForLink,
            other => CommandError::Sharing {
                message: other.to_string(),
            },
        })
    };
    let alphabet = if options.compact {
        LinkAlphabet::Compact
    } else {
        LinkAlphabet::Fragment
    };
    let link = write(alphabet)?;
    let qr_link = write(LinkAlphabet::QrCode).ok();
    let url = format!("{VIEWER_URL}#{}", link.payload);
    let link_characters = u32::try_from(url.chars().count()).unwrap_or(u32::MAX);
    Ok(SharedTrip {
        long_for_messengers: url.chars().count() > LONG_LINK_CHARACTERS,
        url,
        link_characters,
        qr: qr_link.map(|qr_link| QrText {
            address: format!("{VIEWER_URL}#"),
            payload: qr_link.payload,
        }),
    })
}

/// Returns the name of the `.btt` file for a trip name.
pub fn trip_file_name(trip_name: &str) -> String {
    format!("{}.btt", file_stem(trip_name))
}

/// Writes a snapshot and the trip it was made from as the JSON text of a `.btt` file.
///
/// The viewer shows the snapshot. The app opens the trip as a new trip to edit, see
/// `trip_of_file_json`.
///
/// # Errors
///
/// Returns an error when the snapshot cannot be written as JSON.
pub fn trip_file_json(snapshot: TripSnapshot, trip: &Trip) -> Result<String, CommandError> {
    let document = StoredSnapshot::of_trip(snapshot, trip);
    serde_json::to_string(&document).map_err(|error| CommandError::Sharing {
        message: error.to_string(),
    })
}

/// The schema version of a `.btt` file, without the rest of the document.
#[derive(serde::Deserialize)]
struct SchemaVersion {
    /// See `StoredSnapshot::schema_version`.
    schema_version: u16,
}

/// Largest `.btt` file in bytes that the app reads, which is the limit of the viewer.
const MAXIMUM_TRIP_FILE_BYTES: usize = 8_000_000;

/// Reads the JSON text of a `.btt` file as a new trip with a new ID.
///
/// The trip has the settings and the picked journeys of the file. It has no travellers, because
/// the file has no traveller profiles. A file from a version that did not write the settings
/// gives the trip with default settings, and its planned journeys become picked journeys.
///
/// # Errors
///
/// Returns `CommandError::TripFileUnreadable` when the text is not a trip file, or when a newer
/// version of the app wrote it.
pub fn trip_of_file_json(contents: &str) -> Result<Trip, CommandError> {
    let unreadable = |message: String| CommandError::TripFileUnreadable { message };
    if contents.len() > MAXIMUM_TRIP_FILE_BYTES {
        return Err(unreadable("the file is too large".to_owned()));
    }
    // The version is read first, so that a file from a newer version gets a clear message even
    // when the rest of its layout changed.
    let version: SchemaVersion = serde_json::from_str(contents)
        .map_err(|_| unreadable("the file is not a trip file".to_owned()))?;
    if version.schema_version > SNAPSHOT_SCHEMA_VERSION {
        return Err(unreadable(
            TripFileError::NewerSchema(version.schema_version).to_string(),
        ));
    }
    let document: StoredSnapshot = serde_json::from_str(contents)
        .map_err(|error| unreadable(format!("the file is not a trip file ({error})")))?;
    document
        .into_editable_trip()
        .map_err(|error| unreadable(error.to_string()))
}

/// Returns a trip name as the name of a file, without the characters file systems reject.
fn file_stem(name: &str) -> String {
    let stem: String = name
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == ' ' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect();
    let stem = stem.trim();
    if stem.is_empty() {
        "trip".to_owned()
    } else {
        stem.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_name_drops_the_characters_file_systems_reject() {
        assert_eq!(file_stem("Wien / Roma: 2026"), "Wien _ Roma_ 2026");
        assert_eq!(file_stem("  "), "trip");
        assert_eq!(file_stem("Zürich"), "Zürich");
    }

    #[test]
    fn a_text_that_is_not_a_trip_file_is_unreadable() {
        for contents in ["", "not json", "{}", r#"{"schema_version": 2}"#] {
            assert!(matches!(
                trip_of_file_json(contents),
                Err(CommandError::TripFileUnreadable { .. })
            ));
        }
    }

    #[test]
    fn a_file_from_a_newer_version_names_the_version() {
        let contents = format!(
            r#"{{"schema_version": {}, "layout": "new"}}"#,
            SNAPSHOT_SCHEMA_VERSION + 1
        );
        let Err(CommandError::TripFileUnreadable { message }) = trip_of_file_json(&contents) else {
            panic!("A file from a newer version must be refused.");
        };
        assert!(message.contains("newer version"), "{message}");
    }

    #[test]
    fn a_file_larger_than_the_limit_is_unreadable() {
        let contents = " ".repeat(MAXIMUM_TRIP_FILE_BYTES + 1);
        assert!(matches!(
            trip_of_file_json(&contents),
            Err(CommandError::TripFileUnreadable { .. })
        ));
    }
}
