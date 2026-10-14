// Read-only helpers over the itinerary types from the Rust model.
//
// Only types are imported from the bindings, so these helpers work in the web viewer too,
// which has no Tauri runtime.

import type {
  DailyTimeWindow,
  Itinerary,
  ItineraryRoute,
  Journey,
  JourneyChoice,
  LegPlan,
  LegTravelMode,
  PlaceStay,
  PlaceWindowOverride,
  StationWay,
  TrainRide,
  Trip,
  TripPlace,
  TripPlaceId,
} from "./bindings";
import { dateInZone, formatTimeInZone } from "./format";
import { haversineMeters } from "./geo";

/** Position of a place in the itinerary. Only via places have a stay. */
export type PlaceRole = "origin" | "via" | "destination";

/** The colour a leg takes on the itinerary spine, the map and its train chips. */
export interface LegLine {
  /** A CSS colour value, such as `var(--leg-3)`. */
  colour: string;
  /** Whether the leg is a night leg, which draws as a dashed line. */
  night: boolean;
}

/** Returns the colour of the leg at a one-based position, cycling the line palette. */
export function legLine(travelMode: LegTravelMode, position: number): LegLine {
  return legLineOf(travelMode !== "Daytime", position);
}

/** Returns the colour of a night leg, or of a day leg at a one-based position in the palette. */
export function legLineOf(night: boolean, position: number): LegLine {
  if (night) {
    return { colour: "var(--color-night)", night: true };
  }
  return { colour: `var(--leg-${((position - 1) % 4) + 1})`, night: false };
}

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

/** Names how long a place stays on its card: its role at the ends and its stay in between. */
export function stayLabel(role: PlaceRole, stay: PlaceStay | null): string {
  if (role !== "via" || stay === null) {
    return role;
  }
  const nights = stayNights(stay);
  if (stay.SameDay !== undefined) {
    return "same day";
  }
  return `${nights} ${nights === 1 ? "night" : "nights"}`;
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
 * arrive on their departure date until a plan gives the exact time. `plannedArrivalDates`
 * holds those dates by arrival place, and a place shows its planned date when it has one. The
 * departure dates keep to the calendar rules, because the planner plans each leg on them.
 */
export function itineraryRows(
  trip: Trip,
  plannedArrivalDates: ReadonlyMap<TripPlaceId, string> = new Map(),
): ItineraryRows {
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
      arrivalDate: plannedArrivalDates.get(arrival.place.id) ?? arrivalDate,
      departureDate: nextDepartureDate,
    });
    departurePlace = arrival.place;
    departureDate = nextDepartureDate ?? arrivalDate;
  }
  return { places, legs };
}

/** Returns the date on which the chosen journey of a leg plan reaches its last station. */
export function plannedArrivalDate(legPlan: LegPlan): string {
  const journey = legPlan.chosen_journey;
  const lastRide = journey.later_rides.at(-1)?.ride ?? journey.first_ride;
  const timeZone = legPlan.station_table[lastRide.alighting_station]?.time_zone ?? "UTC";
  return dateInZone(lastRide.arrival_time, timeZone);
}

/** Returns the arrival window a place overrides, or `null` when it uses the trip's default. */
export function overriddenArrivalWindow(windowOverride: PlaceWindowOverride | null): DailyTimeWindow | null {
  return windowOverride?.Arrival ?? windowOverride?.Both?.arrival ?? null;
}

/** Returns the departure window a place overrides, or `null` when it uses the trip's default. */
export function overriddenDepartureWindow(windowOverride: PlaceWindowOverride | null): DailyTimeWindow | null {
  return windowOverride?.Departure ?? windowOverride?.Both?.departure ?? null;
}

/** Combines the windows a place overrides into an override, or `null` when it overrides none. */
export function placeWindowOverride(
  arrival: DailyTimeWindow | null,
  departure: DailyTimeWindow | null,
): PlaceWindowOverride | null {
  if (arrival !== null && departure !== null) {
    return { Both: { arrival, departure } };
  }
  if (arrival !== null) {
    return { Arrival: arrival };
  }
  return departure === null ? null : { Departure: departure };
}

/** Returns the journey the traveller picked for a leg, or `null` when the planner picks. */
export function storedJourneyChoice(trip: Trip, leg: LegRow): JourneyChoice | null {
  const chosen = (trip.chosen_journeys ?? []).find(
    (candidate) =>
      candidate.departure_place_id === leg.departurePlace.id && candidate.arrival_place_id === leg.arrivalPlace.id,
  );
  return chosen?.choice ?? null;
}

/** Returns the rides of a journey in travel order. */
export function journeyRides(journey: Journey): TrainRide[] {
  return [journey.first_ride, ...journey.later_rides.map((later) => later.ride)];
}

/** Returns how a train is named, such as `ICE 125`. */
export function trainName(ride: TrainRide): string {
  return `${ride.train_label.category} ${ride.train_label.number}`;
}

/** Returns door-to-door minutes, including the ways to and from the stations. */
export function journeyMinutes(journey: Journey): number {
  const lastRide = journey.later_rides.at(-1)?.ride ?? journey.first_ride;
  return (
    journey.access.duration +
    (lastRide.arrival_time - journey.first_ride.departure_time) +
    (journey.arrival_footpath?.way.duration ?? 0) +
    journey.egress.duration
  );
}

/** Describes the number of transfers, such as "direct" or "2 transfers". */
export function transferSummary(journey: Journey): string {
  const count = journey.later_rides.length;
  if (count === 0) {
    return "direct";
  }
  return `${count} ${count === 1 ? "transfer" : "transfers"}`;
}

/** Returns the name of a station of a leg plan, or a placeholder for an index it lacks. */
export function stationName(legPlan: LegPlan, stationIndex: number): string {
  return legPlan.station_table[stationIndex]?.name ?? "Unknown station";
}

/** Returns a moment as the local time of a station of a leg plan. */
export function stationTime(legPlan: LegPlan, stationIndex: number, moment: number): string {
  const timeZone = legPlan.station_table[stationIndex]?.time_zone ?? "UTC";
  return formatTimeInZone(moment, timeZone);
}

/** A moment at a station, as the card of a journey shows it. */
export interface StationMoment {
  /** The local time at the station. */
  time: string;
  /** The name of the station. */
  station: string;
}

/** What the card of a journey shows of it at a glance. */
export interface JourneyOverview {
  /** The first boarding. */
  boarding: StationMoment;
  /** The last alighting. */
  alighting: StationMoment;
  /** Door-to-door minutes, with the ways to and from the stations. */
  minutes: number;
  /** The number of changes of train. */
  transfers: number;
  /** The names of the trains in travel order. */
  trains: string[];
}

/** Summarises a journey of a leg plan by its first boarding, its last alighting and its length. */
export function journeyOverview(legPlan: LegPlan, journey: Journey): JourneyOverview {
  const rides = journeyRides(journey);
  const lastRide = rides.at(-1) ?? journey.first_ride;
  return {
    boarding: {
      time: stationTime(legPlan, journey.first_ride.boarding_station, journey.first_ride.departure_time),
      station: stationName(legPlan, journey.first_ride.boarding_station),
    },
    alighting: {
      time: stationTime(legPlan, lastRide.alighting_station, lastRide.arrival_time),
      station: stationName(legPlan, lastRide.alighting_station),
    },
    minutes: journeyMinutes(journey),
    transfers: journey.later_rides.length,
    trains: rides.map(trainName),
  };
}

/** The farthest straight-line distance in meters that the timetable builder counts as a walk. */
const longestWalkMeters = 1000;

/**
 * Returns the way between the stations of two consecutive rides, or `null` when the second ride
 * boards at the station the first alights at. The plan stores only the time to change, so the
 * way is estimated from the straight-line distance with the rule of the timetable builder
 * (`04_footpaths.sql`): walking at 5 km/h along a path 30 percent longer below a kilometer, and
 * above it 15 minutes plus the distance at 15 km/h by local transport.
 */
export function transferWay(legPlan: LegPlan, previous: TrainRide, next: TrainRide): StationWay | null {
  if (previous.alighting_station === next.boarding_station) {
    return null;
  }
  const from = legPlan.station_table[previous.alighting_station];
  const to = legPlan.station_table[next.boarding_station];
  if (from === undefined || to === undefined) {
    return null;
  }
  const meters = haversineMeters(from.coordinates, to.coordinates);
  const walk = meters < longestWalkMeters;
  return {
    duration: Math.max(1, Math.ceil(walk ? (meters * 1.3) / 83.3 : 15 + meters / 250)),
    distance_meters: Math.round(meters),
    mode: walk ? "Walk" : "LocalTransport",
  };
}
