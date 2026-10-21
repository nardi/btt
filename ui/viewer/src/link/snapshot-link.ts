// Reads the payload of a link, which is the text after the `#` of the viewer's address.
//
// The Rust side is `crates/btt-model/src/link/mod.rs`. A payload is a marker for its alphabet,
// the compressed snapshot written as one big number, and two check characters.

import type { TripSnapshot } from "@btt/shared/bindings";
import { SnapshotLinkError, malformed } from "./errors";
import { inflateRaw } from "./inflate";
import { loadCompactAlphabet } from "./compact-alphabet";
import { checkCharacters, decodeDigits, decodeValues, fragmentAlphabet, qrAlphabet } from "./radix";
import { readSchemaVersion, readSnapshot, supportedSchemaVersion } from "./snapshot-reader";
import { readSnapshotV1 } from "./snapshot-reader-v1";
import { loadCityDictionary } from "./cities";
import { loadStationDictionary } from "./stations";
import { loadTrainDictionary } from "./trains";

/**
 * The first character of a payload names its alphabet and says whether the bytes are
 * compressed. A link for a chat uses the 78 characters of a URL fragment or, when it has to be
 * short, the 44,190 letters of the compact alphabet. A link for a QR code uses the 43 characters
 * of its alphanumeric mode.
 */
const markers = {
  a: { alphabet: fragmentAlphabet, checks: "fragment", compressed: true },
  b: { alphabet: fragmentAlphabet, checks: "fragment", compressed: false },
  c: { alphabet: "compact", checks: "fragment", compressed: true },
  d: { alphabet: "compact", checks: "fragment", compressed: false },
  Q: { alphabet: qrAlphabet, checks: "qr", compressed: true },
  R: { alphabet: qrAlphabet, checks: "qr", compressed: false },
} as const;

/** Most characters a payload may have. A longer text is not a link the app wrote. */
export const maximumPayloadCharacters = 32_768;

/** Most bytes a payload may inflate to, which stops a small link from filling the memory. */
export const maximumInflatedBytes = 1 << 20;

/**
 * Reads the payload of a link, with or without the `#` in front of it. Throws a
 * `SnapshotLinkError` when the link was cut, comes from a newer version or is not a trip.
 */
export async function decodeSnapshotLink(fragment: string): Promise<TripSnapshot> {
  let payload = fragment.startsWith("#") ? fragment.slice(1) : fragment;
  // A letter takes at most 9 characters percent-encoded, which is how a browser keeps a link.
  if (payload.length > 9 * maximumPayloadCharacters) {
    throw new SnapshotLinkError("too-large", "the link is longer than the viewer reads");
  }
  if (payload.includes("%")) {
    try {
      payload = decodeURIComponent(payload);
    } catch {
      throw malformed("the link has a damaged percent sign");
    }
  }
  if (payload.length > maximumPayloadCharacters) {
    throw new SnapshotLinkError("too-large", "the link is longer than the viewer reads");
  }
  if (payload.length < 4) {
    throw malformed("the link is not a payload");
  }
  const marker = markers[payload[0] as keyof typeof markers];
  if (marker === undefined) {
    throw malformed("the alphabet is unknown");
  }
  // The alphabets other than the compact one are ASCII, so any other character means the text is
  // no payload.
  if (marker.alphabet !== "compact" && !/^[\x21-\x7e]+$/.test(payload)) {
    throw malformed("the link is not a payload");
  }
  const body = payload.slice(0, -2);
  if (checkCharacters(body, marker.checks) !== payload.slice(-2)) {
    throw new SnapshotLinkError("cut", "the link was cut or changed");
  }
  let bytes: Uint8Array;
  if (marker.alphabet === "compact") {
    const alphabet = await loadCompactAlphabet();
    const values: number[] = [];
    for (const letter of body.slice(1)) {
      const value = alphabet.indexOf(letter);
      if (value === undefined) {
        throw malformed("a character is not in the alphabet");
      }
      values.push(value);
    }
    bytes = decodeValues(values, alphabet.size);
  } else {
    bytes = decodeDigits(body.slice(1), marker.alphabet);
  }
  let bits = bytes;
  if (marker.compressed) {
    bits = await inflateRaw(bytes, maximumInflatedBytes);
  } else if (bytes.length > maximumInflatedBytes) {
    throw new SnapshotLinkError("too-large", "the link is larger than the viewer reads");
  }
  // The first number of the layout is its schema version, which picks the reader.
  const version = readSchemaVersion(bits);
  if (version > supportedSchemaVersion) {
    throw new SnapshotLinkError("newer-schema", `the trip was written with schema version ${version}`, version);
  }
  const [stations, cities, trains] = await Promise.all([
    loadStationDictionary(),
    loadCityDictionary(),
    loadTrainDictionary(),
  ]);
  return version === 1 ? readSnapshotV1(bits, stations, cities) : readSnapshot(bits, stations, cities, trains);
}
