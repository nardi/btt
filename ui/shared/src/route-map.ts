// The drawing of the route of a trip as a small metro map: every leg is a line of straight
// pieces and 45 degree pieces with rounded bends, and every place is a station on it.

import { haversineMeters } from "./geo";
import { legLineOf } from "./itinerary";

/** A place of the route, as the trip list receives it. */
export interface RouteStop {
  /** Where the place is. */
  coordinates: { latitude: number; longitude: number };
  /** How far around the place the planner looks for stations, in meters. */
  station_search_radius: number;
  /** Whether the leg that arrives here includes a night train. False for the origin. */
  arrives_by_night: boolean;
}

/** A point of the drawing, in pixels. */
export interface MapPoint {
  x: number;
  y: number;
}

/** A place on the map. */
export interface MapStation extends MapPoint {
  /** A circle for the ends of the route and a square for the places between them. */
  kind: "end" | "via";
}

/** One leg of the route, drawn as a line. */
export interface MapLeg {
  /** The corners of the line, from the station it leaves to the station it reaches. */
  points: MapPoint[];
  /** The path of the line, with its bends rounded. */
  path: string;
  /** The length of the line in pixels. */
  length: number;
  /** A CSS color value from the palette of the legs. */
  colour: string;
  /** Whether the leg is a night leg, which draws dashed. */
  night: boolean;
}

/** The drawing of a route. */
export interface RouteMap {
  width: number;
  height: number;
  stations: MapStation[];
  legs: MapLeg[];
  /** The path of the whole route in one piece, which a train can follow. */
  path: string;
  /** The length of the whole route in pixels. */
  length: number;
}

/** The size and the shape of the drawing. */
export interface RouteMapOptions {
  width: number;
  height: number;
  /** The space between the stations and the edge. */
  padding: number;
  /** The grid the stations snap to, which makes the lines meet at clean angles. */
  grid: number;
  /** The largest radius of a bend. */
  bendRadius: number;
}

/** The most an axis stretches beyond the scale that keeps the shape of the route. */
const maximumStretch = 3;

/** How much of a station's place on the map comes from its rank along each axis, against its true position. */
const rankShare = 0.5;

/** The fewest grid cells between any two stations, where the box has room for it. */
const preferredCells = 3;

/** A grid cell of the drawing, counted from the padding. */
interface GridCell {
  column: number;
  row: number;
}

/** The grid that the stations snap to. */
interface StationGrid {
  padding: number;
  grid: number;
  lastColumn: number;
  lastRow: number;
}

/** Mixes a true position on an axis with a rank on it, both between 0 and 1. */
function mix(position: number, rank: number): number {
  return (1 - rankShare) * position + rankShare * rank;
}

/**
 * Returns the rank of each value among the distinct values, spread evenly between 0 and 1.
 * Equal values get the same rank, and a single distinct value gets 0.
 */
export function evenRanks(values: readonly number[]): number[] {
  const distinct = [...new Set(values)].sort((left, right) => left - right);
  const last = Math.max(1, distinct.length - 1);
  return values.map((value) => distinct.indexOf(value) / last);
}

/**
 * Returns the grid cell nearest to a point that keeps `preferredCells` from every station placed
 * so far. Where the box has no such cell, the spacing shrinks one cell at a time, and with no
 * room at all the nearest cell wins.
 */
function freeCell(point: MapPoint, placed: readonly GridCell[], stationGrid: StationGrid): GridCell {
  const { padding, grid, lastColumn, lastRow } = stationGrid;
  for (let spacing = preferredCells; spacing >= 0; spacing--) {
    let best: GridCell | null = null;
    let bestDistance = Infinity;
    for (let column = 0; column <= lastColumn; column++) {
      for (let row = 0; row <= lastRow; row++) {
        const isFree = placed.every((other) => Math.max(Math.abs(other.column - column), Math.abs(other.row - row)) >= spacing);
        if (!isFree) {
          continue;
        }
        const distance = Math.hypot(padding + column * grid - point.x, padding + row * grid - point.y);
        if (distance < bestDistance) {
          best = { column, row };
          bestDistance = distance;
        }
      }
    }
    if (best !== null) {
      return best;
    }
  }
  return { column: 0, row: 0 };
}

/** Returns the length of a line through points. */
function polylineLength(points: readonly MapPoint[]): number {
  let length = 0;
  for (let index = 1; index < points.length; index++) {
    length += Math.hypot(points[index]!.x - points[index - 1]!.x, points[index]!.y - points[index - 1]!.y);
  }
  return length;
}

/**
 * Returns the corners of a line between two stations that only runs straight along an axis or
 * at 45 degrees: a straight piece, a diagonal and a straight piece, with the straight part split
 * evenly around the diagonal.
 */
export function octilinearPoints(from: MapPoint, to: MapPoint): MapPoint[] {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const absoluteX = Math.abs(dx);
  const absoluteY = Math.abs(dy);
  if (absoluteX === 0 || absoluteY === 0 || absoluteX === absoluteY) {
    return [from, to];
  }
  const diagonal = Math.min(absoluteX, absoluteY);
  const half = (Math.max(absoluteX, absoluteY) - diagonal) / 2;
  const stepX = Math.sign(dx);
  const stepY = Math.sign(dy);
  if (absoluteX > absoluteY) {
    return [
      from,
      { x: from.x + stepX * half, y: from.y },
      { x: from.x + stepX * (half + diagonal), y: from.y + stepY * diagonal },
      to,
    ];
  }
  return [
    from,
    { x: from.x, y: from.y + stepY * half },
    { x: from.x + stepX * diagonal, y: from.y + stepY * (half + diagonal) },
    to,
  ];
}

/** Rounds a number for a path, which keeps the text short. */
function round(value: number): number {
  return Math.round(value * 100) / 100;
}

/**
 * Returns the commands of a path through points with rounded bends, from the first point on.
 * A bend is rounded with a radius of at most `radius`, and less where the pieces next to it are
 * short.
 */
function roundedCommands(points: readonly MapPoint[], radius: number): string {
  const commands: string[] = [];
  for (let index = 1; index < points.length; index++) {
    const point = points[index]!;
    const next = points[index + 1];
    if (next === undefined) {
      commands.push(`L${round(point.x)} ${round(point.y)}`);
      continue;
    }
    const previous = points[index - 1]!;
    const before = Math.hypot(point.x - previous.x, point.y - previous.y);
    const after = Math.hypot(next.x - point.x, next.y - point.y);
    const reach = Math.min(radius, before / 2, after / 2);
    const enter = { x: point.x + ((previous.x - point.x) / before) * reach, y: point.y + ((previous.y - point.y) / before) * reach };
    const leave = { x: point.x + ((next.x - point.x) / after) * reach, y: point.y + ((next.y - point.y) / after) * reach };
    commands.push(
      `L${round(enter.x)} ${round(enter.y)}`,
      `Q${round(point.x)} ${round(point.y)} ${round(leave.x)} ${round(leave.y)}`,
    );
  }
  return commands.join("");
}

/**
 * Returns for each stop the index of the place it stands for on the map, with the places numbered
 * in the order the route first reaches them. Two stops are the same place when each lies within
 * the other's station search radius, and stops linked by a chain of such pairs are one place too.
 */
export function samePlaceIndices(stops: readonly RouteStop[]): number[] {
  const parent = stops.map((_, index) => index);
  const root = (index: number): number => {
    let current = index;
    while (parent[current] !== current) {
      current = parent[current]!;
    }
    return current;
  };
  stops.forEach((stop, index) => {
    for (let other = index + 1; other < stops.length; other++) {
      const distance = haversineMeters(stop.coordinates, stops[other]!.coordinates);
      if (distance <= Math.min(stop.station_search_radius, stops[other]!.station_search_radius)) {
        // The earlier stop stays the root, so a place takes its number from its first visit.
        const [first, second] = [root(index), root(other)].sort((left, right) => left - right);
        parent[second!] = first!;
      }
    }
  });
  const numbers = new Map<number, number>();
  return stops.map((_, index) => {
    const placeRoot = root(index);
    if (!numbers.has(placeRoot)) {
      numbers.set(placeRoot, numbers.size);
    }
    return numbers.get(placeRoot)!;
  });
}

/**
 * Draws the route of a trip as a schematic metro map that fits a box. The places mostly keep their order
 * from west to east and from north to south, but not their distances. Places close together
 * spread out, and every station keeps a few grid cells from every other. The lines between them
 * only run along an axis or at 45 degrees. Returns `null` for a route with no places.
 */
export function routeMap(stops: readonly RouteStop[], options: RouteMapOptions): RouteMap | null {
  if (stops.length === 0) {
    return null;
  }
  const { width, height, padding, grid, bendRadius } = options;
  // The layout places each place once, and the stops that stand for the same place share it.
  const placeOfStop = samePlaceIndices(stops);
  const places = stops.filter((_, index) => placeOfStop.indexOf(placeOfStop[index]!) === index);
  const meanLatitude = places.reduce((sum, stop) => sum + stop.coordinates.latitude, 0) / places.length;
  const widthFactor = Math.cos((meanLatitude * Math.PI) / 180);
  // North is up, and a degree of longitude is shorter than a degree of latitude away from the equator.
  const projected = places.map((stop) => ({ x: stop.coordinates.longitude * widthFactor, y: -stop.coordinates.latitude }));
  const minimumX = Math.min(...projected.map((point) => point.x));
  const maximumX = Math.max(...projected.map((point) => point.x));
  const minimumY = Math.min(...projected.map((point) => point.y));
  const maximumY = Math.max(...projected.map((point) => point.y));
  const spanX = maximumX - minimumX;
  const spanY = maximumY - minimumY;
  const availableWidth = width - 2 * padding;
  const availableHeight = height - 2 * padding;
  // A metro map is schematic, so each axis may stretch to fill the box, but at most to three
  // times the scale that keeps the shape, which keeps a route recognisable.
  const fitX = spanX === 0 ? Infinity : availableWidth / spanX;
  const fitY = spanY === 0 ? Infinity : availableHeight / spanY;
  const shapeScale = Math.min(fitX, fitY);
  const scaleX = Number.isFinite(shapeScale) ? Math.min(fitX, shapeScale * maximumStretch) : 0;
  const scaleY = Number.isFinite(shapeScale) ? Math.min(fitY, shapeScale * maximumStretch) : 0;
  const extentX = spanX * scaleX;
  const extentY = spanY * scaleY;
  const offsetX = padding + (availableWidth - extentX) / 2;
  const offsetY = padding + (availableHeight - extentY) / 2;

  // Each axis mixes the true position with the rank of the place along that axis. The ranks are
  // evenly spaced, so places that cluster together spread out, while the order along each axis
  // and so the rough shape of the route stay the same.
  const rankX = evenRanks(projected.map((point) => point.x));
  const rankY = evenRanks(projected.map((point) => point.y));
  const ideal = projected.map((point, index) => ({
    x: offsetX + extentX * mix(spanX === 0 ? 0 : (point.x - minimumX) / spanX, rankX[index]!),
    y: offsetY + extentY * mix(spanY === 0 ? 0 : (point.y - minimumY) / spanY, rankY[index]!),
  }));

  // The stations sit on the lines of a grid. The last line on each axis is the last one that
  // still leaves the padding to the edge.
  const lastColumn = Math.max(0, Math.floor(availableWidth / grid));
  const lastRow = Math.max(0, Math.floor(availableHeight / grid));
  const cells: GridCell[] = [];
  for (const point of ideal) {
    cells.push(freeCell(point, cells, { padding, grid, lastColumn, lastRow }));
  }
  const points: MapPoint[] = placeOfStop.map((place) => ({
    x: padding + cells[place]!.column * grid,
    y: padding + cells[place]!.row * grid,
  }));

  const stations: MapStation[] = points.map((point, index) => ({
    ...point,
    kind: index === 0 || index === points.length - 1 ? "end" : "via",
  }));

  const legs: MapLeg[] = [];
  for (let index = 1; index < points.length; index++) {
    const corners = octilinearPoints(points[index - 1]!, points[index]!);
    const night = stops[index]!.arrives_by_night;
    legs.push({
      points: corners,
      path: `M${round(corners[0]!.x)} ${round(corners[0]!.y)}${roundedCommands(corners, bendRadius)}`,
      length: polylineLength(corners),
      colour: legLineOf(night, index).colour,
      night,
    });
  }

  const start = points[0]!;
  const path =
    legs.length === 0
      ? `M${round(start.x)} ${round(start.y)}`
      : legs
          .map((leg, index) => (index === 0 ? leg.path : `${roundedCommands(leg.points, bendRadius)}`))
          .join("");
  return {
    width,
    height,
    stations,
    legs,
    path,
    length: legs.reduce((sum, leg) => sum + leg.length, 0),
  };
}
