//! Writes bytes as one big number in an alphabet, and the check characters of a link.
//!
//! A chunked encoding such as base64 needs a radix that is a power of two. Writing the whole
//! byte string as one number has no such limit, so the 78 characters that RFC 3986 allows in a
//! URL fragment (except the apostrophe) each carry log2(78) = 6.29 bits.

use super::SnapshotLinkError;

/// The digits of the fragment alphabet, in order of value: the digits, the letters, then the
/// sixteen symbols a URL fragment may hold without percent-encoding.
pub(super) const FRAGMENT_ALPHABET: &[u8; 78] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz-._~!$&()*+,;=:@";

/// The letters and digits the two check characters are written in. Chat apps and Markdown
/// linkifiers cut punctuation off the end of a link, and never a letter or a digit.
const CHECK_ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Number of values the two check characters hold.
const CHECK_VALUES: u32 = 62 * 62;

/// Writes bytes as digits of an alphabet. A leading one byte goes in front of the bytes, so
/// leading zero bytes survive.
pub(super) fn encode_digits(bytes: &[u8], alphabet: &[u8]) -> String {
    let radix = alphabet.len() as u64;
    // The number as 32-bit limbs, lowest first, with the sentinel byte on top.
    let mut limbs: Vec<u32> = Vec::with_capacity(bytes.len() / 4 + 2);
    let mut padded = vec![1_u8];
    padded.extend_from_slice(bytes);
    for chunk in padded.rchunks(4) {
        let mut limb = 0_u32;
        for byte in chunk {
            limb = (limb << 8) | u32::from(*byte);
        }
        limbs.push(limb);
    }
    let mut digits: Vec<u8> = Vec::new();
    while limbs.iter().any(|limb| *limb != 0) {
        let mut remainder = 0_u64;
        for limb in limbs.iter_mut().rev() {
            let value = (remainder << 32) | u64::from(*limb);
            // The quotient of a value below radix * 2^32 by the radix fits in 32 bits.
            #[allow(clippy::cast_possible_truncation)]
            {
                *limb = (value / radix) as u32;
            }
            remainder = value % radix;
        }
        // The remainder is below the radix, which is below 256.
        #[allow(clippy::cast_possible_truncation)]
        digits.push(alphabet[remainder as usize]);
    }
    digits.reverse();
    String::from_utf8(digits).unwrap_or_default()
}

/// Reads digits of an alphabet back into the bytes `encode_digits` wrote.
pub(super) fn decode_digits(digits: &str, alphabet: &[u8]) -> Result<Vec<u8>, SnapshotLinkError> {
    let radix = alphabet.len() as u64;
    let mut limbs: Vec<u32> = Vec::new();
    for character in digits.bytes() {
        let value = alphabet
            .iter()
            .position(|candidate| *candidate == character)
            .ok_or(SnapshotLinkError::Malformed(
                "a character is not in the alphabet",
            ))?;
        let mut carry = value as u64;
        for limb in &mut limbs {
            let product = u64::from(*limb) * radix + carry;
            *limb = (product & 0xffff_ffff) as u32;
            carry = product >> 32;
        }
        if carry != 0 {
            limbs.push(u32::try_from(carry).unwrap_or(0));
        }
    }
    let mut bytes: Vec<u8> = limbs.iter().flat_map(|limb| limb.to_le_bytes()).collect();
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    // The sentinel is the highest byte that is left, and it is a one.
    if bytes.pop() != Some(1) {
        return Err(SnapshotLinkError::Malformed(
            "the number has no leading marker",
        ));
    }
    bytes.reverse();
    Ok(bytes)
}

/// Returns the two check characters of a text. They hold the FNV-1a hash of the text, reduced to
/// 3,844 values, so a cut or damaged link fails the check in all but one case in 3,844.
pub(super) fn check_characters(text: &str) -> [char; 2] {
    let mut hash = 0x811c_9dc5_u32;
    for byte in text.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    let value = hash % CHECK_VALUES;
    [
        char::from(CHECK_ALPHABET[(value / 62) as usize]),
        char::from(CHECK_ALPHABET[(value % 62) as usize]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_round_trip_through_the_alphabet() {
        for bytes in [
            vec![],
            vec![0],
            vec![0, 0, 0, 7],
            vec![255; 33],
            (0..=255).collect::<Vec<u8>>(),
        ] {
            let digits = encode_digits(&bytes, FRAGMENT_ALPHABET);
            assert_eq!(decode_digits(&digits, FRAGMENT_ALPHABET).unwrap(), bytes);
        }
    }

    #[test]
    fn the_digits_use_only_characters_a_fragment_allows() {
        let digits = encode_digits(&(0..=255).collect::<Vec<u8>>(), FRAGMENT_ALPHABET);
        assert!(digits.bytes().all(|byte| FRAGMENT_ALPHABET.contains(&byte)));
        assert!(!digits.contains('\''));
    }

    #[test]
    fn a_character_outside_the_alphabet_fails() {
        assert!(decode_digits("ab c", FRAGMENT_ALPHABET).is_err());
    }

    #[test]
    fn the_check_characters_are_letters_or_digits_and_depend_on_the_text() {
        let first = check_characters("abc");
        assert!(first.iter().all(char::is_ascii_alphanumeric));
        assert_ne!(first, check_characters("abd"));
    }
}
