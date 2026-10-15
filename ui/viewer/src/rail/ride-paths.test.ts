/// <reference types="node" />
import { readFileSync } from "node:fs";
import type { TripSnapshot } from "@btt/shared/bindings";
import { describe, expect, it } from "vitest";
import demo from "../fixtures/demo.json";
import rides from "../fixtures/ride-paths.json";
import { parseRailGraph, ridePathPoints } from "./rail-graph";
import { withRidePaths } from "./ride-paths";

type Point = [number, number];

const graph = parseRailGraph(new Uint8Array(readFileSync(new URL("../generated/rail-graph.bin", import.meta.url))));

const toPoint = ([latitude, longitude]: number[]) => ({ latitude: latitude!, longitude: longitude! });

/** Returns the distance in meters from a point to a line, on a flat map around the point. */
function distanceToLine(point: Point, line: Point[]): number {
  const metersPerDegree = 111_320;
  const scale = Math.cos((point[0] * Math.PI) / 180);
  const flat = (candidate: Point): Point => [(candidate[1] - point[1]) * metersPerDegree * scale, (candidate[0] - point[0]) * metersPerDegree];
  let best = Number.POSITIVE_INFINITY;
  for (let index = 0; index + 1 < line.length; index += 1) {
    const [ax, ay] = flat(line[index]!);
    const [bx, by] = flat(line[index + 1]!);
    const lengthSquared = (bx - ax) ** 2 + (by - ay) ** 2;
    const along = lengthSquared === 0 ? 0 : Math.max(0, Math.min(1, (-ax * (bx - ax) - ay * (by - ay)) / lengthSquared));
    best = Math.min(best, Math.hypot(ax + along * (bx - ax), ay + along * (by - ay)));
  }
  return best;
}

/** Returns the mean distance of the points of one line to the other line, in meters. */
function meanDistance(points: Point[], line: Point[]): number {
  return points.reduce((total, point) => total + distanceToLine(point, line), 0) / points.length;
}

describe("the lines of real rides", () => {
  // The rides come from `cargo run -p btt-planner --example ride_path_samples`, with the stations
  // that the desktop map draws each of them through.
  const rows = rides
    .filter((ride) => ride.path.length > 0)
    .map((ride) => {
      const desktop: Point[] = [ride.from as Point, ...(ride.path as Point[]), ride.to as Point];
      const found = ridePathPoints(graph, toPoint(ride.from), toPoint(ride.to), ride.category);
      const viewer: Point[] = [ride.from as Point, ...found.map((point): Point => [point.latitude, point.longitude]), ride.to as Point];
      // The desktop path of a ride can hold a station with a wrong position, so the lines are
      // compared by the larger of the two mean distances and the test looks at the middle ones.
      return { found: found.length, distance: Math.max(meanDistance(viewer, desktop), meanDistance(desktop, viewer)) };
    });

  it("finds a line for nearly every ride that the desktop draws along the track", () => {
    expect(rows.filter((row) => row.found > 0).length / rows.length).toBeGreaterThan(0.95);
  });

  it("finds the line the desktop draws for three rides in four, and the same line for most", () => {
    const within = (meters: number) => rows.filter((row) => row.distance < meters).length / rows.length;
    // Some trains run along another track than the shortest one of their category, which a ride
    // that holds only its two end stations cannot tell. Those make up the rest.
    expect(within(1_000)).toBeGreaterThan(0.65);
    expect(within(3_000)).toBeGreaterThan(0.7);
    expect(within(30_000)).toBeGreaterThan(0.9);
    const sorted = rows.map((row) => row.distance).sort((a, b) => a - b);
    expect(sorted[Math.floor(sorted.length / 2)]).toBeLessThan(500);
  });

  it("finds the lines of all rides quickly", () => {
    const started = Date.now();
    for (const ride of rides) {
      ridePathPoints(graph, toPoint(ride.from), toPoint(ride.to), ride.category);
    }
    // A few hundred milliseconds in practice. The limit leaves room for a slow machine.
    expect(Date.now() - started).toBeLessThan(5_000);
  });
});

describe("the rides of a snapshot", () => {
  const snapshot = demo.document.snapshot as TripSnapshot;
  const enriched = withRidePaths(snapshot, graph);
  const chosenRides = (trip: TripSnapshot) =>
    trip.legs.flatMap((leg) => {
      const plan = leg.result.Found;
      return plan === undefined ? [] : [plan.chosen_journey.first_ride, ...plan.chosen_journey.later_rides.map((later) => later.ride)];
    });

  it("gets the stations that the long rides of the chosen journeys pass", () => {
    const long = chosenRides(enriched).filter((ride) => (ride.path_coordinates ?? []).length > 0);
    expect(long.length).toBeGreaterThan(0);
  });

  it("changes nothing else, and leaves the snapshot it was given as it was", () => {
    expect(chosenRides(snapshot).every((ride) => (ride.path_coordinates ?? []).length === 0)).toBe(true);
    const stripped = JSON.parse(JSON.stringify(enriched)) as TripSnapshot;
    for (const ride of chosenRides(stripped)) {
      ride.path_coordinates = [];
    }
    expect(stripped).toEqual(snapshot);
  });

  it("leaves the alternatives and the legs without a journey as they are", () => {
    const [first, , third] = enriched.legs;
    expect(first!.result.Found!.alternative_journeys).toEqual(snapshot.legs[0]!.result.Found!.alternative_journeys);
    expect(third).toEqual(snapshot.legs[2]);
  });
});
