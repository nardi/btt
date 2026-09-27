import { describe, expect, it } from "vitest";

import {
  dailyTimeWindow,
  formatDailyTimeWindow,
  formatDistance,
  formatDuration,
  formatTimeInZone,
  parseMinutesOfDay,
} from "./format";

describe("daily time windows", () => {
  it("crosses midnight when the end comes before the start", () => {
    expect(dailyTimeWindow(22 * 60, 60)).toEqual({ start: 1320, length: 180 });
    expect(formatDailyTimeWindow({ start: 1320, length: 180 })).toBe("22:00–01:00");
  });

  it("covers the whole day when start and end are equal", () => {
    expect(dailyTimeWindow(300, 300).length).toBe(1440);
    expect(formatDailyTimeWindow({ start: 300, length: 1440 })).toBe("any time");
  });
});

describe("parseMinutesOfDay", () => {
  it("accepts valid times and rejects others", () => {
    expect(parseMinutesOfDay("07:30")).toBe(450);
    expect(parseMinutesOfDay("24:00")).toBeNull();
    expect(parseMinutesOfDay("noon")).toBeNull();
  });
});

describe("formatDuration", () => {
  it("drops empty parts", () => {
    expect(formatDuration(45)).toBe("45 min");
    expect(formatDuration(120)).toBe("2 h");
    expect(formatDuration(448)).toBe("7 h 28 min");
  });
});

describe("formatTimeInZone", () => {
  it("shows the station's local clock", () => {
    // 2026-10-01 06:00 UTC is 08:00 in Amsterdam and 07:00 in London.
    const moment = Date.UTC(2026, 9, 1, 6, 0) / 60_000;
    expect(formatTimeInZone(moment, "Europe/Amsterdam")).toBe("08:00");
    expect(formatTimeInZone(moment, "Europe/London")).toBe("07:00");
  });
});

describe("formatDistance", () => {
  it("uses meters below a kilometer and drops needless decimals", () => {
    expect(formatDistance(800)).toBe("800 m");
    expect(formatDistance(2500)).toBe("2.5 km");
    expect(formatDistance(5000)).toBe("5 km");
    expect(formatDistance(12_400)).toBe("12 km");
  });
});
