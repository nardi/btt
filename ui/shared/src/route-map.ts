// The drawing of the route of a trip as a small metro map: every leg is a line of straight
// pieces and 45 degree pieces with rounded bends, and every place is a station on it.

import { legLineOf } from "./itinerary";

/** A place of the route, as the trip list receives it. */
export interface RouteStop {
  /** Where the place is. */
  coordinates: { latitude: number; longitude: number };
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

/** The fewest grid cells between two stations that follow each other. */
const minimumCells = 2;

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
 * Draws the route of a trip as a metro map that fits a box. The places keep their relative
 * positions, snap to a grid, and are joined by lines that only run along an axis or at 45
 * degrees. Returns `null` for a route with no places.
 */
export function routeMap(stops: readonly RouteStop[], options: RouteMapOptions): RouteMap | null {
  if (stops.length === 0) {
    return null;
  }
  const { width, height, padding, grid, bendRadius } = options;
  const meanLatitude = stops.reduce((sum, stop) => sum + stop.coordinates.latitude, 0) / stops.length;
  const widthFactor = Math.cos((meanLatitude * Math.PI) / 180);
  // North is up, and a degree of longitude is shorter than a degree of latitude away from the equator.
  const projected = stops.map((stop) => ({ x: stop.coordinates.longitude * widthFactor, y: -stop.coordinates.latitude }));
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
  const offsetX = padding + (availableWidth - spanX * scaleX) / 2;
  const offsetY = padding + (availableHeight - spanY * scaleY) / 2;

  // The stations sit on the lines of a grid. The last line on each axis is the last one that
  // still leaves the padding to the edge.
  const lastColumn = Math.max(0, Math.floor(availableWidth / grid));
  const lastRow = Math.max(0, Math.floor(availableHeight / grid));
  const cells = projected.map((point) => ({
    column: Math.min(lastColumn, Math.max(0, Math.round((offsetX + (point.x - minimumX) * scaleX - padding) / grid))),
    row: Math.min(lastRow, Math.max(0, Math.round((offsetY + (point.y - minimumY) * scaleY - padding) / grid))),
  }));
  // Stations that follow each other keep at least two cells apart, so no station hides another.
  // A station that is too close moves along the axis it is farthest apart on, away from the one
  // before it, or toward the middle of the box where there is no room on that side.
  for (let index = 1; index < cells.length; index++) {
    const previous = cells[index - 1]!;
    const cell = cells[index]!;
    if (Math.max(Math.abs(cell.column - previous.column), Math.abs(cell.row - previous.row)) >= minimumCells) {
      continue;
    }
    const horizontal =
      Math.abs(projected[index]!.x - projected[index - 1]!.x) >= Math.abs(projected[index]!.y - projected[index - 1]!.y);
    const direction = Math.sign(horizontal ? projected[index]!.x - projected[index - 1]!.x : projected[index]!.y - projected[index - 1]!.y) || 1;
    const last = horizontal ? lastColumn : lastRow;
    const from = horizontal ? previous.column : previous.row;
    const target = from + direction * minimumCells;
    const moved = target < 0 || target > last ? from - direction * minimumCells : target;
    if (horizontal) {
      cell.column = Math.min(last, Math.max(0, moved));
    } else {
      cell.row = Math.min(last, Math.max(0, moved));
    }
  }
  const points: MapPoint[] = cells.map((cell) => ({
    x: padding + cell.column * grid,
    y: padding + cell.row * grid,
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
