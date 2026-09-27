// Read-only helpers over the itinerary types from the Rust model.
//
// Only types are imported from the bindings, so these helpers work in the web viewer too,
// which has no Tauri runtime.

import type {
  Itinerary,
  ItineraryRoute,
  LegTravelMode,
  PlaceStay,
  Trip,
  TripPlace,
} from "./bindings";

/** Position of a place in the itinerary. Only via places have a stay. */
export type PlaceRole = "origin" | "via" | "destination";

/** One place of the itinerary, with the dates the itinerary implies. */
export interface PlaceRow {
  place: TripPlace;
  role: PlaceRole;
  /** Stay at a via place, `null` for the origin and the destination. */
  stay: PlaceStay | null;
  /** Date of arrival as `YYYY-MM-DD`, `null` for the origin. */
  arrivalDate: string | null;
  /** Date of departure as `YYYY-MM-DD`, `null` for the destination. */
  departureDate: string | null;
}

/** One leg between two adjacent places. The arrival place's ID identifies the leg. */
export interface LegRow {
  departurePlace: TripPlace;
  arrivalPlace: TripPlace;
  travelMode: LegTravelMode;
  /** Date of departure as `YYYY-MM-DD`. */
  departureDate: string;
}

/** The places and legs of an itinerary in travel order. */
export interface ItineraryRows {
  places: PlaceRow[];
  legs: LegRow[];
}

/** Returns the route of an itinerary that has at least an origin and a destination. */
export function itineraryRoute(itinerary: Itinerary): ItineraryRoute | null {
  if (typeof itinerary === "object" && itinerary.Route !== undefined) {
    return itinerary.Route;
  }
  return null;
}

/** Returns every place of an itinerary in travel order. */
export function tripPlaces(itinerary: Itinerary): TripPlace[] {
  if (itinerary === "Empty") {
    return [];
  }
  if (itinerary.OriginOnly !== undefined) {
    return [itinerary.OriginOnly];
  }
  const route = itinerary.Route;
  return [route.origin, ...route.via_places.map((viaPlace) => viaPlace.place), route.destination];
}

/** Returns whether a leg includes a night train. */
export function isOvernight(travelMode: LegTravelMode): boolean {
  return travelMode !== "Daytime";
}

/** Returns the nights of a stay, which is zero for a same-day stay. */
export function stayNights(stay: PlaceStay): number {
  return stay.Nights ?? 0;
}

/** Adds days to a `YYYY-MM-DD` date. Works in UTC, so daylight saving time cannot shift it. */
export function addDays(isoDate: string, days: number): string {
  const date = new Date(`${isoDate}T00:00:00Z`);
  date.setUTCDate(date.getUTCDate() + days);
  return date.toISOString().slice(0, 10);
}

/**
 * Lays out the places and legs of a trip with their dates.
 *
 * Follows the calendar rules of `Itinerary::leg_departure_dates` in Rust. Each via place adds its
 * nights, and an overnight leg arrives the day after it departs. Day trains are assumed to
 * arrive on their departure date until a plan gives the exact time.
 */
export function itineraryRows(trip: Trip): ItineraryRows {
  const route = itineraryRoute(trip.itinerary);
  if (route === null) {
    const places = tripPlaces(trip.itinerary).map(
      (place): PlaceRow => ({
        place,
        role: "origin",
        stay: null,
        arrivalDate: null,
        departureDate: trip.start_date,
      }),
    );
    return { places, legs: [] };
  }

  const places: PlaceRow[] = [
    {
      place: route.origin,
      role: "origin",
      stay: null,
      arrivalDate: null,
      departureDate: trip.start_date,
    },
  ];
  const legs: LegRow[] = [];
  let departureDate = trip.start_date;
  let departurePlace = route.origin;

  const arrivals = [
    ...route.via_places.map((viaPlace) => ({
      place: viaPlace.place,
      travelMode: viaPlace.arriving_leg_travel_mode,
      stay: viaPlace.stay as PlaceStay | null,
    })),
    { place: route.destination, travelMode: route.final_leg_travel_mode, stay: null },
  ];
  for (const arrival of arrivals) {
    legs.push({
      departurePlace,
      arrivalPlace: arrival.place,
      travelMode: arrival.travelMode,
      departureDate,
    });
    const arrivalDate = addDays(departureDate, isOvernight(arrival.travelMode) ? 1 : 0);
    const nextDepartureDate =
      arrival.stay === null ? null : addDays(arrivalDate, stayNights(arrival.stay));
    places.push({
      place: arrival.place,
      role: arrival.stay === null ? "destination" : "via",
      stay: arrival.stay,
      arrivalDate,
      departureDate: nextDepartureDate,
    });
    departurePlace = arrival.place;
    departureDate = nextDepartureDate ?? arrivalDate;
  }
  return { places, legs };
}
