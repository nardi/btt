import { describe, expect, it } from "vitest";

import type { OperatorEndDate } from "./bindings";
import { lastDepartureDate, operatorGaps } from "./timetable-coverage";
import { amsterdamViennaRome } from "./test-fixtures";

function end(name: string, lastDepartureDate: string): OperatorEndDate {
  return { name, country: "XX", last_departure_date: lastDepartureDate };
}

describe("lastDepartureDate", () => {
  it("is the day the last leg leaves", () => {
    expect(lastDepartureDate(amsterdamViennaRome())).toBe("2026-10-03");
  });

  it("is the start date of a trip without legs", () => {
    const trip = { ...amsterdamViennaRome(), itinerary: "Empty" as const };
    expect(lastDepartureDate(trip)).toBe("2026-10-01");
  });
});

describe("operatorGaps", () => {
  it("lists operators that end before the last leg, ordered by name", () => {
    const gaps = operatorGaps(
      [end("SNCF", "2026-10-02"), end("DB", "2026-09-30"), end("ÖBB", "2026-10-03")],
      amsterdamViennaRome(),
    );
    expect(gaps.map((gap) => [gap.name, gap.runsAtStart])).toEqual([
      ["DB", false],
      ["SNCF", true],
    ]);
  });

  it("is empty when every operator runs through the trip", () => {
    expect(operatorGaps([end("DB", "2026-12-12")], amsterdamViennaRome())).toEqual([]);
  });
});
