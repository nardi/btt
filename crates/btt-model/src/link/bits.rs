//! A bit-level writer and reader for the snapshot encoding.
//!
//! Bits fill each byte from the lowest to the highest, and a value of several bits starts with its
//! lowest bit. The web viewer reads the same layout.

use super::SnapshotLinkError;

/// Most 7-bit groups a number may have, which holds values up to 2^49. The viewer reads numbers
/// as floating point values, which are exact up to 2^53.
const MAXIMUM_NUMBER_GROUPS: u32 = 7;

/// Writes bits into a byte vector.
#[derive(Debug, Default)]
pub(super) struct BitWriter {
    /// The bytes written so far. The last byte may be partly filled.
    bytes: Vec<u8>,
    /// Number of bits written.
    bit_count: usize,
}

impl BitWriter {
    /// Writes one bit.
    pub(super) fn bit(&mut self, value: bool) {
        if self.bit_count.is_multiple_of(8) {
            self.bytes.push(0);
        }
        if value && let Some(last) = self.bytes.last_mut() {
            *last |= 1 << (self.bit_count % 8);
        }
        self.bit_count += 1;
    }

    /// Writes the lowest `width` bits of a value, lowest bit first.
    pub(super) fn bits(&mut self, value: u32, width: u32) {
        for position in 0..width {
            self.bit((value >> position) & 1 == 1);
        }
    }

    /// Writes a number as groups of 7 bits, lowest group first, each after a bit that says
    /// whether another group follows.
    pub(super) fn number(&mut self, mut value: u64) {
        debug_assert!(value < 1 << (7 * MAXIMUM_NUMBER_GROUPS), "number too large");
        loop {
            // The mask keeps the group within 7 bits.
            #[allow(clippy::cast_possible_truncation)]
            let group = (value & 0x7f) as u32;
            value >>= 7;
            self.bit(value != 0);
            self.bits(group, 7);
            if value == 0 {
                break;
            }
        }
    }

    /// Writes a signed number, with small magnitudes of either sign in few bits.
    pub(super) fn signed_number(&mut self, value: i64) {
        // The zigzag mapping sends 0, -1, 1, -2 to 0, 1, 2, 3.
        self.number(((value << 1) ^ (value >> 63)).cast_unsigned());
    }

    /// Writes a length and then the bytes of a text.
    pub(super) fn text(&mut self, text: &str) {
        self.number(text.len() as u64);
        for byte in text.bytes() {
            self.bits(u32::from(byte), 8);
        }
    }

    /// Returns the bytes, with the last byte padded with zero bits.
    pub(super) fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

/// Reads bits from a byte slice, and fails instead of reading past its end.
#[derive(Debug)]
pub(super) struct BitReader<'a> {
    /// The bytes to read.
    bytes: &'a [u8],
    /// Number of bits read.
    position: usize,
}

impl<'a> BitReader<'a> {
    /// Starts reading at the first bit.
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    /// Returns how many bits are left, including the padding of the last byte.
    pub(super) fn remaining_bits(&self) -> usize {
        self.bytes.len() * 8 - self.position
    }

    /// Reads one bit.
    pub(super) fn bit(&mut self) -> Result<bool, SnapshotLinkError> {
        let byte = self
            .bytes
            .get(self.position / 8)
            .ok_or(SnapshotLinkError::Truncated)?;
        let value = (byte >> (self.position % 8)) & 1 == 1;
        self.position += 1;
        Ok(value)
    }

    /// Reads `width` bits, lowest bit first.
    pub(super) fn bits(&mut self, width: u32) -> Result<u32, SnapshotLinkError> {
        let mut value = 0;
        for position in 0..width {
            value |= u32::from(self.bit()?) << position;
        }
        Ok(value)
    }

    /// Reads a number written by `BitWriter::number`.
    pub(super) fn number(&mut self) -> Result<u64, SnapshotLinkError> {
        let mut value = 0_u64;
        for group in 0..MAXIMUM_NUMBER_GROUPS {
            let more = self.bit()?;
            value |= u64::from(self.bits(7)?) << (7 * group);
            if !more {
                return Ok(value);
            }
        }
        Err(SnapshotLinkError::Malformed("a number is too large"))
    }

    /// Reads a number that has to fit in a `u32`.
    pub(super) fn number_u32(&mut self) -> Result<u32, SnapshotLinkError> {
        u32::try_from(self.number()?)
            .map_err(|_| SnapshotLinkError::Malformed("a number is too large"))
    }

    /// Reads a number that has to fit in a `u16`.
    pub(super) fn number_u16(&mut self) -> Result<u16, SnapshotLinkError> {
        u16::try_from(self.number()?)
            .map_err(|_| SnapshotLinkError::Malformed("a number is too large"))
    }

    /// Reads a signed number written by `BitWriter::signed_number`.
    pub(super) fn signed_number(&mut self) -> Result<i64, SnapshotLinkError> {
        let zigzag = self.number()?;
        Ok(i64::try_from(zigzag >> 1)
            .map_err(|_| SnapshotLinkError::Malformed("a number is too large"))?
            ^ -i64::from(zigzag & 1 == 1))
    }

    /// Reads the count of the items of a list. Every item takes at least one bit, so a count
    /// above the bits that are left cannot be true.
    pub(super) fn count(&mut self) -> Result<usize, SnapshotLinkError> {
        let count = usize::try_from(self.number()?)
            .map_err(|_| SnapshotLinkError::Malformed("a count is too large"))?;
        if count > self.remaining_bits() {
            return Err(SnapshotLinkError::Malformed(
                "a count is larger than the data",
            ));
        }
        Ok(count)
    }

    /// Reads a text written by `BitWriter::text`.
    pub(super) fn text(&mut self) -> Result<String, SnapshotLinkError> {
        let length = self.count()?;
        if length * 8 > self.remaining_bits() {
            return Err(SnapshotLinkError::Truncated);
        }
        let mut bytes = Vec::with_capacity(length);
        for _ in 0..length {
            // Eight bits always fit in a byte.
            #[allow(clippy::cast_possible_truncation)]
            bytes.push(self.bits(8)? as u8);
        }
        String::from_utf8(bytes).map_err(|_| SnapshotLinkError::Malformed("a text is not UTF-8"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_texts_round_trip() {
        let mut writer = BitWriter::default();
        writer.number(0);
        writer.number(127);
        writer.number(128);
        writer.number((1 << 49) - 1);
        writer.signed_number(-1);
        writer.signed_number(1_000_000);
        writer.signed_number(-52_378_902);
        writer.bit(true);
        writer.bits(5, 3);
        writer.text("Zürich HB");
        let bytes = writer.finish();
        let mut reader = BitReader::new(&bytes);
        assert_eq!(reader.number().unwrap(), 0);
        assert_eq!(reader.number().unwrap(), 127);
        assert_eq!(reader.number().unwrap(), 128);
        assert_eq!(reader.number().unwrap(), (1 << 49) - 1);
        assert_eq!(reader.signed_number().unwrap(), -1);
        assert_eq!(reader.signed_number().unwrap(), 1_000_000);
        assert_eq!(reader.signed_number().unwrap(), -52_378_902);
        assert!(reader.bit().unwrap());
        assert_eq!(reader.bits(3).unwrap(), 5);
        assert_eq!(reader.text().unwrap(), "Zürich HB");
    }

    #[test]
    fn reading_past_the_end_fails() {
        let mut reader = BitReader::new(&[0xff]);
        assert!(reader.bits(8).is_ok());
        assert_eq!(reader.bit(), Err(SnapshotLinkError::Truncated));
    }

    #[test]
    fn a_count_larger_than_the_data_fails() {
        let mut writer = BitWriter::default();
        writer.number(1_000);
        let bytes = writer.finish();
        assert!(BitReader::new(&bytes).count().is_err());
    }

    #[test]
    fn a_number_of_too_many_groups_fails() {
        // Eight groups that all say another one follows.
        let bytes = [0xff; 8];
        assert!(BitReader::new(&bytes).number().is_err());
    }
}
