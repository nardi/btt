import { describe, expect, it } from "vitest";
import demo from "../fixtures/demo.json";
import stress from "../fixtures/stress.json";
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

  it("accepts the hash sign in front of the payload", async () => {
    expect(await decodeSnapshotLink(`#${demo.payload}`)).toEqual(demo.document.snapshot);
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
