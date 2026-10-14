// Reads the payload of a link, which is the text after the `#` of the viewer's address.
//
// The Rust side is `crates/btt-model/src/link/mod.rs`. A payload is a marker for its alphabet,
// the compressed snapshot written as one big number, and two check characters.

import type { TripSnapshot } from "@btt/shared/bindings";
import { SnapshotLinkError, malformed } from "./errors";
import { inflateRaw } from "./inflate";
import { checkCharacters, decodeDigits, fragmentAlphabet } from "./radix";
import { readSnapshot } from "./snapshot-reader";

/** The character that starts a payload written in the 78-character fragment alphabet. */
const fragmentAlphabetMarker = "a";

/** Most characters a payload may have. A longer text is not a link the app wrote. */
export const maximumPayloadCharacters = 32_768;

/** Most bytes a payload may inflate to, which stops a small link from filling the memory. */
export const maximumInflatedBytes = 1 << 20;

/**
 * Reads the payload of a link, with or without the `#` in front of it. Throws a
 * `SnapshotLinkError` when the link was cut, comes from a newer version or is not a trip.
 */
export async function decodeSnapshotLink(fragment: string): Promise<TripSnapshot> {
  const payload = fragment.startsWith("#") ? fragment.slice(1) : fragment;
  if (payload.length > maximumPayloadCharacters) {
    throw new SnapshotLinkError("too-large", "the link is longer than the viewer reads");
  }
  // The alphabet is ASCII, so any other character means the text is no payload.
  if (!/^[\x21-\x7e]{4,}$/.test(payload)) {
    throw malformed("the link is not a payload");
  }
  const body = payload.slice(0, -2);
  if (checkCharacters(body) !== payload.slice(-2)) {
    throw new SnapshotLinkError("cut", "the link was cut or changed");
  }
  if (!body.startsWith(fragmentAlphabetMarker)) {
    throw malformed("the alphabet is unknown");
  }
  const compressed = decodeDigits(body.slice(1), fragmentAlphabet);
  const bits = await inflateRaw(compressed, maximumInflatedBytes);
  return readSnapshot(bits);
}
