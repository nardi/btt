//! Writing a trip snapshot as a link for the web viewer or as a file.

use btt_model::{
    CommandError, LinkAlphabet, SNAPSHOT_SCHEMA_VERSION, SnapshotLinkError, StoredSnapshot,
    TripSnapshot, encode_snapshot_link_in,
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

/// Writes a snapshot as the JSON text of a `.btt` file.
///
/// # Errors
///
/// Returns an error when the snapshot cannot be written as JSON.
pub fn trip_file_json(snapshot: TripSnapshot) -> Result<String, CommandError> {
    let document = StoredSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        snapshot,
    };
    serde_json::to_string(&document).map_err(|error| CommandError::Sharing {
        message: error.to_string(),
    })
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
}
