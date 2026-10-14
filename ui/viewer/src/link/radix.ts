// Reads bytes written as one big number in an alphabet, and the check characters of a link.
//
// The Rust side is `crates/btt-model/src/link/radix.rs`.

import { malformed } from "./errors";

/** The 78 characters a URL fragment may hold without percent-encoding, apart from the apostrophe. */
export const fragmentAlphabet = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz-._~!$&()*+,;=:@";

const checkAlphabet = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

const limbRange = 2 ** 32;

/**
 * Reads digits of an alphabet back into the bytes the encoder wrote. The encoder puts a one
 * byte in front of the bytes so that leading zeros survive, and this removes it again.
 */
export function decodeDigits(digits: string, alphabet: string): Uint8Array {
  const radix = alphabet.length;
  // The number as 32-bit limbs, lowest first. Each digit adds log2(radix) bits at most.
  const limbs = new Uint32Array(Math.ceil((digits.length * Math.log2(radix)) / 32) + 2);
  let used = 0;
  for (const character of digits) {
    const value = alphabet.indexOf(character);
    if (value < 0) {
      throw malformed("a character is not in the alphabet");
    }
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
 * Returns the two check characters of a text: the FNV-1a hash of its bytes reduced to 3,844
 * values and written as a letter or digit each. The text has to be ASCII.
 */
export function checkCharacters(text: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < text.length; index += 1) {
    hash ^= text.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  const value = hash % (62 * 62);
  return (checkAlphabet[Math.floor(value / 62)] ?? "0") + (checkAlphabet[value % 62] ?? "0");
}
