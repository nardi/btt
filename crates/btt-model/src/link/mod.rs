//! The link that carries a snapshot in the fragment of a URL.
//!
//! The encoder writes the snapshot as a bit layout that names no fields, compresses it with raw
//! deflate, writes the bytes as one big number in an alphabet of the 78 characters a URL fragment
//! may hold, and ends the text with two check characters. The web viewer reads the same format.

mod bits;
mod codec;
mod radix;
pub mod vocabulary;

use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use thiserror::Error;

use crate::snapshot::TripSnapshot;

/// The character that starts a payload written in the 78-character fragment alphabet.
pub const FRAGMENT_ALPHABET_MARKER: char = 'a';

/// Most characters a payload may have. A longer text is not a link this app wrote, and reading
/// it would take long.
pub const MAXIMUM_PAYLOAD_CHARACTERS: usize = 32_768;

/// Most bytes a payload may inflate to, which stops a small link from filling the memory.
pub const MAXIMUM_INFLATED_BYTES: usize = 1 << 20;

/// Why a link could not be written or read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SnapshotLinkError {
    /// The data ends before the snapshot does.
    #[error("the link ends too early")]
    Truncated,
    /// The data holds something that no snapshot holds.
    #[error("the link is damaged: {0}")]
    Malformed(&'static str),
    /// The check characters do not match, so the link was cut or changed on the way.
    #[error("the link was cut or changed")]
    Cut,
    /// A newer app wrote the snapshot, with a layout this build does not know.
    #[error("the link comes from a newer version, with schema version {0}")]
    NewerSchema(u16),
    /// The snapshot has more than a link can carry.
    #[error("the trip is too big for a link")]
    TooLarge,
}

/// A snapshot written as the payload of a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotLink {
    /// The text after the `#` of the link.
    pub payload: String,
    /// Bytes of the snapshot after compression, before they are written in the alphabet.
    pub compressed_bytes: usize,
}

/// Writes a snapshot as the payload of a link.
///
/// # Errors
///
/// Returns an error when the snapshot has more stations in one leg than a journey can point at,
/// or when its link would be longer than the viewer reads.
pub fn encode_snapshot_link(snapshot: &TripSnapshot) -> Result<SnapshotLink, SnapshotLinkError> {
    let bits = codec::write_snapshot(snapshot)?;
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(&bits)
        .map_err(|_| SnapshotLinkError::TooLarge)?;
    let compressed = encoder.finish().map_err(|_| SnapshotLinkError::TooLarge)?;
    let mut payload = String::from(FRAGMENT_ALPHABET_MARKER);
    payload.push_str(&radix::encode_digits(&compressed, radix::FRAGMENT_ALPHABET));
    let check = radix::check_characters(&payload);
    payload.extend(check);
    if payload.len() > MAXIMUM_PAYLOAD_CHARACTERS {
        return Err(SnapshotLinkError::TooLarge);
    }
    Ok(SnapshotLink {
        payload,
        compressed_bytes: compressed.len(),
    })
}

/// Reads the payload of a link, with or without the `#` in front of it.
///
/// # Errors
///
/// Returns an error when the link was cut, comes from a newer version, or is not a snapshot.
pub fn decode_snapshot_link(payload: &str) -> Result<TripSnapshot, SnapshotLinkError> {
    let payload = payload.strip_prefix('#').unwrap_or(payload);
    if payload.len() > MAXIMUM_PAYLOAD_CHARACTERS {
        return Err(SnapshotLinkError::TooLarge);
    }
    // The alphabet is ASCII, so any other character means the text is no payload.
    if !payload.is_ascii() || payload.len() < 4 {
        return Err(SnapshotLinkError::Malformed("the link is not a payload"));
    }
    let (body, check) = payload.split_at(payload.len() - 2);
    if radix::check_characters(body).iter().collect::<String>() != check {
        return Err(SnapshotLinkError::Cut);
    }
    let digits = body
        .strip_prefix(FRAGMENT_ALPHABET_MARKER)
        .ok_or(SnapshotLinkError::Malformed("the alphabet is unknown"))?;
    let compressed = radix::decode_digits(digits, radix::FRAGMENT_ALPHABET)?;
    let mut bits = Vec::new();
    DeflateDecoder::new(compressed.as_slice())
        .take(MAXIMUM_INFLATED_BYTES as u64 + 1)
        .read_to_end(&mut bits)
        .map_err(|_| SnapshotLinkError::Malformed("the data is not compressed"))?;
    if bits.len() > MAXIMUM_INFLATED_BYTES {
        return Err(SnapshotLinkError::TooLarge);
    }
    codec::read_snapshot(&bits)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::link::bits::BitWriter;

    // The fixtures of the integration tests, which name this crate by its package name.
    #[path = "../../../tests/common/mod.rs"]
    #[allow(dead_code)]
    mod fixtures;

    /// Wraps bytes as a payload the way the encoder does, so the checks of the outer layers pass
    /// and the bytes themselves meet the reader.
    fn payload_of_bytes(bytes: &[u8]) -> String {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(bytes).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut payload = String::from(FRAGMENT_ALPHABET_MARKER);
        payload.push_str(&radix::encode_digits(&compressed, radix::FRAGMENT_ALPHABET));
        let check = radix::check_characters(&payload);
        payload.extend(check);
        payload
    }

    #[test]
    fn a_newer_schema_version_is_named_as_such() {
        let mut writer = BitWriter::default();
        writer.number(2);
        writer.number(99);
        let payload = payload_of_bytes(&writer.finish());
        assert_eq!(
            decode_snapshot_link(&payload),
            Err(SnapshotLinkError::NewerSchema(2))
        );
    }

    #[test]
    fn a_payload_that_inflates_past_the_cap_is_refused() {
        let payload = payload_of_bytes(&vec![0; MAXIMUM_INFLATED_BYTES + 10]);
        assert_eq!(
            decode_snapshot_link(&payload),
            Err(SnapshotLinkError::TooLarge)
        );
    }

    #[test]
    fn a_count_that_the_data_cannot_hold_is_refused() {
        // Version 1, a date, a bundle version, a trip name of a million bytes and no data.
        let mut writer = BitWriter::default();
        writer.number(1);
        writer.number(20_000);
        writer.number(1);
        writer.number(1_000_000);
        let payload = payload_of_bytes(&writer.finish());
        assert!(decode_snapshot_link(&payload).is_err());
    }

    proptest! {
        #[test]
        fn random_bytes_never_panic_the_reader(bytes in proptest::collection::vec(any::<u8>(), 0..300)) {
            let _ = decode_snapshot_link(&payload_of_bytes(&bytes));
        }

        #[test]
        fn random_text_never_panics_the_decoder(text in "\\PC{0,200}") {
            let _ = decode_snapshot_link(&text);
        }

        #[test]
        fn a_damaged_snapshot_never_panics_the_reader(
            flips in proptest::collection::vec((any::<prop::sample::Index>(), 0..8u32), 1..8),
            cut in any::<prop::sample::Index>(),
        ) {
            let snapshot = fixtures::demo_snapshot();
            let mut bits = codec::write_snapshot(&snapshot).unwrap();
            for (index, bit) in flips {
                let position = index.index(bits.len());
                bits[position] ^= 1 << bit;
            }
            bits.truncate(cut.index(bits.len() + 1));
            let _ = codec::read_snapshot(&bits);
        }
    }
}
