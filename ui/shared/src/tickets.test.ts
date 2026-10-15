import { describe, expect, it } from "vitest";

import type { TicketItem, TicketRide } from "./bindings";
import {
  adjustedTotal,
  effectivePrice,
  describeJourneyPrice,
  formatEstimate,
  formatEuro,
  formatJourneyPrice,
  formatEuroRange,
  formatPriceRange,
  isLivePending,
  formatPrice,
  formatTotal,
  isLiveFailed,
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

describe("prices in another currency", () => {
  it("show the original amount after the euro price", () => {
    const source = { name: "t", note: "", as_of: null, page: null };
    const francs = { low: 5370, high: 5370, kind: "Live" as const, source, original: { hundredths: 5000, currency: "CHF" } };
    expect(formatPrice(francs)).toBe("€53.70 (50 CHF)");
    const forints = { ...francs, original: { hundredths: 460050, currency: "HUF" } };
    expect(formatPrice(forints)).toBe("€53.70 (4600.50 HUF)");
    expect(formatPrice({ ...francs, original: null })).toBe("€53.70");
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
      ? { booking_link: "https://www.sbb.ch/en?x", first_departure_time: 1, ride_count: 1, travel_class: "Second" as const, site_search: null }
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

  it("marks a lookup that found nothing as failed, but not one that is still to come", () => {
    const ticket = item(true);
    const key = liveQuoteKey(ticket)!;
    expect(isLiveFailed(ticket, new Map([[key, null]]))).toBe(true);
    expect(isLiveFailed(ticket, new Map())).toBe(false);
    expect(isLiveFailed(ticket, new Map([[key, "Unavailable" as const]]))).toBe(false);
    expect(isLiveFailed(item(false), new Map())).toBe(false);
  });

  it("keeps the live price of an accommodation apart from the ticket of the same train", () => {
    const ticket = item(true);
    const request = ticket.live_quote_request!;
    const accommodation = (withPass: boolean): TicketItem => ({
      ...ticket,
      live_quote_request: { ...request, accommodation: { train: "NJ 466", with_pass: withPass } },
    });
    const keys = [ticket, accommodation(false), accommodation(true)].map((entry) => liveQuoteKey(entry));
    expect(new Set(keys).size).toBe(3);
  });
});

describe("formatEstimate", () => {
  it("shows a range as its two ends", () => {
    expect(formatEstimate({ low: 3900, median: 4800, high: 6100 })).toBe("€39 to €61");
  });

  it("marks a single estimated amount with a tilde and never shows it as exact", () => {
    expect(formatEstimate({ low: 4800, median: 4800, high: 4800 })).toBe("~€48");
  });
});

describe("formatEuroRange", () => {
  it("adds the median to the range and marks a single amount with a tilde", () => {
    expect(formatEuroRange({ low: 3900, median: 4800, high: 6100 })).toBe("about €48 (€39 to €61)");
    expect(formatEuroRange({ low: 4800, median: 4800, high: 4800 })).toBe("~€48");
  });
});

describe("formatJourneyPrice", () => {
  const range = { low: 3900, median: 4800, high: 6100 };

  it("shows a live price as one exact amount", () => {
    expect(formatJourneyPrice({ low: 4799, median: 4799, high: 4799 }, true)).toBe("€47.99");
  });

  it("keeps the range of a live price that is not one amount", () => {
    expect(formatJourneyPrice(range, true)).toBe("€39 to €61");
  });

  it("never shows an estimate as an exact amount", () => {
    expect(formatJourneyPrice(range, false)).toBe("€39 to €61");
    expect(formatJourneyPrice({ low: 4800, median: 4800, high: 4800 }, false)).toBe("~€48");
  });

  it("returns nothing for a journey with no price", () => {
    expect(formatJourneyPrice(null, false)).toBeNull();
    expect(formatJourneyPrice(undefined, true)).toBeNull();
  });

  it("names the source of the price for its tooltip", () => {
    expect(describeJourneyPrice({ low: 4800, median: 4800, high: 4800 }, true)).toBe(
      "A live price from the booking site.",
    );
    expect(describeJourneyPrice(range, true)).toBe(
      "Live prices from the booking sites where they exist, and estimates for the other tickets.",
    );
    expect(describeJourneyPrice(range, false)).toBe("An estimate from the price model, about €48.");
  });
});

describe("estimated prices of tickets", () => {
  const source = { name: "t", note: "", as_of: null, page: null };

  it("mark an estimate with one amount with a tilde", () => {
    expect(formatPrice({ low: 4800, high: 4800, kind: "Range", source, original: null })).toBe("~€48");
    expect(formatTotal({ low: 9000, high: 9000, kind: "Range" })).toBe("~€90");
  });

  it("keep a range, a live price and a published price as they are", () => {
    expect(formatPrice({ low: 3900, high: 6100, kind: "Range", source, original: null })).toBe("€39 to €61");
    expect(formatPrice({ low: 4800, high: 4800, kind: "Live", source, original: null })).toBe("€48");
    expect(formatPrice({ low: 4800, high: 4800, kind: "Formula", source, original: null })).toBe("€48");
    expect(formatTotal({ low: 9000, high: 12000, kind: "Range" })).toBe("€90 to €120");
  });
});
