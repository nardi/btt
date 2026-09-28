import { describe, expect, it } from "vitest";

import type { LegPlan } from "./bindings";
import { placeBounds, radiusCircles, rideLines, stationWayLines } from "./map-features";
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
    access: { duration: 0, distance_meters: 0, mode: "Walk" },
    first_ride: {
      boarding_station: 0,
      departure_time: 0,
      alighting_station: 1,
      arrival_time: 600,
      train_label: { category: "NJ", number: "40233" },
      offers_berths: true,
      intermediate_station_coordinates: [{ latitude: 43.7, longitude: 11.2 }],
    },
    later_rides: [],
    arrival_footpath: null,
    egress: { duration: 0, distance_meters: 0, mode: "Walk" },
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
    expect(lines.features[0]!.properties).toEqual({ arrivalPlaceId: 3, offersBerths: true });
    expect(lines.features[0]!.geometry.coordinates).toEqual([
      [16.3761, 48.1852],
      [11.2, 43.7],
      [12.5007, 41.901],
    ]);
  });
});

describe("stationWayLines", () => {
  it("connects the places with the first and last station", () => {
    const lines = stationWayLines(viewState([[3, { status: "found", legPlan: viennaToRome }]]));
    expect(lines.features).toHaveLength(2);
    expect(lines.features[0]!.geometry.coordinates[1]).toEqual([16.3761, 48.1852]);
    expect(lines.features[1]!.geometry.coordinates[0]).toEqual([12.5007, 41.901]);
  });

  it("adds the footpath between two stations at the end", () => {
    const withFootpath: LegPlan = {
      ...viennaToRome,
      chosen_journey: {
        ...viennaToRome.chosen_journey,
        arrival_footpath: {
          to_station: 2,
          way: { duration: 20, distance_meters: 2600, mode: "LocalTransport" },
        },
      },
      station_table: [
        ...viennaToRome.station_table,
        { name: "Roma Tiburtina", coordinates: { latitude: 41.9106, longitude: 12.5305 }, time_zone: "Europe/Rome" },
      ],
    };
    const lines = stationWayLines(viewState([[3, { status: "found", legPlan: withFootpath }]]));
    expect(lines.features).toHaveLength(3);
    expect(lines.features[1]!.geometry.coordinates).toEqual([
      [12.5007, 41.901],
      [12.5305, 41.9106],
    ]);
    expect(lines.features[2]!.geometry.coordinates[0]).toEqual([12.5305, 41.9106]);
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
