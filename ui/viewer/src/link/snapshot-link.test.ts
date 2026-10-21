import { describe, expect, it } from "vitest";
import demo from "../fixtures/demo.json";
import demoV1 from "../fixtures/demo-v1.json";
import stress from "../fixtures/stress.json";
import stressV1 from "../fixtures/stress-v1.json";
import { SnapshotLinkError } from "./errors";
import { decodeSnapshotLink } from "./snapshot-link";

/** Returns the error a promise rejects with, or fails the test. */
async function rejection(promise: Promise<unknown>): Promise<SnapshotLinkError> {
  try {
    await promise;
  } catch (error) {
    expect(error).toBeInstanceOf(SnapshotLinkError);
    return error as SnapshotLinkError;
  }
  throw new Error("the promise did not reject");
}

describe("reading the links the encoder writes", () => {
  it("reads the demo trip the way the encoder meant it", async () => {
    expect(await decodeSnapshotLink(demo.payload)).toEqual(demo.document.snapshot);
  });

  it("reads the large trip the way the encoder meant it", async () => {
    expect(await decodeSnapshotLink(stress.payload)).toEqual(stress.document.snapshot);
  });

  it("reads the links written for a QR code", async () => {
    expect(await decodeSnapshotLink(demo.qr_payload)).toEqual(demo.document.snapshot);
    expect(await decodeSnapshotLink(stress.qr_payload)).toEqual(stress.document.snapshot);
  });

  it("reads the compact links, as typed and as a browser keeps them", async () => {
    expect(await decodeSnapshotLink(demo.compact_payload)).toEqual(demo.document.snapshot);
    expect(await decodeSnapshotLink(stress.compact_payload)).toEqual(stress.document.snapshot);
    // A browser keeps the letters percent-encoded.
    expect(await decodeSnapshotLink(encodeURIComponent(demo.compact_payload))).toEqual(
      demo.document.snapshot,
    );
  });

  it("recognises a compact link that was cut", async () => {
    const error = await rejection(decodeSnapshotLink(demo.compact_payload.slice(0, -1)));
    expect(error.kind).toBe("cut");
    await rejection(decodeSnapshotLink("c%E4%BA"));
    await rejection(decodeSnapshotLink("c%ZZZZ"));
    await rejection(decodeSnapshotLink(`c${"x".repeat(30)}ab`));
  });

  it("accepts the hash sign in front of the payload", async () => {
    expect(await decodeSnapshotLink(`#${demo.payload}`)).toEqual(demo.document.snapshot);
  });
});

describe("reading the links of schema version 1", () => {
  // The fixtures of version 1 never change, so the links that were shared before version 2 keep
  // opening.
  it("reads the demo trip with the settings the link kept", async () => {
    expect(await decodeSnapshotLink(demoV1.payload)).toEqual(demoV1.document.snapshot);
    expect(await decodeSnapshotLink(demoV1.qr_payload)).toEqual(demoV1.document.snapshot);
    expect(await decodeSnapshotLink(demoV1.compact_payload)).toEqual(demoV1.document.snapshot);
  });

  it("reads the large trip", async () => {
    expect(await decodeSnapshotLink(stressV1.payload)).toEqual(stressV1.document.snapshot);
  });
});

describe("reading links that are damaged", () => {
  it("recognises a link with cut characters", async () => {
    for (let cut = 1; cut <= 4; cut += 1) {
      const error = await rejection(decodeSnapshotLink(demo.payload.slice(0, -cut)));
      expect(error.kind).toBe("cut");
    }
  });

  it("recognises a link that a messenger ended with punctuation", async () => {
    for (const punctuation of [".", ",", ")", ";", ":", "!"]) {
      await rejection(decodeSnapshotLink(demo.payload + punctuation));
    }
  });

  it("recognises a changed character", async () => {
    const middle = Math.floor(demo.payload.length / 2);
    const changed = demo.payload.slice(0, middle) + (demo.payload[middle] === "x" ? "y" : "x") + demo.payload.slice(middle + 1);
    expect((await rejection(decodeSnapshotLink(changed))).kind).toBe("cut");
  });

  it("refuses text that is no payload", async () => {
    for (const text of ["", "a", "abc", "###", "ünïcode", "a~~~~~~~~~~~~", "a".repeat(40_000)]) {
      await rejection(decodeSnapshotLink(text));
    }
  });
});
