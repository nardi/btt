//! The alphabet of the compact link: 44,190 letters of the Basic Multilingual Plane.
//!
//! The alphabet is the file `data/compact-alphabet.txt`, which lists its ranges in order as two
//! hexadecimal code points per line. The web viewer reads the same file. Every letter is one
//! UTF-16 unit and carries 15.4 bits, so a compact link is about 2.4 times shorter in characters
//! than a link in the fragment alphabet. Links depend on the exact alphabet, so the file never
//! changes. The script `tools/compact-alphabet.js` wrote it.

use std::sync::OnceLock;

use super::SnapshotLinkError;
use super::radix;

/// The alphabet file, compiled in.
const ALPHABET_FILE: &str = include_str!("../../data/compact-alphabet.txt");

/// A range of the alphabet.
#[derive(Debug, Clone, Copy)]
struct LetterRange {
    /// First code point of the range.
    first: u32,
    /// Last code point of the range.
    last: u32,
    /// Position in the alphabet of the first letter of the range.
    start: u32,
}

/// The letters of the alphabet file in order.
#[derive(Debug)]
struct Alphabet {
    ranges: Vec<LetterRange>,
    size: u32,
}

/// Returns the alphabet, which is parsed on first use.
fn alphabet() -> &'static Alphabet {
    static ALPHABET: OnceLock<Alphabet> = OnceLock::new();
    ALPHABET.get_or_init(|| {
        let mut ranges = Vec::new();
        let mut size = 0_u32;
        for line in ALPHABET_FILE.lines() {
            if line.starts_with('#') {
                continue;
            }
            if let Some((first, last)) = line.split_once('-')
                && let (Ok(first), Ok(last)) = (
                    u32::from_str_radix(first, 16),
                    u32::from_str_radix(last, 16),
                )
                && first <= last
            {
                ranges.push(LetterRange {
                    first,
                    last,
                    start: size,
                });
                size += last - first + 1;
            }
        }
        Alphabet { ranges, size }
    })
}

impl Alphabet {
    /// Returns the letter at a position.
    fn letter(&self, index: u32) -> Option<char> {
        let position = self.ranges.partition_point(|range| range.start <= index);
        let range = self.ranges.get(position.checked_sub(1)?)?;
        char::from_u32(range.first + (index - range.start))
            .filter(|letter| u32::from(*letter) <= range.last)
    }

    /// Returns the position of a letter, or `None` when the alphabet lacks it.
    fn index_of(&self, letter: char) -> Option<u32> {
        let code = u32::from(letter);
        let position = self.ranges.partition_point(|range| range.last < code);
        let range = self.ranges.get(position)?;
        (range.first <= code).then(|| range.start + (code - range.first))
    }
}

/// Returns the number of letters in the alphabet.
#[cfg(test)]
pub(super) fn size() -> u32 {
    alphabet().size
}

/// Writes bytes as letters of the compact alphabet.
pub(super) fn encode_letters(bytes: &[u8]) -> String {
    let alphabet = alphabet();
    radix::encode_values(bytes, alphabet.size)
        .into_iter()
        .filter_map(|digit| alphabet.letter(digit))
        .collect()
}

/// Reads letters of the compact alphabet back into the bytes `encode_letters` wrote.
pub(super) fn decode_letters(letters: &str) -> Result<Vec<u8>, SnapshotLinkError> {
    let alphabet = alphabet();
    let values = letters
        .chars()
        .map(|letter| {
            alphabet
                .index_of(letter)
                .ok_or(SnapshotLinkError::Malformed(
                    "a character is not in the alphabet",
                ))
        })
        .collect::<Result<Vec<u32>, _>>()?;
    radix::decode_values(&values, alphabet.size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_holds_forty_four_thousand_letters_that_are_in_order() {
        assert_eq!(size(), 44_190);
        let alphabet = alphabet();
        for pair in alphabet.ranges.windows(2) {
            assert!(pair[0].last < pair[1].first);
        }
        // Every letter is a letter of the Basic Multilingual Plane, outside the ASCII range and
        // the surrogates.
        assert!(
            alphabet
                .ranges
                .iter()
                .all(|range| range.first >= 0x80 && range.last <= 0xffff)
        );
        assert!(
            alphabet
                .ranges
                .iter()
                .all(|range| range.last < 0xd800 || range.first > 0xdfff)
        );
    }

    #[test]
    fn every_position_names_a_letter_that_names_the_position() {
        let alphabet = alphabet();
        for index in (0..alphabet.size).step_by(7) {
            let letter = alphabet.letter(index).unwrap();
            assert_eq!(alphabet.index_of(letter), Some(index));
        }
        assert_eq!(alphabet.letter(alphabet.size), None);
        assert_eq!(alphabet.index_of('a'), None);
    }

    #[test]
    fn bytes_round_trip_through_the_letters() {
        for bytes in [
            vec![],
            vec![0, 0, 9],
            vec![255; 50],
            (0..=255).collect::<Vec<u8>>(),
        ] {
            let letters = encode_letters(&bytes);
            assert!(letters.chars().all(|letter| u32::from(letter) <= 0xffff));
            assert_eq!(decode_letters(&letters).unwrap(), bytes);
        }
    }

    #[test]
    fn an_ascii_letter_is_not_in_the_alphabet() {
        assert!(decode_letters("abc").is_err());
    }
}
