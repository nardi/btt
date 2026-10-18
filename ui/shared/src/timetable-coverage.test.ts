import { describe, expect, it } from "vitest";

import type { OperatorEndDate } from "./bindings";
import { operatorGaps, tripLastDate } from "./timetable-coverage";
import { amsterdamViennaRome } from "./test-fixtures";
import { tripPlaces } from "./itinerary";

function end(name: string, lastDepartureDate: string): OperatorEndDate {
  return { name, country: "XX", last_departure_date: lastDepartureDate };
}

describe("tripLastDate", () => {
  it("is the day the last leg arrives by the calendar", () => {
    expect(tripLastDate(amsterdamViennaRome())).toBe("2026-10-04");
  });

  it("is the planned arrival day when the last leg takes longer", () => {
    const trip = amsterdamViennaRome();
    const rome = tripPlaces(trip.itinerary)[2]!.id;
    expect(tripLastDate(trip, new Map([[rome, "2026-10-06"]]))).toBe("2026-10-06");
  });

  it("is the start date of a trip without legs", () => {
    const trip = { ...amsterdamViennaRome(), itinerary: "Empty" as const };
    expect(tripLastDate(trip)).toBe("2026-10-01");
  });
});

describe("operatorGaps", () => {
  it("lists operators that end before the last leg, ordered by name", () => {
    const gaps = operatorGaps(
      [end("SNCF", "2026-10-02"), end("DB", "2026-09-30"), end("ÖBB", "2026-10-04")],
      "2026-10-04",
    );
    expect(gaps.map((gap) => [gap.name, gap.lastDepartureDate])).toEqual([
      ["DB", "2026-09-30"],
      ["SNCF", "2026-10-02"],
    ]);
  });

  it("is empty when every operator runs through the trip", () => {
    expect(operatorGaps([end("DB", "2026-12-12")], "2026-10-04")).toEqual([]);
  });
});
