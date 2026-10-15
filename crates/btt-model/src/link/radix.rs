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

/// The digits of the QR alphabet: the digits, the uppercase letters and the symbols `$*+-./:`.
/// These are the characters of the alphanumeric mode of a QR code that a URL may hold. The mode
/// also has the space and the percent sign, which a URL cannot hold as they are.
pub(super) const QR_ALPHABET: &[u8; 43] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ$*+-./:";

/// The letters and digits the two check characters of a fragment payload are written in. Chat
/// apps and Markdown linkifiers cut punctuation off the end of a link, and never a letter or a
/// digit.
pub(super) const FRAGMENT_CHECK_ALPHABET: &[u8] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The digits and uppercase letters the two check characters of a QR payload are written in.
pub(super) const QR_CHECK_ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// Writes bytes as one number and returns its digits in the given radix, most significant
/// first. A leading one byte goes in front of the bytes, so leading zero bytes survive.
pub(super) fn encode_values(bytes: &[u8], radix: u32) -> Vec<u32> {
    let radix = u64::from(radix);
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
    let mut digits: Vec<u32> = Vec::new();
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
        // The remainder is below the radix, which fits in 32 bits.
        #[allow(clippy::cast_possible_truncation)]
        digits.push(remainder as u32);
    }
    digits.reverse();
    digits
}

/// Reads digits in a radix, most significant first, back into the bytes `encode_values` wrote.
pub(super) fn decode_values(digits: &[u32], radix: u32) -> Result<Vec<u8>, SnapshotLinkError> {
    let radix = u64::from(radix);
    let mut limbs: Vec<u32> = Vec::new();
    for digit in digits {
        let mut carry = u64::from(*digit);
        for limb in &mut limbs {
            let product = u64::from(*limb) * radix + carry;
            *limb = u32::try_from(product & 0xffff_ffff).unwrap_or(0);
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

/// Writes bytes as digits of an ASCII alphabet.
pub(super) fn encode_digits(bytes: &[u8], alphabet: &[u8]) -> String {
    let radix = u32::try_from(alphabet.len()).unwrap_or(u32::MAX);
    encode_values(bytes, radix)
        .into_iter()
        .map(|digit| char::from(alphabet[digit as usize]))
        .collect()
}

/// Reads digits of an ASCII alphabet back into the bytes `encode_digits` wrote.
pub(super) fn decode_digits(digits: &str, alphabet: &[u8]) -> Result<Vec<u8>, SnapshotLinkError> {
    let values = digits
        .bytes()
        .map(|character| {
            alphabet
                .iter()
                .position(|candidate| *candidate == character)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or(SnapshotLinkError::Malformed(
                    "a character is not in the alphabet",
                ))
        })
        .collect::<Result<Vec<u32>, _>>()?;
    decode_values(&values, u32::try_from(alphabet.len()).unwrap_or(u32::MAX))
}

/// Returns the two check characters of a text. They hold the FNV-1a hash of the text, reduced to
/// the square of the size of the check alphabet, which is 3,844 for a fragment payload. A cut or
/// damaged link then fails the check in all but about one case in 3,844.
pub(super) fn check_characters(text: &str, check_alphabet: &[u8]) -> [char; 2] {
    let mut hash = 0x811c_9dc5_u32;
    for byte in text.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    // The check alphabets have at most 62 characters.
    let size = u32::try_from(check_alphabet.len()).unwrap_or(62);
    let value = hash % (size * size);
    [
        char::from(check_alphabet[(value / size) as usize]),
        char::from(check_alphabet[(value % size) as usize]),
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
    fn bytes_round_trip_through_the_qr_alphabet() {
        for bytes in [
            vec![],
            vec![0, 0, 9],
            vec![255; 40],
            (0..=255).collect::<Vec<u8>>(),
        ] {
            let digits = encode_digits(&bytes, QR_ALPHABET);
            assert!(digits.bytes().all(|byte| QR_ALPHABET.contains(&byte)));
            assert_eq!(decode_digits(&digits, QR_ALPHABET).unwrap(), bytes);
        }
    }

    #[test]
    fn a_character_outside_the_alphabet_fails() {
        assert!(decode_digits("ab c", FRAGMENT_ALPHABET).is_err());
    }

    #[test]
    fn the_check_characters_are_letters_or_digits_and_depend_on_the_text() {
        let first = check_characters("abc", FRAGMENT_CHECK_ALPHABET);
        assert!(first.iter().all(char::is_ascii_alphanumeric));
        assert_ne!(first, check_characters("abd", FRAGMENT_CHECK_ALPHABET));
        let qr = check_characters("ABC", QR_CHECK_ALPHABET);
        assert!(
            qr.iter()
                .all(|character| QR_CHECK_ALPHABET.contains(&(*character as u8)))
        );
    }
}
