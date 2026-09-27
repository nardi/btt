// GeoJSON for the map layers, derived from a trip and its leg plans.

import type { FeatureCollection, LineString, Polygon } from "geojson";

import type { GeoCoordinates, Trip, TripPlaceId } from "./bindings";
import { circleRing } from "./geo";
import { isOvernight, itineraryRows, tripPlaces } from "./itinerary";
import { legViewState, type TripViewState } from "./trip-source";

/** Properties of a station search radius circle. */
export interface RadiusCircleProperties {
  placeId: TripPlaceId;
  selected: boolean;
}

/** Properties of a ride line. */
export interface RideLineProperties {
  arrivalPlaceId: TripPlaceId;
  overnight: boolean;
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

/**
 * Returns one straight line per planned train ride, from the boarding to the alighting station.
 * Legs that are still planning or have no journey draw nothing.
 */
export function rideLines(viewState: TripViewState): FeatureCollection<LineString, RideLineProperties> {
  const features: FeatureCollection<LineString, RideLineProperties>["features"] = [];
  for (const leg of itineraryRows(viewState.trip).legs) {
    const legState = legViewState(viewState, leg.arrivalPlace.id);
    if (legState.status !== "found") {
      continue;
    }
    const { chosen_journey: journey, station_table: stations } = legState.legPlan;
    const rides = [journey.first_ride, ...journey.later_rides.map((later) => later.ride)];
    for (const ride of rides) {
      const boarding = stations[ride.boarding_station];
      const alighting = stations[ride.alighting_station];
      if (boarding === undefined || alighting === undefined) {
        continue;
      }
      features.push({
        type: "Feature",
        geometry: {
          type: "LineString",
          coordinates: [
            [boarding.coordinates.longitude, boarding.coordinates.latitude],
            [alighting.coordinates.longitude, alighting.coordinates.latitude],
          ],
        },
        properties: { arrivalPlaceId: leg.arrivalPlace.id, overnight: isOvernight(leg.travelMode) },
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
