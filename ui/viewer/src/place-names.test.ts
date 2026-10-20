import { describe, expect, it } from "vitest";
import type { Itinerary, TripPlace, TripSnapshot } from "@btt/shared/bindings";
import demo from "./fixtures/demo.json";
import { nameEmptyPlacesByStation } from "./place-names";

/** Returns the places of a snapshot in travel order. */
function placesOf(snapshot: TripSnapshot): TripPlace[] {
  const itinerary: Itinerary = snapshot.trip.itinerary;
  if (itinerary === "Empty") {
    return [];
  }
  const route = itinerary.Route;
  if (route === undefined) {
    return [itinerary.OriginOnly];
  }
  const { origin, via_places, destination } = route;
  return [origin, ...via_places.map((via) => via.place), destination];
}

/** Returns the snapshot with every place nameless, as a link without place names holds it. */
function withoutPlaceNames(snapshot: TripSnapshot): TripSnapshot {
  const itinerary = snapshot.trip.itinerary;
  if (itinerary === "Empty" || "OriginOnly" in itinerary) {
    return snapshot;
  }
  const blank = (place: TripPlace): TripPlace => ({ ...place, name: "" });
  const route = itinerary.Route;
  return {
    ...snapshot,
    trip: {
      ...snapshot.trip,
      itinerary: {
        Route: {
          ...route,
          origin: blank(route.origin),
          via_places: route.via_places.map((via) => ({ ...via, place: blank(via.place) })),
          destination: blank(route.destination),
        },
      },
    },
  };
}

describe("naming the places of a snapshot by their nearest station", () => {
  const snapshot = demo.document.snapshot as unknown as TripSnapshot;
  const stationNames = new Set(snapshot.legs.flatMap(({ result }) => result.Found?.station_table.map((s) => s.name) ?? []));

  it("gives every nameless place the name of a station of the plans", () => {
    const named = nameEmptyPlacesByStation(withoutPlaceNames(snapshot));
    for (const place of placesOf(named)) {
      expect(stationNames.has(place.name)).toBe(true);
    }
  });

  it("keeps the names that are there", () => {
    expect(nameEmptyPlacesByStation(snapshot)).toEqual(snapshot);
  });

  it("leaves a place nameless when the plans have no station", () => {
    const nameless = withoutPlaceNames({ ...snapshot, legs: [] });
    expect(placesOf(nameEmptyPlacesByStation(nameless)).every((place) => place.name === "")).toBe(true);
  });
});
