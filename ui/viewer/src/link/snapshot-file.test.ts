import { describe, expect, it } from "vitest";
import demo from "../fixtures/demo.json";
import { SnapshotLinkError } from "./errors";
import { parseSnapshotFile } from "./snapshot-file";

/** Returns a deep copy of the demo document that a test may damage. */
function demoDocument(): any {
  return JSON.parse(JSON.stringify(demo.document));
}

/** Returns the error that reading a document throws, or fails the test. */
function failure(document: unknown): SnapshotLinkError {
  try {
    parseSnapshotFile(JSON.stringify(document));
  } catch (error) {
    expect(error).toBeInstanceOf(SnapshotLinkError);
    return error as SnapshotLinkError;
  }
  throw new Error("the file was accepted");
}

describe("reading a trip file", () => {
  it("reads the file the desktop app writes the way the link reads", () => {
    expect(parseSnapshotFile(JSON.stringify(demo.document))).toEqual(demo.document.snapshot);
  });

  it("refuses text that is not JSON", () => {
    expect(() => parseSnapshotFile("not json")).toThrow(SnapshotLinkError);
    expect(() => parseSnapshotFile("")).toThrow(SnapshotLinkError);
  });

  it("names a file from a newer version", () => {
    const document = demoDocument();
    document.schema_version = 3;
    const error = failure(document);
    expect(error.kind).toBe("newer-schema");
    expect(error.schemaVersion).toBe(3);
  });

  it("drops fields it does not know", () => {
    const document = demoDocument();
    document.snapshot.trip.evil = "x";
    document.snapshot.legs[0].result.Found.chosen_journey.first_ride.path_coordinates = [
      { latitude: 1, longitude: 1 },
    ];
    const parsed = parseSnapshotFile(JSON.stringify(document));
    expect(parsed).toEqual(demo.document.snapshot);
  });

  it("refuses a journey that points at a station outside its table", () => {
    const document = demoDocument();
    document.snapshot.legs[0].result.Found.chosen_journey.first_ride.boarding_station = 40;
    expect(failure(document).kind).toBe("malformed");
  });

  it("refuses values out of range and values of the wrong type", () => {
    for (const damage of [
      (document: any) => (document.snapshot.trip.itinerary.Route.origin.coordinates.latitude = 95),
      (document: any) => (document.snapshot.trip.itinerary.Route.origin.name = 5),
      (document: any) => (document.snapshot.trip.start_date = "yesterday"),
      (document: any) => (document.snapshot.trip.itinerary.Route.via_places = "none"),
      (document: any) => (document.snapshot.legs[0].result = { Found: null }),
      (document: any) => (document.snapshot.legs[0].result = { Unknown: 1 }),
      (document: any) => (document.snapshot.bundle_version = -1),
      (document: any) => delete document.snapshot,
    ]) {
      const document = demoDocument();
      damage(document);
      expect(failure(document).kind).toBe("malformed");
    }
  });

  it("keeps markup in a station name as plain text", () => {
    const document = demoDocument();
    document.snapshot.legs[0].result.Found.station_table[0].name = "<img src=x onerror=alert(1)>";
    const parsed = parseSnapshotFile(JSON.stringify(document));
    const result = parsed.legs[0]!.result;
    expect(result.Found?.station_table[0]?.name).toBe("<img src=x onerror=alert(1)>");
  });
});
