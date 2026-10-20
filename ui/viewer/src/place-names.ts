// Names the places of a snapshot whose link left out the name.

import type { GeoCoordinates, TripPlace, TripSnapshot } from "@btt/shared/bindings";
import { haversineMeters } from "@btt/shared/geo";

/** A station of the plans of a snapshot, as far as naming a place needs it. */
interface NamedStation {
  readonly name: string;
  readonly coordinates: GeoCoordinates;
}

/** Returns the stations of every plan of the snapshot. */
function stationsOf(snapshot: TripSnapshot): NamedStation[] {
  return snapshot.legs.flatMap(({ result }) => result.Found?.station_table ?? []);
}

/** Returns the name of the station nearest to a point, or `null` when there is no station. */
function nearestStationName(point: GeoCoordinates, stations: readonly NamedStation[]): string | null {
  let nearestName: string | null = null;
  let nearestDistance = Infinity;
  for (const station of stations) {
    const distance = haversineMeters(point, station.coordinates);
    if (distance < nearestDistance) {
      nearestDistance = distance;
      nearestName = station.name;
    }
  }
  return nearestName;
}

/**
 * Returns the snapshot with the name of the nearest station on every place that has an empty
 * name, which is how a link without place names holds them. A place keeps its empty name when
 * the snapshot has no station.
 */
export function nameEmptyPlacesByStation(snapshot: TripSnapshot): TripSnapshot {
  const itinerary = snapshot.trip.itinerary;
  if (itinerary === "Empty") {
    return snapshot;
  }
  const stations = stationsOf(snapshot);
  const named = (place: TripPlace): TripPlace => {
    const name = place.name === "" ? nearestStationName(place.coordinates, stations) : null;
    return name === null ? place : { ...place, name };
  };
  const route = itinerary.Route;
  const namedItinerary =
    route === undefined
      ? { OriginOnly: named(itinerary.OriginOnly) }
      : {
          Route: {
            ...route,
            origin: named(route.origin),
            via_places: route.via_places.map((via) => ({ ...via, place: named(via.place) })),
            destination: named(route.destination),
          },
        };
  return { ...snapshot, trip: { ...snapshot.trip, itinerary: namedItinerary } };
}
