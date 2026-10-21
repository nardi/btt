// Finding the journey of a leg that a booking site sells as a similar trip.

import type { Journey, LegPlan, SimilarTrip, TicketItem, TicketPlan, TripPlaceId } from "./bindings";
import { journeyRides } from "./itinerary";

/** The furthest, in minutes, that a journey can leave from the similar trip and still be it. */
const SAME_DEPARTURE_MINUTES = 30;

/** Long names of categories that the booking sites write short. */
const SHORT_CATEGORIES: ReadonlyMap<string, string> = new Map([
  ["INTERCITY", "IC"],
  ["EUROCITY", "EC"],
]);

/** Returns a key that names a train by its category and number, such as `ICE848`. */
function trainKey(category: string, number: string): string {
  const upper = category.trim().toUpperCase();
  return `${SHORT_CATEGORIES.get(upper) ?? upper}${number.trim()}`;
}

/** Reads a train name of a booking site, such as `ICE 848`, into its key. */
function siteTrainKey(name: string): string {
  const text = name.trim();
  const split = text.search(/\d/);
  return split < 0 ? trainKey(text, "") : trainKey(text.slice(0, split), text.slice(split));
}

/** Returns the keys of the trains of a journey in travel order. */
function journeyTrainKeys(journey: Journey): string[] {
  return journeyRides(journey).map((ride) => trainKey(ride.train_label.category, ride.train_label.number));
}

/**
 * Returns whether a journey is the trip of a booking site, ignoring the first and the last
 * train. The timetable and a booking site can disagree on a city train at either end. With no
 * train between them, one shared train is all there is to go on.
 */
function isSimilarTrip(journey: Journey, similar: SimilarTrip): boolean {
  const minutesApart = Math.abs(journey.first_ride.departure_time - similar.first_departure_time);
  if (minutesApart > SAME_DEPARTURE_MINUTES) {
    return false;
  }
  const own = journeyTrainKeys(journey);
  const theirs = similar.trains.map(siteTrainKey);
  if (own.length < 3 || theirs.length < 3) {
    return own.some((key) => theirs.includes(key));
  }
  const middle = (keys: string[]) => keys.slice(1, -1).join("|");
  return middle(own) === middle(theirs);
}

/** Returns whether two journeys leave at the same time with the same trains. */
function isSameJourney(first: Journey, second: Journey): boolean {
  return (
    first.first_ride.departure_time === second.first_ride.departure_time &&
    journeyTrainKeys(first).join("|") === journeyTrainKeys(second).join("|")
  );
}

/**
 * Returns the alternative of a leg that is the similar trip of a booking site, or `null` when
 * the planner has none or the chosen journey already is it. Among several, the one that leaves
 * closest to the similar trip wins.
 */
export function journeyOfSimilarTrip(legPlan: LegPlan, similar: SimilarTrip): Journey | null {
  if (isSimilarTrip(legPlan.chosen_journey, similar)) {
    return null;
  }
  const candidates = legPlan.alternative_journeys.filter(
    (journey) => isSimilarTrip(journey, similar) && !isSameJourney(journey, legPlan.chosen_journey),
  );
  const apart = (journey: Journey) => Math.abs(journey.first_ride.departure_time - similar.first_departure_time);
  return candidates.reduce<Journey | null>(
    (best, journey) => (best === null || apart(journey) < apart(best) ? journey : best),
    null,
  );
}

/**
 * Returns the place that the leg of a ticket arrives at, or `null` when the ticket covers rides
 * of several legs, which one pick of a journey cannot change.
 */
export function arrivalPlaceOfTicket(plan: TicketPlan, item: TicketItem): TripPlaceId | null {
  const places = new Set(item.ride_indices.map((index) => plan.rides[index]?.leg_arrival_place_id));
  const [only] = places;
  return places.size === 1 && only !== undefined ? only : null;
}
