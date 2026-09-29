// GeoJSON for the map layers, derived from a trip and its leg plans.

import type { FeatureCollection, LineString, Point, Polygon } from "geojson";

import type { GeoCoordinates, LegPlan, TrainRide, Trip, TripPlaceId } from "./bindings";
import { formatDuration, formatTimeInZone } from "./format";
import { circleRing } from "./geo";
import { itineraryRows, tripPlaces } from "./itinerary";
import { curvedRidePath } from "./ride-path";
import { legViewState, type TripViewState } from "./trip-source";

/** Properties of a station search radius circle. */
export interface RadiusCircleProperties {
  placeId: TripPlaceId;
  selected: boolean;
}

/** Properties of a ride line. */
export interface RideLineProperties {
  arrivalPlaceId: TripPlaceId;
  /** Whether the train has couchettes or sleepers, which the map shows in purple. */
  offersBerths: boolean;
  /** Lines of text for the hover tooltip, separated by line breaks. */
  tooltip: string;
}

/** Whether a point marks a change between trains or the start or end of a journey. */
export type TransferPointKind = "transfer" | "journey-end";

/** Properties of a transfer point. */
export interface TransferPointProperties {
  kind: TransferPointKind;
  /** Lines of text for the hover tooltip, separated by line breaks. */
  tooltip: string;
}

/** A place whose position or radius is being dragged and not yet saved. */
export interface PlaceDragPreview {
  placeId: TripPlaceId;
  coordinates: GeoCoordinates;
  radiusMeters: number;
}

/** Returns one circle per place, showing where the planner may look for stations. */
export function radiusCircles(
  trip: Trip,
  selectedPlaceId: TripPlaceId | null,
  dragPreview: PlaceDragPreview | null,
): FeatureCollection<Polygon, RadiusCircleProperties> {
  return {
    type: "FeatureCollection",
    features: tripPlaces(trip.itinerary).map((place) => {
      const preview = dragPreview?.placeId === place.id ? dragPreview : null;
      const center = preview?.coordinates ?? place.coordinates;
      const radiusMeters = preview?.radiusMeters ?? place.station_search_radius;
      return {
        type: "Feature",
        geometry: { type: "Polygon", coordinates: [circleRing(center, radiusMeters)] },
        properties: { placeId: place.id, selected: place.id === selectedPlaceId },
      };
    }),
  };
}

/** Properties of a line between a place and a station, or between two stations on foot. */
export interface StationWayLineProperties {
  arrivalPlaceId: TripPlaceId;
}

/** Turns coordinates into the `[longitude, latitude]` pair GeoJSON expects. */
function position(coordinates: GeoCoordinates): [number, number] {
  return [coordinates.longitude, coordinates.latitude];
}

/** Returns a train's label for display, such as "ICE 145". */
function trainName(ride: TrainRide): string {
  return `${ride.train_label.category} ${ride.train_label.number}`.trim();
}

/** Returns the name of a station of a leg plan. */
function stationName(legPlan: LegPlan, stationIndex: number): string {
  return legPlan.station_table[stationIndex]?.name ?? "Unknown station";
}

/** Returns a moment on the local clock of a station of a leg plan. */
function stationTime(legPlan: LegPlan, stationIndex: number, moment: number): string {
  return formatTimeInZone(moment, legPlan.station_table[stationIndex]?.time_zone ?? "UTC");
}

/** Returns the rides of a leg plan's chosen journey in travel order. */
function chosenRides(legPlan: LegPlan): TrainRide[] {
  const journey = legPlan.chosen_journey;
  return [journey.first_ride, ...journey.later_rides.map((later) => later.ride)];
}

/**
 * Returns one line per planned train ride, from the boarding station through every station the
 * train passes to the alighting station, curved like a railway. Legs that are still planning or
 * have no journey draw nothing.
 */
export function rideLines(viewState: TripViewState): FeatureCollection<LineString, RideLineProperties> {
  const features: FeatureCollection<LineString, RideLineProperties>["features"] = [];
  for (const leg of itineraryRows(viewState.trip).legs) {
    const legState = legViewState(viewState, leg.arrivalPlace.id);
    if (legState.status !== "found") {
      continue;
    }
    const legPlan = legState.legPlan;
    for (const ride of chosenRides(legPlan)) {
      const boarding = legPlan.station_table[ride.boarding_station];
      const alighting = legPlan.station_table[ride.alighting_station];
      if (boarding === undefined || alighting === undefined) {
        continue;
      }
      const tooltip = [
        trainName(ride),
        `${stationTime(legPlan, ride.boarding_station, ride.departure_time)} ${boarding.name}`,
        `${stationTime(legPlan, ride.alighting_station, ride.arrival_time)} ${alighting.name}`,
      ].join("\n");
      features.push({
        type: "Feature",
        geometry: {
          type: "LineString",
          coordinates: curvedRidePath([
            boarding.coordinates,
            ...ride.intermediate_station_coordinates,
            alighting.coordinates,
          ]),
        },
        properties: { arrivalPlaceId: leg.arrivalPlace.id, offersBerths: ride.offers_berths, tooltip },
      });
    }
  }
  return { type: "FeatureCollection", features };
}

/**
 * Returns a point for every transfer of the planned journeys, and a smaller one for the first
 * and last station of each journey. A change to another station gets a point at both stations.
 */
export function transferPoints(viewState: TripViewState): FeatureCollection<Point, TransferPointProperties> {
  const features: FeatureCollection<Point, TransferPointProperties>["features"] = [];
  const addPoint = (legPlan: LegPlan, stationIndex: number, kind: TransferPointKind, tooltip: string[]) => {
    const station = legPlan.station_table[stationIndex];
    if (station !== undefined) {
      features.push({
        type: "Feature",
        geometry: { type: "Point", coordinates: position(station.coordinates) },
        properties: { kind, tooltip: tooltip.join("\n") },
      });
    }
  };
  for (const leg of itineraryRows(viewState.trip).legs) {
    const legState = legViewState(viewState, leg.arrivalPlace.id);
    if (legState.status !== "found") {
      continue;
    }
    const legPlan = legState.legPlan;
    const rides = chosenRides(legPlan);
    const first = rides[0]!;
    const last = rides.at(-1)!;
    addPoint(legPlan, first.boarding_station, "journey-end", [
      `Board ${trainName(first)}`,
      `${stationTime(legPlan, first.boarding_station, first.departure_time)} ${stationName(legPlan, first.boarding_station)}`,
    ]);
    legPlan.chosen_journey.later_rides.forEach((later, index) => {
      const arriving = rides[index]!;
      const departing = later.ride;
      const arrival = `Arrive ${stationTime(legPlan, arriving.alighting_station, arriving.arrival_time)} on ${trainName(arriving)}`;
      const departure = `Leave ${stationTime(legPlan, departing.boarding_station, departing.departure_time)} on ${trainName(departing)}`;
      const time = `${formatDuration(later.transfer_duration)} to change`;
      if (arriving.alighting_station === departing.boarding_station) {
        addPoint(legPlan, arriving.alighting_station, "transfer", [
          `Transfer at ${stationName(legPlan, arriving.alighting_station)}`,
          arrival,
          departure,
          time,
        ]);
      } else {
        const change = `Change from ${stationName(legPlan, arriving.alighting_station)} to ${stationName(legPlan, departing.boarding_station)}`;
        addPoint(legPlan, arriving.alighting_station, "transfer", [change, arrival, departure, `${time}, including the way between the stations`]);
        addPoint(legPlan, departing.boarding_station, "transfer", [change, arrival, departure, `${time}, including the way between the stations`]);
      }
    });
    addPoint(legPlan, last.alighting_station, "journey-end", [
      `Leave ${trainName(last)}`,
      `${stationTime(legPlan, last.alighting_station, last.arrival_time)} ${stationName(legPlan, last.alighting_station)}`,
    ]);
  }
  return { type: "FeatureCollection", features };
}

/**
 * Returns the ways of each planned journey that are not on a train: from the departure place to
 * the first station, from the last station to the arrival place, between two stations at a
 * transfer, and a footpath between two stations at the end.
 */
export function stationWayLines(
  viewState: TripViewState,
): FeatureCollection<LineString, StationWayLineProperties> {
  const features: FeatureCollection<LineString, StationWayLineProperties>["features"] = [];
  for (const leg of itineraryRows(viewState.trip).legs) {
    const legState = legViewState(viewState, leg.arrivalPlace.id);
    if (legState.status !== "found") {
      continue;
    }
    const { chosen_journey: journey, station_table: stations } = legState.legPlan;
    const lastRide = journey.later_rides.at(-1)?.ride ?? journey.first_ride;
    const firstStation = stations[journey.first_ride.boarding_station];
    const alightingStation = stations[lastRide.alighting_station];
    const footpathStation =
      journey.arrival_footpath === null ? undefined : stations[journey.arrival_footpath.to_station];
    const endStation = footpathStation ?? alightingStation;
    const ways: [GeoCoordinates, GeoCoordinates][] = [];
    if (firstStation !== undefined) {
      ways.push([leg.departurePlace.coordinates, firstStation.coordinates]);
    }
    // A transfer to another station has a way between the two.
    let previousRide = journey.first_ride;
    for (const { ride } of journey.later_rides) {
      const from = stations[previousRide.alighting_station];
      const to = stations[ride.boarding_station];
      if (from !== undefined && to !== undefined && previousRide.alighting_station !== ride.boarding_station) {
        ways.push([from.coordinates, to.coordinates]);
      }
      previousRide = ride;
    }
    if (footpathStation !== undefined && alightingStation !== undefined) {
      ways.push([alightingStation.coordinates, footpathStation.coordinates]);
    }
    if (endStation !== undefined) {
      ways.push([endStation.coordinates, leg.arrivalPlace.coordinates]);
    }
    for (const [from, to] of ways) {
      features.push({
        type: "Feature",
        geometry: { type: "LineString", coordinates: [position(from), position(to)] },
        properties: { arrivalPlaceId: leg.arrivalPlace.id },
      });
    }
  }
  return { type: "FeatureCollection", features };
}

/** Returns the south-west and north-east corners around every place, or `null` without places. */
export function placeBounds(trip: Trip): [[number, number], [number, number]] | null {
  const places = tripPlaces(trip.itinerary);
  if (places.length === 0) {
    return null;
  }
  const longitudes = places.map((place) => place.coordinates.longitude);
  const latitudes = places.map((place) => place.coordinates.latitude);
  return [
    [Math.min(...longitudes), Math.min(...latitudes)],
    [Math.max(...longitudes), Math.max(...latitudes)],
  ];
}

/**
 * Asks the map to move to a place. The number grows with every request, so asking for the same
 * place twice moves the map twice.
 */
export interface PlaceFocusRequest {
  placeId: TripPlaceId;
  requestNumber: number;
}
