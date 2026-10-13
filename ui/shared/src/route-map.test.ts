import { describe, expect, it } from "vitest";

import { evenRanks, octilinearPoints, samePlaceIndices, type RouteMapOptions, type RouteStop, routeMap } from "./route-map";

const options: RouteMapOptions = { width: 120, height: 64, padding: 10, grid: 8, bendRadius: 6 };

function stop(latitude: number, longitude: number, night = false, radius = 5000): RouteStop {
  return { coordinates: { latitude, longitude }, station_search_radius: radius, arrives_by_night: night };
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

  /** Returns the fewest grid cells between any two stations of a map. */
  function closestCells(stations: readonly { x: number; y: number }[], grid = options.grid): number {
    let closest = Infinity;
    stations.forEach((station, index) => {
      for (const other of stations.slice(index + 1)) {
        closest = Math.min(closest, Math.max(Math.abs(station.x - other.x), Math.abs(station.y - other.y)) / grid);
      }
    });
    return closest;
  }

  it("keeps every station apart from every other, even for places close together", () => {
    // Four stations in Cologne, a few hundred meters apart, each with a radius too small to reach the others.
    const close = [stop(50.9432, 6.9587, false, 100), stop(50.9452, 6.9617, false, 100), stop(50.9472, 6.9647, true, 100), stop(50.9492, 6.9677, false, 100)];
    expect(closestCells(routeMap(close, options)!.stations)).toBeGreaterThanOrEqual(3);
  });

  it("spreads out a cluster of places on a long route and keeps their order", () => {
    // Munich, Salzburg and Innsbruck lie close together between Stockholm and Palermo.
    const route = [stop(59.3293, 18.0686), stop(48.1351, 11.582), stop(47.8095, 13.055), stop(47.2692, 11.4041), stop(38.1157, 13.3615)];
    // The box of the trip list, which has room for five places from north to south.
    const map = routeMap(route, { width: 160, height: 76, padding: 14, grid: 6, bendRadius: 7 })!;
    expect(closestCells(map.stations, 6)).toBeGreaterThanOrEqual(3);
    const [stockholm, munich, salzburg, innsbruck, palermo] = map.stations;
    expect(munich!.y).toBeGreaterThan(stockholm!.y);
    expect(salzburg!.x).toBeGreaterThan(munich!.x);
    expect(innsbruck!.y).toBeGreaterThanOrEqual(salzburg!.y);
    expect(palermo!.y).toBeGreaterThan(innsbruck!.y);
  });

  it("ranks equal values the same and spreads the others evenly", () => {
    expect(evenRanks([5, 1, 5, 3])).toEqual([1, 0, 1, 0.5]);
    expect(evenRanks([2, 2])).toEqual([0, 0]);
  });

  it("stretches a narrow route sideways so that it fills more of the box", () => {
    // Amsterdam to Zurich runs mostly north to south, and the box is wide.
    const map = routeMap([amsterdam, stop(47.3779, 8.5403)], options)!;
    const width = Math.abs(map.stations[1]!.x - map.stations[0]!.x);
    expect(width).toBeGreaterThan(20);
  });

  it("draws a place that the route visits twice at one position", () => {
    // A round trip from Amsterdam, with its end a few streets from where it started.
    const map = routeMap([amsterdam, vienna, rome, stop(52.37, 4.89)], options)!;
    expect(map.stations).toHaveLength(4);
    expect({ x: map.stations[3]!.x, y: map.stations[3]!.y }).toEqual({ x: map.stations[0]!.x, y: map.stations[0]!.y });
    expect(Number.isFinite(map.length)).toBe(true);
  });

  it("copes with places that share a position", () => {
    const map = routeMap([amsterdam, amsterdam], options)!;
    expect(map.stations).toHaveLength(2);
    expect(Number.isFinite(map.stations[0]!.x)).toBe(true);
  });
});

describe("samePlaceIndices", () => {
  it("numbers the places in the order the route first reaches them", () => {
    expect(samePlaceIndices([amsterdam, vienna, stop(52.37, 4.89), rome])).toEqual([0, 1, 0, 2]);
  });

  it("needs each place within the radius of the other", () => {
    // About 3 km apart, inside the radius of the first but outside the radius of the second.
    const wide = stop(52.0, 5.0, false, 5000);
    const narrow = stop(52.027, 5.0, false, 1000);
    expect(samePlaceIndices([wide, narrow])).toEqual([0, 1]);
    expect(samePlaceIndices([wide, stop(52.027, 5.0)])).toEqual([0, 0]);
  });
});
