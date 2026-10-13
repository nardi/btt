import { describe, expect, it } from "vitest";

import { octilinearPoints, type RouteMapOptions, type RouteStop, routeMap } from "./route-map";

const options: RouteMapOptions = { width: 120, height: 64, padding: 10, grid: 8, bendRadius: 6 };

function stop(latitude: number, longitude: number, night = false): RouteStop {
  return { coordinates: { latitude, longitude }, arrives_by_night: night };
}

const amsterdam = stop(52.3789, 4.9003);
const vienna = stop(48.1852, 16.3761);
const rome = stop(41.901, 12.5007, true);

/** Returns whether every piece of a line runs along an axis or at 45 degrees. */
function isOctilinear(points: readonly { x: number; y: number }[]): boolean {
  return points.every((point, index) => {
    if (index === 0) {
      return true;
    }
    const dx = Math.abs(point.x - points[index - 1]!.x);
    const dy = Math.abs(point.y - points[index - 1]!.y);
    return dx < 1e-9 || dy < 1e-9 || Math.abs(dx - dy) < 1e-9;
  });
}

describe("octilinearPoints", () => {
  it("keeps a horizontal, a vertical and a diagonal line as one piece", () => {
    expect(octilinearPoints({ x: 0, y: 0 }, { x: 16, y: 0 })).toHaveLength(2);
    expect(octilinearPoints({ x: 0, y: 0 }, { x: 0, y: -16 })).toHaveLength(2);
    expect(octilinearPoints({ x: 0, y: 0 }, { x: 16, y: 16 })).toHaveLength(2);
  });

  it("joins other stations with a straight piece, a diagonal and a straight piece", () => {
    const points = octilinearPoints({ x: 0, y: 0 }, { x: 40, y: 16 });
    expect(points).toEqual([
      { x: 0, y: 0 },
      { x: 12, y: 0 },
      { x: 28, y: 16 },
      { x: 40, y: 16 },
    ]);
    expect(isOctilinear(points)).toBe(true);
  });

  it("works in every direction", () => {
    for (const to of [{ x: -40, y: 16 }, { x: 40, y: -16 }, { x: -16, y: -40 }, { x: 16, y: 40 }]) {
      const points = octilinearPoints({ x: 0, y: 0 }, to);
      expect(isOctilinear(points)).toBe(true);
      expect(points.at(-1)).toEqual(to);
    }
  });
});

describe("routeMap", () => {
  it("returns nothing for a trip with no places", () => {
    expect(routeMap([], options)).toBeNull();
  });

  it("draws one station and no line for a trip with one place", () => {
    const map = routeMap([amsterdam], options)!;
    expect(map.stations).toHaveLength(1);
    expect(map.legs).toHaveLength(0);
    expect(map.length).toBe(0);
  });

  it("draws a station for every place and a line for every leg", () => {
    const map = routeMap([amsterdam, vienna, rome], options)!;
    expect(map.stations.map((station) => station.kind)).toEqual(["end", "via", "end"]);
    expect(map.legs).toHaveLength(2);
    expect(map.legs.map((leg) => leg.night)).toEqual([false, true]);
    expect(map.legs.map((leg) => leg.colour)).toEqual(["var(--leg-1)", "var(--color-night)"]);
    expect(map.length).toBeCloseTo(map.legs[0]!.length + map.legs[1]!.length, 6);
  });

  it("keeps every station in the box with the padding around it", () => {
    const map = routeMap([amsterdam, vienna, rome], options)!;
    for (const station of map.stations) {
      expect(station.x).toBeGreaterThanOrEqual(options.padding);
      expect(station.x).toBeLessThanOrEqual(options.width - options.padding);
      expect(station.y).toBeGreaterThanOrEqual(options.padding);
      expect(station.y).toBeLessThanOrEqual(options.height - options.padding);
    }
  });

  it("puts north up and east to the right", () => {
    const map = routeMap([amsterdam, vienna, rome], options)!;
    const [first, second, third] = map.stations;
    // Vienna is east and south of Amsterdam, and Rome is south of Vienna.
    expect(second!.x).toBeGreaterThan(first!.x);
    expect(second!.y).toBeGreaterThan(first!.y);
    expect(third!.y).toBeGreaterThan(second!.y);
  });

  it("joins the stations with lines that only run along an axis or at 45 degrees", () => {
    const map = routeMap([amsterdam, vienna, rome, stop(45.4642, 9.19)], options)!;
    for (const leg of map.legs) {
      expect(isOctilinear(leg.points)).toBe(true);
    }
    // Each line starts at the station it leaves and ends at the station it reaches.
    map.legs.forEach((leg, index) => {
      expect(leg.points[0]).toEqual({ x: map.stations[index]!.x, y: map.stations[index]!.y });
      expect(leg.points.at(-1)).toEqual({ x: map.stations[index + 1]!.x, y: map.stations[index + 1]!.y });
    });
  });

  it("starts the path of the whole route at the first station and goes through all legs", () => {
    const map = routeMap([amsterdam, vienna, rome], options)!;
    expect(map.path.startsWith(`M${map.stations[0]!.x} ${map.stations[0]!.y}`)).toBe(true);
    expect(map.path.match(/M/g)).toHaveLength(1);
  });

  it("keeps stations that follow each other apart, even for places close together", () => {
    const close = [stop(50.9432, 6.9587), stop(50.9433, 6.9588), stop(50.9434, 6.9589, true), stop(50.9435, 6.959)];
    const map = routeMap(close, options)!;
    map.stations.forEach((station, index) => {
      if (index === 0) {
        return;
      }
      const previous = map.stations[index - 1]!;
      const cells = Math.max(Math.abs(station.x - previous.x), Math.abs(station.y - previous.y)) / options.grid;
      expect(cells).toBeGreaterThanOrEqual(2);
    });
  });

  it("stretches a narrow route sideways so that it fills more of the box", () => {
    // Amsterdam to Zurich runs mostly north to south, and the box is wide.
    const map = routeMap([amsterdam, stop(47.3779, 8.5403)], options)!;
    const width = Math.abs(map.stations[1]!.x - map.stations[0]!.x);
    expect(width).toBeGreaterThan(20);
  });

  it("copes with places that share a position", () => {
    const map = routeMap([amsterdam, amsterdam], options)!;
    expect(map.stations).toHaveLength(2);
    expect(Number.isFinite(map.stations[0]!.x)).toBe(true);
  });
});
