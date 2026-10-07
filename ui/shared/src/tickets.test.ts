import { describe, expect, it } from "vitest";

import type { TicketRide } from "./bindings";
import {
  adjustedTotal,
  effectivePrice,
  formatEuro,
  formatPriceRange,
  isLivePending,
  isLiveUnavailable,
  liveQuoteKey,
  livePriceOf,
  ridesByDate,
  ticketColour,
} from "./tickets";

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

describe("live prices", () => {
  const price = (low: number, kind: "Formula" | "Live" | "Range") => ({
    low,
    high: low,
    kind,
    source: { name: "test", note: "", as_of: null, page: null },
  });
  const item = (id: number, estimate: number, withRequest: boolean) => ({
    id,
    product: "SingleTicket" as const,
    title: "t",
    detail: "d",
    ride_indices: [0],
    price: price(estimate, "Range"),
    booking_link: null,
    live_quote_request: withRequest
      ? {
          booking_link: "https://www.sbb.ch/en?x",
          first_departure_time: 100 + id,
          ride_count: 1,
          travel_class: "Second" as const,
        }
      : null,
  });

  it("replaces an estimate with the live price of the same ticket", () => {
    const strategy = {
      kind: "SingleTickets" as const,
      title: "s",
      items: [item(0, 5000, true), item(1, 3000, true), item(2, 1000, false)],
      total: { low: 9000, high: 9000, kind: "Range" as const },
      covers: [],
      notes: [],
    };
    const live = new Map([[liveQuoteKey(strategy.items[0]!)!, { Priced: price(4500, "Live") }]]);
    expect(adjustedTotal(strategy, live)).toEqual({ low: 8500, high: 8500, kind: "Range" });
    expect(effectivePrice(strategy.items[0]!, live).kind).toBe("Live");
    expect(effectivePrice(strategy.items[1]!, live).kind).toBe("Range");
    expect(liveQuoteKey(strategy.items[2]!)).toBeNull();
  });
});

describe("pending live prices", () => {
  const item = (withRequest: boolean) => ({
    id: 0,
    product: "SingleTicket" as const,
    title: "t",
    detail: "d",
    ride_indices: [0],
    price: { low: 1, high: 1, kind: "Range" as const, source: { name: "t", note: "", as_of: null, page: null } },
    booking_link: null,
    live_quote_request: withRequest
      ? { booking_link: "https://www.sbb.ch/en?x", first_departure_time: 1, ride_count: 1, travel_class: "Second" as const }
      : null,
  });

  it("is pending only while loading and without an answer", () => {
    const ticket = item(true);
    const key = liveQuoteKey(ticket)!;
    expect(isLivePending(ticket, new Map(), true)).toBe(true);
    expect(isLivePending(ticket, new Map(), false)).toBe(false);
    expect(isLivePending(ticket, new Map([[key, null]]), true)).toBe(false);
    expect(isLivePending(item(false), new Map(), true)).toBe(false);
  });

  it("marks a ticket the site does not sell, and keeps its estimate", () => {
    const ticket = item(true);
    const live = new Map([[liveQuoteKey(ticket)!, "Unavailable" as const]]);
    expect(isLiveUnavailable(ticket, live)).toBe(true);
    expect(livePriceOf(ticket, live)).toBeNull();
    expect(isLiveUnavailable(ticket, new Map())).toBe(false);
  });
});
