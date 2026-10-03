import { describe, expect, it } from "vitest";

import type { TicketRide } from "./bindings";
import { formatEuro, formatPriceRange, ridesByDate, ticketColour } from "./tickets";

function ride(departure: number, timeZone = "Europe/Vienna"): TicketRide {
  return {
    leg_arrival_place_id: 1,
    train_label: { category: "ICE", number: "1" },
    from_name: "A",
    to_name: "B",
    departure_time: departure,
    arrival_time: departure + 60,
    time_zone: timeZone,
    offers_berths: false,
  };
}

describe("formatEuro", () => {
  it("drops the cents of whole euros", () => {
    expect(formatEuro(1200)).toBe("€12");
    expect(formatEuro(1250)).toBe("€12.50");
  });
});

describe("formatPriceRange", () => {
  it("shows one amount for equal ends", () => {
    expect(formatPriceRange(500, 500)).toBe("€5");
    expect(formatPriceRange(500, 2000)).toBe("€5 to €20");
  });
});

describe("ridesByDate", () => {
  it("groups by the local date of the departure", () => {
    // 2026-07-01 22:30 UTC is already 2 July in Vienna, and 2026-07-02 08:00 UTC is the same day.
    const lateEvening = Date.UTC(2026, 6, 1, 22, 30) / 60_000;
    const morning = Date.UTC(2026, 6, 2, 8, 0) / 60_000;
    const nextMorning = Date.UTC(2026, 6, 3, 8, 0) / 60_000;
    const groups = ridesByDate([ride(lateEvening), ride(morning), ride(nextMorning)]);
    expect(groups).toEqual([
      { date: "2026-07-02", indices: [0, 1] },
      { date: "2026-07-03", indices: [2] },
    ]);
  });
});

describe("ticketColour", () => {
  it("repeats for the same id", () => {
    expect(ticketColour(2)).toBe(ticketColour(2));
    expect(ticketColour(0)).not.toBe(ticketColour(1));
  });
});
