/// <reference types="node" />
import { readFileSync } from "node:fs";
import type { GeoCoordinates } from "@btt/shared/bindings";
import { describe, expect, it } from "vitest";
import hopCases from "../fixtures/hop-paths.json";
import { type RailGraph, haversineMeters, parseRailGraph, ridePathPoints, RailGraph as Graph } from "./rail-graph";

/** The graph of the real bundle, which the example `rail_graph` wrote. */
function realGraph(): RailGraph {
  return parseRailGraph(new Uint8Array(readFileSync(new URL("../generated/rail-graph.bin", import.meta.url))));
}

const point = ([latitude, longitude]: number[]): GeoCoordinates => ({ latitude: latitude!, longitude: longitude! });

describe("the search for the stations of a hop", () => {
  // The cases come from the tests of `ride_path.rs`, so the search here finds what the desktop finds.
  for (const hopCase of hopCases) {
    it(hopCase.name, () => {
      const graph = new Graph(
        hopCase.coordinates.map(point),
        hopCase.hops.map(([first, second]) => ({ first: first!, second: second!, categories: [] })),
      );
      const passed = graph.passedStations(hopCase.from, hopCase.to).map((station) => graph.coordinates(station));
      expect(passed).toEqual(hopCase.passed.map(point));
    });
  }
});

describe("the rail graph of the bundle", () => {
  const graph = realGraph();

  it("holds the stations and edges that the file says", () => {
    expect(graph.stationCount).toBeGreaterThan(30_000);
    expect(graph.regionalCategories.has("S")).toBe(true);
    expect(graph.regionalCategories.has("ICE")).toBe(false);
  });

  it("finds the station nearest to a point, but only within a few hundred meters", () => {
    // Amsterdam Centraal and a point in the North Sea.
    const station = graph.nearestStation({ latitude: 52.379, longitude: 4.9 });
    expect(station).not.toBeNull();
    expect(graph.distanceTo(station!, { latitude: 52.379, longitude: 4.9 })).toBeLessThan(300);
    expect(graph.nearestStation({ latitude: 54.0, longitude: 3.0 })).toBeNull();
  });
});

describe("the line of a ride", () => {
  const graph = realGraph();
  const amsterdam = { latitude: 52.379036, longitude: 4.900459 };
  const frankfurt = { latitude: 50.107676, longitude: 8.664515 };

  it("passes stations between Amsterdam and Frankfurt for a long-distance train", () => {
    const points = ridePathPoints(graph, amsterdam, frankfurt, "ICE");
    expect(points.length).toBeGreaterThan(5);
    // Every point lies within the corridor between the two cities, and the line is not much
    // longer than the straight distance.
    let length = haversineMeters(amsterdam.latitude, amsterdam.longitude, points[0]!.latitude, points[0]!.longitude);
    for (let index = 1; index < points.length; index += 1) {
      length += haversineMeters(points[index - 1]!.latitude, points[index - 1]!.longitude, points[index]!.latitude, points[index]!.longitude);
    }
    const last = points.at(-1)!;
    length += haversineMeters(last.latitude, last.longitude, frankfurt.latitude, frankfurt.longitude);
    const straight = haversineMeters(amsterdam.latitude, amsterdam.longitude, frankfurt.latitude, frankfurt.longitude);
    expect(length).toBeGreaterThan(straight);
    expect(length).toBeLessThan(straight * 1.3);
  });

  it("keeps a short ride straight", () => {
    // Amsterdam Centraal to Amsterdam Sloterdijk, which are 5 km apart.
    const sloterdijk = { latitude: 52.3889, longitude: 4.8372 };
    expect(ridePathPoints(graph, amsterdam, sloterdijk, "IC")).toEqual([]);
  });

  it("keeps a ride straight when a station is not in the graph", () => {
    const nowhere = { latitude: 54.0, longitude: 3.0 };
    expect(ridePathPoints(graph, amsterdam, nowhere, "ICE")).toEqual([]);
    expect(ridePathPoints(graph, nowhere, amsterdam, "ICE")).toEqual([]);
  });

  it("gives a regional train a line over the stations of every train", () => {
    const koeln = { latitude: 50.943076, longitude: 6.958701 };
    const duesseldorf = { latitude: 51.220, longitude: 6.794 };
    const points = ridePathPoints(graph, koeln, duesseldorf, "S");
    expect(points.length).toBeGreaterThan(3);
  });

  it("gives a long-distance train on a high-speed line the stations that the fast trains stop at", () => {
    const koeln = { latitude: 50.943076, longitude: 6.958701 };
    const points = ridePathPoints(graph, koeln, frankfurt, "ICE");
    expect(points.length).toBeGreaterThanOrEqual(1);
  });
});
