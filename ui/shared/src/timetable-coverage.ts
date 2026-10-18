// Tells which operators have no trains on the days of a trip.

import type { OperatorEndDate, Trip, TripPlaceId } from "./bindings";
import { itineraryRows } from "./itinerary";

/** An operator whose trains end before the last day of a trip. */
export interface OperatorGap {
  name: string;
  /** The country of the operator's home network, as an ISO 3166 code. */
  country: string;
  /** The last day the operator has trains, as `YYYY-MM-DD`. */
  lastDepartureDate: string;
}

/**
 * Returns the day the trip arrives at its destination, or the start date when the trip has no
 * legs. A planned arrival date replaces the calendar's, because a journey can take more days
 * than the leg's travel mode assumes, and it can need trains on all of them.
 */
export function tripLastDate(
  trip: Trip,
  plannedArrivalDates: ReadonlyMap<TripPlaceId, string> = new Map(),
): string {
  const destination = itineraryRows(trip, plannedArrivalDates).places.at(-1);
  return destination?.arrivalDate ?? trip.start_date;
}

/**
 * Returns the operators with no trains on the last day of a trip, `tripEnd`, ordered by name. Dates are
 * `YYYY-MM-DD` strings, which compare in date order.
 */
export function operatorGaps(ends: readonly OperatorEndDate[], tripEnd: string): OperatorGap[] {
  return ends
    .filter((end) => end.last_departure_date < tripEnd)
    .map((end) => ({
      name: end.name,
      country: end.country,
      lastDepartureDate: end.last_departure_date,
    }))
    .sort((left, right) => left.name.localeCompare(right.name));
}
