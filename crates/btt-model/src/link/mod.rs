//! The link that carries a snapshot in the fragment of a URL, or in a QR code.
//!
//! The encoder writes the snapshot as a bit layout that names no fields. It keeps the bytes as
//! they are or compresses them with raw deflate, whichever is shorter. Then it writes them as
//! one big number in an alphabet and ends the text with two check characters. The first
//! character of the text names the alphabet and whether the bytes are compressed. The web viewer
//! reads the same format.
//!
//! A link for a chat uses the 78 characters a URL fragment may hold, or the 44,190 letters of the
//! compact alphabet for a shorter one. A link for a QR code uses
//! the 43 characters that a QR code packs two to 11 bits in its alphanumeric mode, which keeps
//! the code smaller than any byte-mode text could.

mod bits;
mod cities;
mod codec;
mod compact;
mod radix;
mod stations;
pub mod vocabulary;

use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use thiserror::Error;

use crate::snapshot::TripSnapshot;

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

/// The alphabet a payload is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkAlphabet {
    /// The 78 characters a URL fragment may hold, for a link that people send.
    Fragment,
    /// The 44,190 letters of the Basic Multilingual Plane, for a link that people send and that
    /// has to be short. Every letter carries 15.4 bits, so the link has less than half the
    /// characters. A browser keeps it percent-encoded, which makes it longer where an app shows
    /// it that way.
    Compact,
    /// The 43 characters of the alphanumeric mode of a QR code that a URL may hold.
    QrCode,
}

impl LinkAlphabet {
    /// Returns the letters and digits the two check characters are written in.
    fn check_alphabet(self) -> &'static [u8] {
        match self {
            Self::Fragment | Self::Compact => radix::FRAGMENT_CHECK_ALPHABET,
            Self::QrCode => radix::QR_CHECK_ALPHABET,
        }
    }

    /// Returns the first character of a payload with this alphabet, for bytes that are
    /// compressed or not.
    fn marker(self, compressed: bool) -> char {
        match (self, compressed) {
            (Self::Fragment, true) => 'a',
            (Self::Fragment, false) => 'b',
            (Self::Compact, true) => 'c',
            (Self::Compact, false) => 'd',
            (Self::QrCode, true) => 'Q',
            (Self::QrCode, false) => 'R',
        }
    }

    /// Returns the alphabet and whether the bytes are compressed for the first character of a
    /// payload.
    fn from_marker(marker: char) -> Option<(Self, bool)> {
        match marker {
            'a' => Some((Self::Fragment, true)),
            'b' => Some((Self::Fragment, false)),
            'c' => Some((Self::Compact, true)),
            'd' => Some((Self::Compact, false)),
            'Q' => Some((Self::QrCode, true)),
            'R' => Some((Self::QrCode, false)),
            _ => None,
        }
    }

    /// Writes bytes as digits of the alphabet.
    fn write_digits(self, bytes: &[u8]) -> String {
        match self {
            Self::Fragment => radix::encode_digits(bytes, radix::FRAGMENT_ALPHABET),
            Self::Compact => compact::encode_letters(bytes),
            Self::QrCode => radix::encode_digits(bytes, radix::QR_ALPHABET),
        }
    }

    /// Reads digits of the alphabet back into bytes.
    fn read_digits(self, digits: &str) -> Result<Vec<u8>, SnapshotLinkError> {
        match self {
            Self::Fragment => radix::decode_digits(digits, radix::FRAGMENT_ALPHABET),
            Self::Compact => compact::decode_letters(digits),
            Self::QrCode => radix::decode_digits(digits, radix::QR_ALPHABET),
        }
    }
}

/// A snapshot written as the payload of a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotLink {
    /// The text after the `#` of the link.
    pub payload: String,
    /// Bytes of the snapshot after compression, before they are written in the alphabet.
    pub compressed_bytes: usize,
}

/// Writes a snapshot as the payload of a link that people send, in the fragment alphabet.
///
/// # Errors
///
/// Returns an error when the snapshot has more stations in one leg than a journey can point at,
/// or when its link would be longer than the viewer reads.
pub fn encode_snapshot_link(snapshot: &TripSnapshot) -> Result<SnapshotLink, SnapshotLinkError> {
    encode_snapshot_link_in(snapshot, LinkAlphabet::Fragment)
}

/// Writes a snapshot as the payload of a link in an alphabet.
///
/// # Errors
///
/// Returns an error when the snapshot has more stations in one leg than a journey can point at,
/// or when its link would be longer than the viewer reads.
pub fn encode_snapshot_link_in(
    snapshot: &TripSnapshot,
    alphabet: LinkAlphabet,
) -> Result<SnapshotLink, SnapshotLinkError> {
    let bits = codec::write_snapshot(snapshot)?;
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(&bits)
        .map_err(|_| SnapshotLinkError::TooLarge)?;
    let deflated = encoder.finish().map_err(|_| SnapshotLinkError::TooLarge)?;
    // Deflate adds a few bytes of its own, which a snapshot of a few hundred bytes can lose.
    let compressed = deflated.len() < bits.len();
    let bytes = if compressed { deflated } else { bits };
    let mut payload = String::from(alphabet.marker(compressed));
    payload.push_str(&alphabet.write_digits(&bytes));
    let check = radix::check_characters(&payload, alphabet.check_alphabet());
    payload.extend(check);
    if payload.chars().count() > MAXIMUM_PAYLOAD_CHARACTERS {
        return Err(SnapshotLinkError::TooLarge);
    }
    Ok(SnapshotLink {
        payload,
        compressed_bytes: bytes.len(),
    })
}

/// Reads the payload of a link, with or without the `#` in front of it. A payload in the compact
/// alphabet may come as a browser keeps it, with its letters percent-encoded.
///
/// # Errors
///
/// Returns an error when the link was cut, comes from a newer version, or is not a snapshot.
pub fn decode_snapshot_link(payload: &str) -> Result<TripSnapshot, SnapshotLinkError> {
    let payload = payload.strip_prefix('#').unwrap_or(payload);
    // A letter takes at most 4 bytes in UTF-8 and 9 in a percent-encoded text.
    if payload.len() > 9 * MAXIMUM_PAYLOAD_CHARACTERS {
        return Err(SnapshotLinkError::TooLarge);
    }
    let decoded;
    let payload = if payload.contains('%') {
        decoded = percent_decode(payload)?;
        decoded.as_str()
    } else {
        payload
    };
    let characters: Vec<char> = payload.chars().collect();
    if characters.len() > MAXIMUM_PAYLOAD_CHARACTERS {
        return Err(SnapshotLinkError::TooLarge);
    }
    if characters.len() < 4 {
        return Err(SnapshotLinkError::Malformed("the link is not a payload"));
    }
    let (alphabet, compressed) = LinkAlphabet::from_marker(characters[0])
        .ok_or(SnapshotLinkError::Malformed("the alphabet is unknown"))?;
    let split = characters.len() - 2;
    let body: String = characters[..split].iter().collect();
    let check: String = characters[split..].iter().collect();
    let expected: String = radix::check_characters(&body, alphabet.check_alphabet())
        .iter()
        .collect();
    if expected != check {
        return Err(SnapshotLinkError::Cut);
    }
    let digits: String = characters[1..split].iter().collect();
    // Only the compact alphabet has letters outside ASCII, and the reading of the digits checks
    // every letter against its alphabet.
    let bytes = alphabet.read_digits(&digits)?;
    if !compressed {
        if bytes.len() > MAXIMUM_INFLATED_BYTES {
            return Err(SnapshotLinkError::TooLarge);
        }
        return codec::read_snapshot(&bytes);
    }
    let mut bits = Vec::new();
    DeflateDecoder::new(bytes.as_slice())
        .take(MAXIMUM_INFLATED_BYTES as u64 + 1)
        .read_to_end(&mut bits)
        .map_err(|_| SnapshotLinkError::Malformed("the data is not compressed"))?;
    if bits.len() > MAXIMUM_INFLATED_BYTES {
        return Err(SnapshotLinkError::TooLarge);
    }
    codec::read_snapshot(&bits)
}

/// Reads the percent-encoded bytes of a text as UTF-8, as `decodeURIComponent` does.
fn percent_decode(text: &str) -> Result<String, SnapshotLinkError> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut position = 0;
    while position < bytes.len() {
        if bytes[position] == b'%' {
            let digits = bytes
                .get(position + 1..position + 3)
                .and_then(|pair| std::str::from_utf8(pair).ok())
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or(SnapshotLinkError::Malformed("a percent sign has no code"))?;
            decoded.push(digits);
            position += 3;
        } else {
            decoded.push(bytes[position]);
            position += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| SnapshotLinkError::Malformed("the link is not UTF-8"))
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
        let alphabet = LinkAlphabet::Fragment;
        let mut payload = String::from(alphabet.marker(true));
        payload.push_str(&alphabet.write_digits(&compressed));
        let check = radix::check_characters(&payload, alphabet.check_alphabet());
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
