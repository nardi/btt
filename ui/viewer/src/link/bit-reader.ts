// Reads the bit layout that `crates/btt-model/src/link/bits.rs` writes.
//
// Bits fill each byte from the lowest to the highest, and a value of several bits starts with its
// lowest bit.

import { malformed } from "./errors";

/** Most 7-bit groups a number may have, which holds values below 2^49. Floats are exact there. */
const maximumNumberGroups = 7;

const textDecoder = new TextDecoder("utf-8", { fatal: true });

/** Reads bits from bytes, and throws instead of reading past the end. */
export class BitReader {
  private position = 0;

  constructor(private readonly bytes: Uint8Array) {}

  /** Returns how many bits are left, including the padding of the last byte. */
  remainingBits(): number {
    return this.bytes.length * 8 - this.position;
  }

  bit(): boolean {
    const byte = this.bytes[this.position >> 3];
    if (byte === undefined) {
      throw malformed("the link ends too early");
    }
    const value = ((byte >> (this.position & 7)) & 1) === 1;
    this.position += 1;
    return value;
  }

  /** Reads up to 31 bits, lowest bit first. */
  bits(width: number): number {
    let value = 0;
    for (let position = 0; position < width; position += 1) {
      if (this.bit()) {
        value += 2 ** position;
      }
    }
    return value;
  }

  /** Reads a number of 7-bit groups, each after a bit that says whether another follows. */
  number(): number {
    let value = 0;
    for (let group = 0; group < maximumNumberGroups; group += 1) {
      const more = this.bit();
      value += this.bits(7) * 2 ** (7 * group);
      if (!more) {
        return value;
      }
    }
    throw malformed("a number is too large");
  }

  /** Reads a number that has to be at most `maximum`. */
  boundedNumber(maximum: number, what: string): number {
    const value = this.number();
    if (value > maximum) {
      throw malformed(`${what} is out of range`);
    }
    return value;
  }

  /** Reads a signed number, which the writer maps to 0, 1, 2, 3 for 0, -1, 1, -2. */
  signedNumber(): number {
    const zigzag = this.number();
    const half = Math.floor(zigzag / 2);
    return zigzag % 2 === 1 ? -half - 1 : half;
  }

  /**
   * Reads the count of the items of a list. Every item takes at least one bit, so a count above
   * the bits that are left cannot be true.
   */
  count(): number {
    const count = this.number();
    if (count > this.remainingBits()) {
      throw malformed("a count is larger than the data");
    }
    return count;
  }

  /** Reads a length and then the bytes of a text. */
  text(): string {
    const length = this.count();
    if (length * 8 > this.remainingBits()) {
      throw malformed("the link ends too early");
    }
    const bytes = new Uint8Array(length);
    for (let index = 0; index < length; index += 1) {
      bytes[index] = this.bits(8);
    }
    try {
      return textDecoder.decode(bytes);
    } catch {
      throw malformed("a text is not UTF-8");
    }
  }
}
