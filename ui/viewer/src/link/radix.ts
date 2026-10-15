// Reads bytes written as one big number in an alphabet, and the check characters of a link.
//
// The Rust side is `crates/btt-model/src/link/radix.rs`.

import { malformed } from "./errors";

/** The 78 characters a URL fragment may hold without percent-encoding, apart from the apostrophe. */
export const fragmentAlphabet = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz-._~!$&()*+,;=:@";

/** The 43 characters of the alphanumeric mode of a QR code that a URL may hold. */
export const qrAlphabet = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ$*+-./:";

/** The letters and digits the two check characters of a fragment payload are written in. */
const fragmentCheckAlphabet = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/** The digits and uppercase letters the two check characters of a QR payload are written in. */
const qrCheckAlphabet = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

const limbRange = 2 ** 32;

/** Reads digits of an ASCII alphabet back into the bytes the encoder wrote. */
export function decodeDigits(digits: string, alphabet: string): Uint8Array {
  const values: number[] = [];
  for (const character of digits) {
    const value = alphabet.indexOf(character);
    if (value < 0) {
      throw malformed("a character is not in the alphabet");
    }
    values.push(value);
  }
  return decodeValues(values, alphabet.length);
}

/**
 * Reads digits in a radix, most significant first, back into the bytes the encoder wrote. The
 * encoder puts a one byte in front of the bytes so that leading zeros survive, and this removes
 * it again.
 */
export function decodeValues(values: readonly number[], radix: number): Uint8Array {
  // The number as 32-bit limbs, lowest first. Each digit adds log2(radix) bits at most.
  const limbs = new Uint32Array(Math.ceil((values.length * Math.log2(radix)) / 32) + 2);
  let used = 0;
  for (const value of values) {
    let carry = value;
    for (let index = 0; index < used; index += 1) {
      const product = (limbs[index] ?? 0) * radix + carry;
      limbs[index] = product % limbRange;
      carry = Math.floor(product / limbRange);
    }
    if (carry > 0) {
      limbs[used] = carry;
      used += 1;
    }
  }
  const bytes: number[] = [];
  for (let index = 0; index < used; index += 1) {
    const limb = limbs[index] ?? 0;
    bytes.push(limb & 0xff, (limb >>> 8) & 0xff, (limb >>> 16) & 0xff, (limb >>> 24) & 0xff);
  }
  while (bytes[bytes.length - 1] === 0) {
    bytes.pop();
  }
  if (bytes.pop() !== 1) {
    throw malformed("the number has no leading marker");
  }
  return Uint8Array.from(bytes.reverse());
}

/**
 * Returns the two check characters of a text: the FNV-1a hash of its bytes reduced to the square
 * of the size of the check alphabet, written as one character of the alphabet each.
 */
export function checkCharacters(text: string, alphabet: "fragment" | "qr" = "fragment"): string {
  const checkAlphabet = alphabet === "fragment" ? fragmentCheckAlphabet : qrCheckAlphabet;
  let hash = 0x811c9dc5;
  // The hash runs over the UTF-8 bytes, which are the characters themselves for ASCII.
  for (const byte of new TextEncoder().encode(text)) {
    hash ^= byte;
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  const size = checkAlphabet.length;
  const value = hash % (size * size);
  return (checkAlphabet[Math.floor(value / size)] ?? "0") + (checkAlphabet[value % size] ?? "0");
}
