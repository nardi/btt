import { describe, expect, it } from "vitest";

import type { LegPlan } from "./bindings";
import { placeBounds, radiusCircles, rideLines } from "./map-features";
import { amsterdamViennaRome } from "./test-fixtures";
import type { LegViewState, TripViewState } from "./trip-source";

function viewState(legStates: [number, LegViewState][]): TripViewState {
  return {
    trip: amsterdamViennaRome(),
    legStatesByArrivalPlace: new Map(legStates),
    notices: [],
    errorMessage: null,
  };
}

const viennaToRome: LegPlan = {
  chosen_journey: {
    access_duration: 0,
    first_ride: {
      boarding_station: 0,
      departure_time: 0,
      alighting_station: 1,
      arrival_time: 600,
      train_label: { category: "NJ", number: "40233" },
    },
    later_rides: [],
    arrival_footpath: null,
    egress_duration: 0,
  },
  alternative_journeys: [],
  station_table: [
    { name: "Wien Hbf", coordinates: { latitude: 48.1852, longitude: 16.3761 }, time_zone: "Europe/Vienna" },
    { name: "Roma Termini", coordinates: { latitude: 41.901, longitude: 12.5007 }, time_zone: "Europe/Rome" },
  ],
};

describe("radiusCircles", () => {
  it("draws one circle per place and marks the selected one", () => {
    const circles = radiusCircles(amsterdamViennaRome(), 2, null);
    expect(circles.features).toHaveLength(3);
    expect(circles.features.map((feature) => feature.properties.selected)).toEqual([false, true, false]);
  });

  it("follows a place that is being dragged", () => {
    const preview = { placeId: 1, coordinates: { latitude: 0, longitude: 0 }, radiusMeters: 1000 };
    const circles = radiusCircles(amsterdamViennaRome(), null, preview);
    const firstPoint = circles.features[0]!.geometry.coordinates[0]![0]!;
    expect(Math.abs(firstPoint[1]!)).toBeLessThan(0.01);
  });
});

describe("rideLines", () => {
  it("draws planned rides and skips pending legs", () => {
    const lines = rideLines(viewState([[3, { status: "found", legPlan: viennaToRome }]]));
    expect(lines.features).toHaveLength(1);
    expect(lines.features[0]!.properties).toEqual({ arrivalPlaceId: 3, overnight: true });
    expect(lines.features[0]!.geometry.coordinates[0]).toEqual([16.3761, 48.1852]);
  });
});

describe("placeBounds", () => {
  it("spans all places", () => {
    expect(placeBounds(amsterdamViennaRome())).toEqual([
      [4.9003, 41.901],
      [16.3761, 52.3789],
    ]);
  });
});
