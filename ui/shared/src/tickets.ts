// Helpers for showing ticket plans.

import type {
  EuroCents,
  LiveQuote,
  Price,
  PriceKind,
  PriceTotal,
  TicketItem,
  TicketRide,
  TicketStrategy,
} from "./bindings";

/** Formats an amount in euro cents, such as `€12.50`, and `€12` for whole euros. */
export function formatEuro(cents: EuroCents): string {
  const euro = cents / 100;
  return Number.isInteger(euro) ? `€${euro}` : `€${euro.toFixed(2)}`;
}

/** Formats a range, such as `€70 to €200`, or one amount when both ends are the same. */
export function formatPriceRange(low: EuroCents, high: EuroCents): string {
  return low === high ? formatEuro(low) : `${formatEuro(low)} to ${formatEuro(high)}`;
}

/** Formats the total of a strategy as a range. */
export function formatTotal(total: PriceTotal): string {
  return formatPriceRange(total.low, total.high);
}

/** Names a kind of price for its badge. */
export function priceKindLabel(kind: PriceKind): string {
  switch (kind) {
    case "Formula":
      return "Published price";
    case "Live":
      return "Live quote";
    case "Range":
      return "Estimate";
  }
}

/** Says what a kind of price means, for its tooltip. */
export function priceKindDescription(kind: PriceKind): string {
  switch (kind) {
    case "Formula":
      return "A published price list or tariff. A cheaper fare may exist.";
    case "Live":
      return "A quote from the operator for this train and date.";
    case "Range":
      return "A rough estimate with a lowest and a highest price.";
  }
}

/** The colours that tickets of a strategy take in turn, as CSS colour values. */
const ticketColours = ["#1f6fb2", "#b25e1f", "#2f8f5b", "#8a3fa8", "#b2331f", "#6b7280"];

/** Returns the colour of the ticket with an id, which stays the same for the same id. */
export function ticketColour(ticketId: number): string {
  return ticketColours[ticketId % ticketColours.length]!;
}

/** Returns the local date of a ride's departure as `YYYY-MM-DD`. */
export function rideDate(ride: TicketRide): string {
  const parts = new Intl.DateTimeFormat("en-CA", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    timeZone: ride.time_zone,
  }).formatToParts(new Date(ride.departure_time * 60_000));
  const part = (type: string) => parts.find((entry) => entry.type === type)?.value ?? "";
  return `${part("year")}-${part("month")}-${part("day")}`;
}

/** Groups the indices of rides by the local date of their departure, in travel order. */
export function ridesByDate(rides: readonly TicketRide[]): { date: string; indices: number[] }[] {
  const groups: { date: string; indices: number[] }[] = [];
  rides.forEach((ride, index) => {
    const date = rideDate(ride);
    const last = groups[groups.length - 1];
    if (last !== undefined && last.date === date) {
      last.indices.push(index);
    } else {
      groups.push({ date, indices: [index] });
    }
  });
  return groups;
}

/** The answers of booking sites, by the key of the ticket they are for. `null` means none. */
export type LivePrices = ReadonlyMap<string, LiveQuote | null>;

/** Returns the key that a ticket's live price is kept under, or `null` for a ticket with none. */
export function liveQuoteKey(item: TicketItem): string | null {
  const request = item.live_quote_request;
  if (request === null) {
    return null;
  }
  const search = request.site_search ?? null;
  const searchKey = search === null ? "" : `${search.origin_code}~${search.destination_code}~${search.local_departure}`;
  return `${request.booking_link}|${request.first_departure_time}|${request.ride_count}|${request.travel_class}|${searchKey}`;
}

/** Returns the live price of a ticket, or `null` when it has none yet. */
export function livePriceOf(item: TicketItem, live: LivePrices): Price | null {
  const key = liveQuoteKey(item);
  const quote = key === null ? null : (live.get(key) ?? null);
  return quote === null || quote === "Unavailable" ? null : quote.Priced;
}

/** Returns whether the booking site shows the trip of a ticket and sells no ticket for it. */
export function isLiveUnavailable(item: TicketItem, live: LivePrices): boolean {
  const key = liveQuoteKey(item);
  return key !== null && live.get(key) === "Unavailable";
}

/** Returns the price to show for a ticket: its live price if it has one, else its estimate. */
export function effectivePrice(item: TicketItem, live: LivePrices): Price {
  return livePriceOf(item, live) ?? item.price;
}

/** Adds up the prices of a strategy, with a live price in place of the estimate where there is one. */
export function adjustedTotal(strategy: TicketStrategy, live: LivePrices): PriceTotal {
  const order: PriceKind[] = ["Formula", "Live", "Range"];
  let low = 0;
  let high = 0;
  let kind: PriceKind = "Formula";
  for (const item of strategy.items) {
    const price = effectivePrice(item, live);
    low += price.low;
    high += price.high;
    if (order.indexOf(price.kind) > order.indexOf(kind)) {
      kind = price.kind;
    }
  }
  return { low, high, kind };
}

/** Returns whether the live price of a ticket is still being looked up. */
export function isLivePending(item: TicketItem, live: LivePrices, loading: boolean): boolean {
  const key = liveQuoteKey(item);
  return loading && key !== null && !live.has(key);
}

/** Returns whether any ticket of a strategy has a live price still being looked up. */
export function hasPendingLivePrice(
  strategy: TicketStrategy,
  live: LivePrices,
  loading: boolean,
): boolean {
  return strategy.items.some((item) => isLivePending(item, live, loading));
}
