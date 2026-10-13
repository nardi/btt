import { describe, expect, it } from "vitest";

import type { LegPlan } from "./bindings";
import {
  intermediateStopPoints,
  placeBounds,
  radiusCircles,
  rideLines,
  stationWayLines,
  transferPoints,
} from "./map-features";
import { amsterdamViennaRome } from "./test-fixtures";
import type { LegViewState, TripViewState } from "./trip-source";

function viewState(legStates: [number, LegViewState][]): TripViewState {
  return {
    trip: amsterdamViennaRome(),
    legStatesByArrivalPlace: new Map(legStates),
    nearbyDaysByArrivalPlace: new Map(),
    notices: [],
    errorMessage: null,
    passScenarios: null,
    livePricesByJourney: new Map(),
    extrasPendingArrivalPlaces: new Set(),
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
    sleeping_ride: null,
  },
  alternative_journeys: [],
  follows_traveller_choice: false,
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
    const line = lines.features[0]!;
    expect(line.properties).toMatchObject({ arrivalPlaceId: 3, legPosition: 2, night: true });
    expect(line.properties.tooltip.split("\n")[0]).toBe("NJ 40233");
    // The curve starts and ends at the stations and passes the station in between.
    const coordinates = line.geometry.coordinates;
    expect(coordinates[0]).toEqual([16.3761, 48.1852]);
    expect(coordinates.at(-1)![0]).toBeCloseTo(12.5007, 6);
    expect(coordinates.at(-1)![1]).toBeCloseTo(41.901, 6);
    expect(
      coordinates.some(([longitude, latitude]) => Math.abs(longitude! - 11.2) < 1e-5 && Math.abs(latitude! - 43.7) < 1e-5),
    ).toBe(true);
  });
});

describe("rideLines with passed stations", () => {
  it("draws the line through the path instead of only the stops", () => {
    const passed = { latitude: 46.0, longitude: 13.5 };
    const withPath: LegPlan = {
      ...viennaToRome,
      chosen_journey: {
        ...viennaToRome.chosen_journey,
        first_ride: {
          ...viennaToRome.chosen_journey.first_ride,
          path_coordinates: [passed, { latitude: 43.7, longitude: 11.2 }],
        },
      },
    };
    const line = rideLines(viewState([[3, { status: "found", legPlan: withPath }]])).features[0]!;
    expect(
      line.geometry.coordinates.some(([longitude, latitude]) => Math.abs(longitude! - 13.5) < 1e-5 && Math.abs(latitude! - 46.0) < 1e-5),
    ).toBe(true);
    // The stop dots still come from the stops only.
    expect(intermediateStopPoints(viewState([[3, { status: "found", legPlan: withPath }]])).features).toHaveLength(1);
  });
});

describe("intermediateStopPoints", () => {
  it("marks the stops between boarding and alighting", () => {
    const points = intermediateStopPoints(viewState([[3, { status: "found", legPlan: viennaToRome }]]));
    expect(points.features).toHaveLength(1);
    expect(points.features[0]!.geometry.coordinates).toEqual([11.2, 43.7]);
    expect(points.features[0]!.properties).toMatchObject({ legPosition: 2, night: true });
  });
});

describe("transferPoints", () => {
  it("marks the start, the end and every transfer of a journey", () => {
    const withTransfer: LegPlan = {
      ...viennaToRome,
      chosen_journey: {
        ...viennaToRome.chosen_journey,
        first_ride: { ...viennaToRome.chosen_journey.first_ride, alighting_station: 2, arrival_time: 300 },
        later_rides: [
          {
            transfer_duration: 25,
            ride: {
              ...viennaToRome.chosen_journey.first_ride,
              boarding_station: 2,
              departure_time: 325,
              train_label: { category: "FR", number: "9541" },
              offers_berths: false,
              intermediate_station_coordinates: [],
            },
          },
        ],
      },
      station_table: [
        ...viennaToRome.station_table,
        { name: "Firenze SMN", coordinates: { latitude: 43.7765, longitude: 11.2478 }, time_zone: "Europe/Rome" },
      ],
    };
    const points = transferPoints(viewState([[3, { status: "found", legPlan: withTransfer }]]));
    expect(points.features.map((point) => point.properties.kind)).toEqual(["journey-end", "transfer", "journey-end"]);
    const transfer = points.features[1]!.properties.tooltip.split("\n");
    expect(transfer[0]).toBe("Transfer at Firenze SMN");
    expect(transfer[2]).toContain("FR 9541");
    expect(transfer[3]).toBe("25 min to change");
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
