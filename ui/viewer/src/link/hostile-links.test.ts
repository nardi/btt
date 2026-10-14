// Links that are made to hurt: a link that inflates to a huge size, counts the data cannot hold
// and random bytes. Each has to end in an error and never freeze or crash the tab.

import { describe, expect, it } from "vitest";
import { SnapshotLinkError } from "./errors";
import { checkCharacters, fragmentAlphabet } from "./radix";
import { decodeSnapshotLink, maximumInflatedBytes } from "./snapshot-link";

/** Compresses bytes as raw deflate, which the encoder does with a library on the Rust side. */
async function deflateRaw(bytes: Uint8Array): Promise<Uint8Array> {
  const stream = new Blob([new Uint8Array(bytes)]).stream().pipeThrough(new CompressionStream("deflate-raw"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

/** Writes bytes as the payload of a link, the way the encoder does, with a valid checksum. */
async function payloadOfBytes(bytes: Uint8Array): Promise<string> {
  const compressed = await deflateRaw(bytes);
  // The sentinel byte goes in front, and the whole number is written in the alphabet.
  let value = 1n;
  for (const byte of compressed) {
    value = (value << 8n) | BigInt(byte);
  }
  let digits = "";
  while (value > 0n) {
    digits = fragmentAlphabet[Number(value % 78n)] + digits;
    value /= 78n;
  }
  const body = `a${digits}`;
  return body + checkCharacters(body);
}

/** Writes numbers the way the bit layout does, as groups of seven bits. */
class TestBits {
  private bits: number[] = [];

  bit(value: boolean): void {
    this.bits.push(value ? 1 : 0);
  }

  number(value: number): void {
    let rest = value;
    for (;;) {
      const group = rest % 128;
      rest = Math.floor(rest / 128);
      this.bit(rest > 0);
      for (let position = 0; position < 7; position += 1) {
        this.bit(((group >> position) & 1) === 1);
      }
      if (rest === 0) {
        return;
      }
    }
  }

  bytes(): Uint8Array {
    const bytes = new Uint8Array(Math.ceil(this.bits.length / 8));
    this.bits.forEach((bit, index) => {
      bytes[index >> 3]! |= bit << (index & 7);
    });
    return bytes;
  }
}

async function kindOf(payload: string): Promise<string> {
  try {
    await decodeSnapshotLink(payload);
  } catch (error) {
    expect(error).toBeInstanceOf(SnapshotLinkError);
    return (error as SnapshotLinkError).kind;
  }
  return "accepted";
}

describe("hostile links", () => {
  it("names a snapshot with a newer schema version", async () => {
    const bits = new TestBits();
    bits.number(2);
    bits.number(99);
    expect(await kindOf(await payloadOfBytes(bits.bytes()))).toBe("newer-schema");
  });

  it("stops inflating a link that grows past the cap", async () => {
    const bomb = new Uint8Array(maximumInflatedBytes + 1000);
    expect(await kindOf(await payloadOfBytes(bomb))).toBe("too-large");
  });

  it("refuses a count that the data cannot hold", async () => {
    // Version 1, a date, a bundle version, then a trip name of a million bytes.
    const bits = new TestBits();
    bits.number(1);
    bits.number(20_000);
    bits.number(1);
    bits.number(1_000_000);
    expect(await kindOf(await payloadOfBytes(bits.bytes()))).toBe("malformed");
  });

  it("refuses data that is not compressed", async () => {
    const body = "aZZZZZZZZZZZZZZZZZZZZ";
    expect(await kindOf(body + checkCharacters(body))).toBe("malformed");
  });

  it("ends every random payload in an error or a trip, within five seconds", async () => {
    // A fixed seed keeps the test the same on every run.
    let seed = 12345;
    const random = () => {
      seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
      return seed / 2 ** 32;
    };
    const started = Date.now();
    for (let round = 0; round < 300; round += 1) {
      const bytes = new Uint8Array(Math.floor(random() * 200));
      bytes.forEach((_, index) => (bytes[index] = Math.floor(random() * 256)));
      expect(["accepted", "malformed", "newer-schema", "too-large"]).toContain(
        await kindOf(await payloadOfBytes(bytes)),
      );
    }
    expect(Date.now() - started).toBeLessThan(5000);
  });
});
