// Tells which operators have no trains on the days of a trip.

import type { OperatorEndDate, Trip } from "./bindings";
import { itineraryRows } from "./itinerary";

/** An operator whose trains end before the last day of a trip. */
export interface OperatorGap {
  name: string;
  /** The country of the operator's home network, as an ISO 3166 code. */
  country: string;
  /** The last day the operator has trains, as `YYYY-MM-DD`. */
  lastDepartureDate: string;
  /** Whether the operator has trains on the first day of the trip, so only later legs lack them. */
  runsAtStart: boolean;
}

/**
 * Returns the day the last leg of a trip departs, or the start date when the trip has no legs.
 * The timetable lists trains by their departure day, so this is the last day that matters.
 */
export function lastDepartureDate(trip: Trip): string {
  return itineraryRows(trip).legs.at(-1)?.departureDate ?? trip.start_date;
}

/**
 * Returns the operators with no trains on the last day of a trip, ordered by name. Dates are
 * `YYYY-MM-DD` strings, which compare in date order.
 */
export function operatorGaps(ends: readonly OperatorEndDate[], trip: Trip): OperatorGap[] {
  const tripEnd = lastDepartureDate(trip);
  return ends
    .filter((end) => end.last_departure_date < tripEnd)
    .map((end) => ({
      name: end.name,
      country: end.country,
      lastDepartureDate: end.last_departure_date,
      runsAtStart: end.last_departure_date >= trip.start_date,
    }))
    .sort((left, right) => left.name.localeCompare(right.name));
}
