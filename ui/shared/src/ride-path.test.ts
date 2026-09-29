import { describe, expect, it } from "vitest";

import { curvedRidePath } from "./ride-path";

/** Returns whether the path contains a point within about a meter of the station. */
function passesThrough(path: [number, number][], station: { latitude: number; longitude: number }): boolean {
  return path.some(
    ([longitude, latitude]) =>
      Math.abs(longitude - station.longitude) < 1e-5 && Math.abs(latitude - station.latitude) < 1e-5,
  );
}

describe("curvedRidePath", () => {
  it("keeps two stations a straight line", () => {
    const path = curvedRidePath([
      { latitude: 52.0, longitude: 5.0 },
      { latitude: 52.5, longitude: 6.0 },
    ]);
    expect(path[0]).toEqual([5.0, 52.0]);
    expect(path.at(-1)![0]).toBeCloseTo(6.0, 9);
    expect(path.at(-1)![1]).toBeCloseTo(52.5, 9);
    for (const [longitude, latitude] of path) {
      // Every point lies on the line between the two stations.
      expect(latitude - 52.0).toBeCloseTo((longitude - 5.0) * 0.5, 6);
    }
  });

  it("passes through every station", () => {
    const stations = [
      { latitude: 52.37, longitude: 4.9 },
      { latitude: 52.09, longitude: 5.11 },
      { latitude: 51.44, longitude: 5.47 },
      { latitude: 50.94, longitude: 6.96 },
    ];
    const path = curvedRidePath(stations);
    for (const station of stations) {
      expect(passesThrough(path, station)).toBe(true);
    }
    expect(path.length).toBeGreaterThan(stations.length);
  });

  it("does not loop back where stations are unevenly spaced", () => {
    // A stop 2 km after the start, then a long run that turns slightly.
    const path = curvedRidePath([
      { latitude: 50.0, longitude: 8.0 },
      { latitude: 50.0, longitude: 8.028 },
      { latitude: 50.2, longitude: 9.0 },
    ]);
    const longitudes = path.map(([longitude]) => longitude);
    for (let index = 1; index < longitudes.length; index += 1) {
      expect(longitudes[index]!).toBeGreaterThanOrEqual(longitudes[index - 1]! - 1e-9);
    }
  });

  it("keeps a sharp corner where the train reverses", () => {
    // In from the west, out again to the south-west, as at a terminus.
    const west = { latitude: 50.1, longitude: 8.5 };
    const terminus = { latitude: 50.1, longitude: 8.66 };
    const southWest = { latitude: 50.05, longitude: 8.55 };
    const path = curvedRidePath([west, terminus, southWest]);
    const terminusIndex = path.findIndex(
      ([longitude, latitude]) =>
        Math.abs(longitude - terminus.longitude) < 1e-5 && Math.abs(latitude - terminus.latitude) < 1e-5,
    );
    expect(terminusIndex).toBeGreaterThan(0);
    // The approach stays on the straight line from the west, without swinging out.
    for (const [, latitude] of path.slice(0, terminusIndex + 1)) {
      expect(latitude).toBeCloseTo(50.1, 6);
    }
  });
});
