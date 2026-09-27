import { describe, expect, it } from "vitest";

import { addDays, itineraryRows, tripPlaces } from "./itinerary";
import { amsterdamViennaRome, place } from "./test-fixtures";

describe("addDays", () => {
  it("crosses month and year ends", () => {
    expect(addDays("2026-10-31", 1)).toBe("2026-11-01");
    expect(addDays("2026-12-31", 2)).toBe("2027-01-02");
  });

  it("ignores daylight saving changes", () => {
    expect(addDays("2026-10-24", 1)).toBe("2026-10-25");
    expect(addDays("2026-10-25", 1)).toBe("2026-10-26");
  });
});

describe("itineraryRows", () => {
  it("reaches Rome on the third morning after Vienna", () => {
    const rows = itineraryRows(amsterdamViennaRome());
    expect(rows.legs.map((leg) => leg.departureDate)).toEqual(["2026-10-01", "2026-10-03"]);
    expect(rows.places.map((row) => [row.role, row.arrivalDate, row.departureDate])).toEqual([
      ["origin", null, "2026-10-01"],
      ["via", "2026-10-01", "2026-10-03"],
      ["destination", "2026-10-04", null],
    ]);
  });

  it("lists places of every itinerary variant", () => {
    const trip = amsterdamViennaRome();
    expect(tripPlaces(trip.itinerary).map((row) => row.name)).toEqual([
      "Amsterdam",
      "Vienna",
      "Rome",
    ]);
    expect(tripPlaces("Empty")).toEqual([]);
    const originOnly = { ...trip, itinerary: { OriginOnly: place(1, "Amsterdam") } };
    expect(itineraryRows(originOnly).legs).toEqual([]);
  });
});
