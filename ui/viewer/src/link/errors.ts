// The ways a link or a trip file can fail to open.

/** What went wrong with a link, which the viewer turns into a message for the traveller. */
export type SnapshotLinkErrorKind =
  /** The check characters differ, so the link was cut or changed on the way. */
  | "cut"
  /** A newer app wrote the trip, so the viewer has to be reloaded to get the newer version. */
  | "newer-schema"
  /** The link is longer than the viewer reads, or inflates past the cap. */
  | "too-large"
  /** The link holds something that no trip holds. */
  | "malformed";

/** Thrown by every step of reading a link or a trip file. */
export class SnapshotLinkError extends Error {
  readonly kind: SnapshotLinkErrorKind;
  /** The schema version of a trip from a newer app, for `newer-schema`. */
  readonly schemaVersion: number | null;

  constructor(kind: SnapshotLinkErrorKind, message: string, schemaVersion: number | null = null) {
    super(message);
    this.name = "SnapshotLinkError";
    this.kind = kind;
    this.schemaVersion = schemaVersion;
  }
}

/** Returns the error for data that does not hold a trip. */
export function malformed(message: string): SnapshotLinkError {
  return new SnapshotLinkError("malformed", message);
}
