// The rail graph of the viewer: the stations that follow each other on a route of the timetable
// bundle, and the search that finds the stations a train passes between two stops.
//
// The file `src/generated/rail-graph.bin` is written by the example `rail_graph` of `btt-planner`,
// which describes its layout. The search follows `crates/btt-planner/src/ride_path.rs`, which
// draws the lines of the desktop map, and the constants have the same names and values. A snapshot
// holds only the stations a ride starts and ends at, so the viewer finds the stations in between
// here and the links stay as short as they are.

import type { GeoCoordinates } from "@btt/shared/bindings";

/** Hops shorter than this keep the plain curve between their two stops, in meters. */
const shortestSearchedHopMeters = 8_000;

/**
 * Longest edge a path may use at first, in meters. Longer edges belong to fast trains that skip
 * the stations the path is after. A high-speed line has few stations along it, so when no path
 * with short edges exists, a second search allows edges of up to half the hop.
 */
const longestRegionalEdgeMeters = 25_000;

/**
 * Longest accepted path of a hop, in percent of its straight distance. Longer paths most likely
 * follow another line, such as an old line next to a high-speed line.
 */
const longestPathPercent = 115;

/**
 * Longest accepted chain of the hops of one category, in percent of the straight distance of a
 * ride. A chain is longer than a hop, because a ride goes around the places its train must stop
 * at, and around mountains. Trains of one category run on one network, so a corridor that is
 * far longer than the straight line is still the one the train takes, as the one from München to Venezia is.
 */
const longestCategoryChainPercent = 200;

/**
 * Longest accepted chain of the hops of any long-distance train, in percent of the straight
 * distance of a ride. The hops of any category make a denser network than the one of a train,
 * so the chain has to stay closer to the straight line to be the track of the train.
 */
const longestAnyChainPercent = 130;

/** Smallest distance between two points of a path, in meters. Station coordinates sit beside the track. */
const shortestPointSpacingMeters = 2_000;

/** Most stations one search settles before it gives up, which bounds the time of a search. */
const maximumSettledStations = 20_000;

/** Most stations the search for a chain of long-distance hops settles. */
const maximumSettledChainStations = 50_000;

/** Furthest a station of a ride may lie from its station of the graph, in meters. */
const nearestStationMeters = 300;

/** Mean radius of the Earth that the planner measures distances with, in meters. */
const earthRadiusMeters = 6_371_008.8;

/** Size of the steps that positions are written in, in microdegrees. */
const stepMicrodegrees = 100;

/** Size of the cells of the lookup of the nearest station, in degrees. */
const cellDegrees = 0.01;

/** An edge between two stations of the graph. */
export interface RailEdge {
  readonly first: number;
  readonly second: number;
  /** The positions in the list of categories of the trains that run on the edge. */
  readonly categories: readonly number[];
}

/** Says whether a search may use an edge, which is a position in the edges of the graph. */
type EdgeFilter = (edge: number) => boolean;

/** The stations of the bundle that routes stop at, and the pairs of consecutive stops. */
export class RailGraph {
  /** Categories of trains that serve a region, such as "S" and "RE". */
  readonly regionalCategories: ReadonlySet<string>;
  readonly latitudes: Float64Array;
  readonly longitudes: Float64Array;
  /** The edges that leave station `s` are the numbers from `rowOffsets[s]` to `rowOffsets[s + 1]`. */
  private readonly rowOffsets: Uint32Array;
  private readonly neighbours: Uint32Array;
  private readonly edgeMeters: Uint32Array;
  /** The categories of the trains on edge `e` are the numbers from `categoryRows[e]` to `categoryRows[e + 1]`. */
  private readonly categoryRows: Uint32Array;
  private readonly categoryPositions: Uint16Array;
  /** The names of the categories, which `categoryPositions` points into. */
  private readonly categoryNames: readonly string[];
  /** Whether each category is a regional one. */
  private readonly categoryIsRegional: readonly boolean[];
  /** The stations in each cell of a grid, for finding the nearest one. */
  private readonly cells = new Map<number, number[]>();

  constructor(
    coordinates: readonly GeoCoordinates[],
    edges: readonly RailEdge[],
    regionalCategories: Iterable<string> = [],
    categoryNames: readonly string[] = [],
  ) {
    this.regionalCategories = new Set(regionalCategories);
    this.categoryNames = categoryNames;
    this.categoryIsRegional = categoryNames.map((name) => this.regionalCategories.has(name));
    this.latitudes = Float64Array.from(coordinates, (point) => point.latitude);
    this.longitudes = Float64Array.from(coordinates, (point) => point.longitude);
    const count = coordinates.length;
    // Each edge is in the rows of both of its stations, which are sorted so a row is contiguous.
    const directed: { from: number; to: number; categories: readonly number[] }[] = [];
    for (const edge of edges) {
      if (edge.first === edge.second || edge.first >= count || edge.second >= count) {
        continue;
      }
      directed.push({ from: edge.first, to: edge.second, categories: edge.categories });
      directed.push({ from: edge.second, to: edge.first, categories: edge.categories });
    }
    directed.sort((a, b) => a.from - b.from || a.to - b.to);
    this.rowOffsets = new Uint32Array(count + 1);
    this.neighbours = new Uint32Array(directed.length);
    this.edgeMeters = new Uint32Array(directed.length);
    this.categoryRows = new Uint32Array(directed.length + 1);
    const positions: number[] = [];
    directed.forEach((edge, index) => {
      this.rowOffsets[edge.from + 1] = (this.rowOffsets[edge.from + 1] ?? 0) + 1;
      this.neighbours[index] = edge.to;
      this.edgeMeters[index] = Math.round(this.distance(edge.from, edge.to));
      positions.push(...edge.categories);
      this.categoryRows[index + 1] = positions.length;
    });
    this.categoryPositions = Uint16Array.from(positions);
    for (let station = 0; station < count; station += 1) {
      this.rowOffsets[station + 1] = (this.rowOffsets[station + 1] ?? 0) + (this.rowOffsets[station] ?? 0);
    }
    for (let station = 0; station < count; station += 1) {
      const key = cellKey(this.latitudes[station]!, this.longitudes[station]!);
      const cell = this.cells.get(key);
      if (cell === undefined) {
        this.cells.set(key, [station]);
      } else {
        cell.push(station);
      }
    }
  }

  get stationCount(): number {
    return this.latitudes.length;
  }

  /** Returns the coordinates of a station. */
  coordinates(station: number): GeoCoordinates {
    return { latitude: this.latitudes[station]!, longitude: this.longitudes[station]! };
  }

  /** Returns the straight distance between two stations in meters. */
  distance(from: number, to: number): number {
    return haversineMeters(
      this.latitudes[from]!,
      this.longitudes[from]!,
      this.latitudes[to]!,
      this.longitudes[to]!,
    );
  }

  /** Returns the straight distance from a station to a point in meters. */
  distanceTo(station: number, point: GeoCoordinates): number {
    return haversineMeters(this.latitudes[station]!, this.longitudes[station]!, point.latitude, point.longitude);
  }

  /** Returns the station nearest to a point within `maximumMeters`, or `null` when none is. */
  nearestStation(point: GeoCoordinates, maximumMeters: number = nearestStationMeters): number | null {
    const row = Math.floor(point.latitude / cellDegrees);
    const column = Math.floor(point.longitude / cellDegrees);
    let best: number | null = null;
    let bestMeters = maximumMeters;
    for (let rowOffset = -1; rowOffset <= 1; rowOffset += 1) {
      for (let columnOffset = -1; columnOffset <= 1; columnOffset += 1) {
        for (const station of this.cells.get(cellKeyOf(row + rowOffset, column + columnOffset)) ?? []) {
          const meters = this.distanceTo(station, point);
          if (meters <= bestMeters) {
            best = station;
            bestMeters = meters;
          }
        }
      }
    }
    return best;
  }

  /**
   * Finds the shortest path between two stations with A*, over edges of at most `longestEdge`
   * meters that the filter allows. Returns the stations of the path including both ends, or
   * `null` when every path is longer than `percent` of `hopMeters`.
   */
  shortestPath(
    from: number,
    to: number,
    hopMeters: number,
    longestEdge: number,
    percent: number,
    filter: EdgeFilter | null,
    maximumSettled: number,
  ): number[] | null {
    const longestPath = Math.floor((hopMeters * percent) / 100);
    const remaining = (station: number) => Math.round(this.distance(station, to));
    // Path length so far and the station it came from, by station.
    const reached = new Map<number, { length: number; previous: number }>();
    reached.set(from, { length: 0, previous: from });
    const queue = new MinimumHeap();
    queue.push(remaining(from), 0, from);
    let settled = 0;
    for (let entry = queue.pop(); entry !== null; entry = queue.pop()) {
      const { length, station } = entry;
      if (station === to) {
        const path = [to];
        let current = to;
        while (current !== from) {
          current = reached.get(current)!.previous;
          path.push(current);
        }
        return path.reverse();
      }
      const best = reached.get(station);
      if (best !== undefined && best.length < length) {
        continue;
      }
      settled += 1;
      if (settled > maximumSettled) {
        return null;
      }
      for (let index = this.rowOffsets[station]!; index < this.rowOffsets[station + 1]!; index += 1) {
        const edgeMeters = this.edgeMeters[index]!;
        if (edgeMeters > longestEdge || (filter !== null && !filter(index))) {
          continue;
        }
        const neighbour = this.neighbours[index]!;
        const neighbourLength = length + edgeMeters;
        // The straight distance to the end is a lower bound, so a station whose path plus that
        // bound is over the limit can't be on an accepted path.
        const estimate = neighbourLength + remaining(neighbour);
        const known = reached.get(neighbour);
        if (estimate > longestPath || (known !== undefined && known.length <= neighbourLength)) {
          continue;
        }
        reached.set(neighbour, { length: neighbourLength, previous: station });
        queue.push(estimate, neighbourLength, neighbour);
      }
    }
    return null;
  }

  /**
   * Returns the stations a train most likely passes between two consecutive stops, in travel
   * order. Returns nothing when the hop is short or no path over short edges stays close to the
   * straight line.
   */
  passedStations(from: number, to: number): number[] {
    const hopMeters = Math.round(this.distance(from, to));
    if (hopMeters < shortestSearchedHopMeters) {
      return [];
    }
    const halfHop = Math.floor(hopMeters / 2);
    let path = this.shortestPath(
      from,
      to,
      hopMeters,
      Math.min(halfHop, longestRegionalEdgeMeters),
      longestPathPercent,
      null,
      maximumSettledStations,
    );
    if (path === null && halfHop > longestRegionalEdgeMeters) {
      path = this.shortestPath(from, to, hopMeters, halfHop, longestPathPercent, null, maximumSettledStations);
    }
    return path === null ? [] : path.slice(1, -1);
  }

  /**
   * Returns the stations that a train of a category most likely stops at between two stations:
   * the shortest chain of the hops that trains of the category run, in travel order, including
   * both ends. A category that the graph does not know falls back to the hops of any train of
   * a long-distance category, and a regional category has no such fallback. Returns `null` when
   * there is no chain close to the straight line.
   */
  chainOf(from: number, to: number, category: string): number[] | null {
    const hopMeters = Math.round(this.distance(from, to));
    const position = this.categoryNames.indexOf(category);
    const searches: { filter: EdgeFilter; percent: number }[] = [];
    if (position >= 0) {
      searches.push({ filter: (edge) => this.edgeHasCategory(edge, position), percent: longestCategoryChainPercent });
    }
    if (!this.regionalCategories.has(category)) {
      searches.push({ filter: (edge) => this.edgeHasLongDistanceCategory(edge), percent: longestAnyChainPercent });
    }
    for (const { filter, percent } of searches) {
      const chain = this.shortestPath(
        from,
        to,
        hopMeters,
        Number.POSITIVE_INFINITY,
        percent,
        filter,
        maximumSettledChainStations,
      );
      if (chain !== null) {
        return chain;
      }
    }
    return null;
  }

  /** Returns whether trains of a category run on an edge. */
  private edgeHasCategory(edge: number, category: number): boolean {
    for (let index = this.categoryRows[edge]!; index < this.categoryRows[edge + 1]!; index += 1) {
      if (this.categoryPositions[index] === category) {
        return true;
      }
    }
    return false;
  }

  /** Returns whether a train of a category outside the regional ones runs on an edge. */
  private edgeHasLongDistanceCategory(edge: number): boolean {
    for (let index = this.categoryRows[edge]!; index < this.categoryRows[edge + 1]!; index += 1) {
      if (this.categoryIsRegional[this.categoryPositions[index]!] === false) {
        return true;
      }
    }
    return false;
  }
}

/** Returns the points of the line of a ride between two stops, as the stations it passes. */
export function ridePathPoints(
  graph: RailGraph,
  boarding: GeoCoordinates,
  alighting: GeoCoordinates,
  category: string,
): GeoCoordinates[] {
  const from = graph.nearestStation(boarding);
  const to = graph.nearestStation(alighting);
  if (from === null || to === null || from === to) {
    return [];
  }
  if (Math.round(graph.distance(from, to)) < shortestSearchedHopMeters) {
    return [];
  }
  // A train stops at some of the stations of a chain of the hops that trains of its category
  // run, and between two of them it passes the stations of the regional lines along the same
  // track. Without a chain the whole ride is one hop, which finds a line when stations of
  // regional lines lie along it.
  const chain = graph.chainOf(from, to, category);
  const stations: number[] = [];
  if (chain === null) {
    stations.push(...graph.passedStations(from, to));
  } else {
    for (let index = 0; index + 1 < chain.length; index += 1) {
      if (index > 0) {
        stations.push(chain[index]!);
      }
      stations.push(...graph.passedStations(chain[index]!, chain[index + 1]!));
    }
  }
  const points: GeoCoordinates[] = [];
  let previous = boarding;
  for (const station of stations) {
    const point = graph.coordinates(station);
    if (
      haversineMeters(previous.latitude, previous.longitude, point.latitude, point.longitude) >= shortestPointSpacingMeters &&
      haversineMeters(point.latitude, point.longitude, alighting.latitude, alighting.longitude) >= shortestPointSpacingMeters
    ) {
      points.push(point);
      previous = point;
    }
  }
  return points;
}

/** Returns the great-circle distance between two points in meters, by the haversine formula. */
export function haversineMeters(latitudeA: number, longitudeA: number, latitudeB: number, longitudeB: number): number {
  const toRadians = Math.PI / 180;
  const deltaLatitude = (latitudeB - latitudeA) * toRadians;
  const deltaLongitude = (longitudeB - longitudeA) * toRadians;
  const halfChord =
    Math.sin(deltaLatitude / 2) ** 2 +
    Math.cos(latitudeA * toRadians) * Math.cos(latitudeB * toRadians) * Math.sin(deltaLongitude / 2) ** 2;
  return 2 * earthRadiusMeters * Math.asin(Math.sqrt(halfChord));
}

function cellKey(latitude: number, longitude: number): number {
  return cellKeyOf(Math.floor(latitude / cellDegrees), Math.floor(longitude / cellDegrees));
}

function cellKeyOf(row: number, column: number): number {
  // Rows run from -9,000 to 9,000 and columns from -18,000 to 18,000.
  return (row + 10_000) * 40_000 + (column + 20_000);
}

/** A queue of stations that gives the one with the lowest estimate first. */
class MinimumHeap {
  private readonly estimates: number[] = [];
  private readonly lengths: number[] = [];
  private readonly stations: number[] = [];

  push(estimate: number, length: number, station: number): void {
    let index = this.estimates.length;
    this.estimates.push(estimate);
    this.lengths.push(length);
    this.stations.push(station);
    while (index > 0) {
      const parent = (index - 1) >> 1;
      if (!this.before(index, parent)) {
        break;
      }
      this.swap(index, parent);
      index = parent;
    }
  }

  pop(): { estimate: number; length: number; station: number } | null {
    if (this.estimates.length === 0) {
      return null;
    }
    const top = { estimate: this.estimates[0]!, length: this.lengths[0]!, station: this.stations[0]! };
    const last = this.estimates.length - 1;
    this.swap(0, last);
    this.estimates.pop();
    this.lengths.pop();
    this.stations.pop();
    let index = 0;
    for (;;) {
      const left = 2 * index + 1;
      const right = left + 1;
      let smallest = index;
      if (left < this.estimates.length && this.before(left, smallest)) {
        smallest = left;
      }
      if (right < this.estimates.length && this.before(right, smallest)) {
        smallest = right;
      }
      if (smallest === index) {
        return top;
      }
      this.swap(index, smallest);
      index = smallest;
    }
  }

  /** Returns whether the entry at `a` comes before the entry at `b`. */
  private before(a: number, b: number): boolean {
    return (
      this.estimates[a]! < this.estimates[b]! ||
      (this.estimates[a] === this.estimates[b] &&
        (this.lengths[a]! < this.lengths[b]! || (this.lengths[a] === this.lengths[b] && this.stations[a]! < this.stations[b]!)))
    );
  }

  private swap(a: number, b: number): void {
    [this.estimates[a], this.estimates[b]] = [this.estimates[b]!, this.estimates[a]!];
    [this.lengths[a], this.lengths[b]] = [this.lengths[b]!, this.lengths[a]!];
    [this.stations[a], this.stations[b]] = [this.stations[b]!, this.stations[a]!];
  }
}

/** Reads the numbers of the file: groups of 7 bits, the low group first. */
class NumberReader {
  private position = 0;

  constructor(private readonly bytes: Uint8Array) {}

  byte(): number {
    const value = this.bytes[this.position];
    if (value === undefined) {
      throw new Error("the rail graph ends too early");
    }
    this.position += 1;
    return value;
  }

  number(): number {
    let value = 0;
    for (let group = 0; group < 7; group += 1) {
      const byte = this.byte();
      value += (byte & 0x7f) * 2 ** (7 * group);
      if ((byte & 0x80) === 0) {
        return value;
      }
    }
    throw new Error("a number of the rail graph is too large");
  }

  signedNumber(): number {
    const zigzag = this.number();
    const half = Math.floor(zigzag / 2);
    return zigzag % 2 === 1 ? -half - 1 : half;
  }

  text(): string {
    const length = this.number();
    const start = this.position;
    this.position += length;
    if (this.position > this.bytes.length) {
      throw new Error("the rail graph ends too early");
    }
    return new TextDecoder().decode(this.bytes.subarray(start, this.position));
  }
}

/** Reads the file that the example `rail_graph` writes. Throws an error when it is damaged. */
export function parseRailGraph(bytes: Uint8Array): RailGraph {
  const reader = new NumberReader(bytes);
  if (reader.byte() !== 2) {
    throw new Error("the rail graph has a version this viewer does not know");
  }
  const regionalCategories: string[] = [];
  for (let remaining = reader.number(); remaining > 0; remaining -= 1) {
    regionalCategories.push(reader.text());
  }
  const categoryNames: string[] = [];
  for (let remaining = reader.number(); remaining > 0; remaining -= 1) {
    categoryNames.push(reader.text());
  }
  const stationCount = reader.number();
  const coordinates: GeoCoordinates[] = [];
  let latitudeSteps = 0;
  let longitudeSteps = 0;
  for (let station = 0; station < stationCount; station += 1) {
    latitudeSteps += reader.signedNumber();
    longitudeSteps += reader.signedNumber();
    coordinates.push({
      latitude: (latitudeSteps * stepMicrodegrees) / 1_000_000,
      longitude: (longitudeSteps * stepMicrodegrees) / 1_000_000,
    });
  }
  const edges: RailEdge[] = [];
  let first = 0;
  for (let remaining = reader.number(); remaining > 0; remaining -= 1) {
    first += reader.number();
    const second = first + reader.number();
    const categories: number[] = [];
    for (let remainingCategories = reader.number(); remainingCategories > 0; remainingCategories -= 1) {
      categories.push(reader.number());
    }
    edges.push({ first, second, categories });
  }
  return new RailGraph(coordinates, edges, regionalCategories, categoryNames);
}

let loaded: Promise<RailGraph> | null = null;

/** Returns the graph, which is fetched on first use as a file of its own. */
export function loadRailGraph(): Promise<RailGraph> {
  loaded ??= import("../generated/rail-graph.bin?url").then(async (module) => {
    const response = await fetch(module.default);
    if (!response.ok) {
      throw new Error("the rail graph could not be loaded");
    }
    return parseRailGraph(new Uint8Array(await response.arrayBuffer()));
  });
  return loaded;
}
