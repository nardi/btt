// Smooth train lines through the stations a train passes.
//
// The line is a centripetal Catmull-Rom spline, written as one cubic Bézier curve between each
// pair of stations. It passes exactly through every station, and the centripetal knots keep it
// from overshooting into loops where stations are unevenly spaced.

import type { GeoCoordinates } from "./bindings";

/** Smallest turning radius a piece may have before its tangents get shortened, in meters. */
export const minimumTurnRadiusMeters = 1000;

/** A turn sharper than this, in degrees, counts as the train reversing at a station. */
const reversalAngleDegrees = 120;

/** Spacing of the points a piece is sampled into, in meters. */
const sampleSpacingMeters = 1000;

/** Bounds on the number of points per piece, so short pieces still bend and long ones stay cheap. */
const minimumSamplesPerPiece = 4;
const maximumSamplesPerPiece = 48;

/** How many times a piece that turns too sharply gets its tangents halved before it goes straight. */
const maximumTangentShortenings = 6;

/** Meters per degree of latitude, and of longitude at the equator. */
const metersPerDegreeLatitude = 110_574;
const metersPerDegreeLongitude = 111_320;

/** A point on a local flat map, in meters east and north of the path's first station. */
interface PlanePoint {
  x: number;
  y: number;
}

/** One cubic Bézier piece between two stations. */
interface BezierPiece {
  start: PlanePoint;
  startHandle: PlanePoint;
  endHandle: PlanePoint;
  end: PlanePoint;
}

/**
 * Returns the points of a smooth line through the given stations, as `[longitude, latitude]`
 * pairs for GeoJSON. Every station is part of the result.
 */
export function curvedRidePath(stations: GeoCoordinates[]): [number, number][] {
  const first = stations[0];
  if (first === undefined) {
    return [];
  }
  const longitudeScale = metersPerDegreeLongitude * Math.cos((first.latitude * Math.PI) / 180);
  const toPlane = (point: GeoCoordinates): PlanePoint => ({
    x: (point.longitude - first.longitude) * longitudeScale,
    y: (point.latitude - first.latitude) * metersPerDegreeLatitude,
  });
  const toGeoJson = (point: PlanePoint): [number, number] => [
    first.longitude + point.x / longitudeScale,
    first.latitude + point.y / metersPerDegreeLatitude,
  ];

  const points = withoutRepeats(stations.map(toPlane));
  const result: PlanePoint[] = [points[0]!];
  for (const section of splitAtReversals(points)) {
    for (const piece of bezierPieces(section)) {
      result.push(...samplePiece(limitCurvature(piece)).slice(1));
    }
  }
  return result.map(toGeoJson);
}

/** Drops stations at the same point as the one before, which would make zero-length pieces. */
function withoutRepeats(points: PlanePoint[]): PlanePoint[] {
  return points.filter((point, index) => {
    const previous = points[index - 1];
    return previous === undefined || distance(previous, point) > 1;
  });
}

/**
 * Splits the stations into sections at every station where the train reverses. Each section
 * gets its own curve, so the line keeps a sharp corner there instead of swinging out wide.
 */
function splitAtReversals(points: PlanePoint[]): PlanePoint[][] {
  const sections: PlanePoint[][] = [];
  let section: PlanePoint[] = [points[0]!];
  for (let index = 1; index < points.length; index += 1) {
    const point = points[index]!;
    section.push(point);
    const previous = points[index - 1]!;
    const next = points[index + 1];
    if (next !== undefined && turnAngleDegrees(previous, point, next) > reversalAngleDegrees) {
      sections.push(section);
      section = [point];
    }
  }
  sections.push(section);
  return sections.filter((candidate) => candidate.length >= 2);
}

/** Returns the Bézier pieces of the centripetal Catmull-Rom spline through a section. */
function bezierPieces(points: PlanePoint[]): BezierPiece[] {
  const pieces: BezierPiece[] = [];
  for (let index = 0; index + 1 < points.length; index += 1) {
    const current = points[index]!;
    const next = points[index + 1]!;
    // The ends of a section get a mirrored neighbour, so the curve leaves them straight.
    const before = points[index - 1] ?? mirror(next, current);
    const after = points[index + 2] ?? mirror(current, next);
    pieces.push(catmullRomPiece(before, current, next, after));
  }
  return pieces;
}

/**
 * Converts the centripetal Catmull-Rom segment from `p1` to `p2` into a cubic Bézier curve.
 * The knot spacing is the square root of each chord length.
 */
function catmullRomPiece(p0: PlanePoint, p1: PlanePoint, p2: PlanePoint, p3: PlanePoint): BezierPiece {
  const d1 = Math.sqrt(distance(p0, p1));
  const d2 = Math.sqrt(distance(p1, p2));
  const d3 = Math.sqrt(distance(p2, p3));
  const startHandle = {
    x: (d1 * d1 * p2.x - d2 * d2 * p0.x + (2 * d1 * d1 + 3 * d1 * d2 + d2 * d2) * p1.x) / (3 * d1 * (d1 + d2)),
    y: (d1 * d1 * p2.y - d2 * d2 * p0.y + (2 * d1 * d1 + 3 * d1 * d2 + d2 * d2) * p1.y) / (3 * d1 * (d1 + d2)),
  };
  const endHandle = {
    x: (d3 * d3 * p1.x - d2 * d2 * p3.x + (2 * d3 * d3 + 3 * d3 * d2 + d2 * d2) * p2.x) / (3 * d3 * (d3 + d2)),
    y: (d3 * d3 * p1.y - d2 * d2 * p3.y + (2 * d3 * d3 + 3 * d3 * d2 + d2 * d2) * p2.y) / (3 * d3 * (d3 + d2)),
  };
  return { start: p1, startHandle, endHandle, end: p2 };
}

/**
 * Shortens the tangent handles of a piece until its tightest turn is no sharper than the
 * minimum radius. The piece still runs between the same two stations. After a few halvings it
 * becomes straight, which a turn at a station can still need.
 */
function limitCurvature(piece: BezierPiece): BezierPiece {
  let current = piece;
  for (let attempt = 0; attempt < maximumTangentShortenings; attempt += 1) {
    if (maximumCurvature(current) <= 1 / minimumTurnRadiusMeters) {
      return current;
    }
    current = {
      ...current,
      startHandle: midpoint(current.start, current.startHandle),
      endHandle: midpoint(current.end, current.endHandle),
    };
  }
  return { ...current, startHandle: current.start, endHandle: current.end };
}

/** Returns the largest curvature of a piece at evenly spaced sample points, in 1 per meter. */
function maximumCurvature(piece: BezierPiece): number {
  let largest = 0;
  for (let step = 0; step <= 16; step += 1) {
    largest = Math.max(largest, curvatureAt(piece, step / 16));
  }
  return largest;
}

/** Returns the curvature of a piece at parameter `t`, from its first and second derivatives. */
function curvatureAt(piece: BezierPiece, t: number): number {
  const { start: a, startHandle: b, endHandle: c, end: d } = piece;
  const u = 1 - t;
  const first = {
    x: 3 * u * u * (b.x - a.x) + 6 * u * t * (c.x - b.x) + 3 * t * t * (d.x - c.x),
    y: 3 * u * u * (b.y - a.y) + 6 * u * t * (c.y - b.y) + 3 * t * t * (d.y - c.y),
  };
  const second = {
    x: 6 * u * (c.x - 2 * b.x + a.x) + 6 * t * (d.x - 2 * c.x + b.x),
    y: 6 * u * (c.y - 2 * b.y + a.y) + 6 * t * (d.y - 2 * c.y + b.y),
  };
  const speed = Math.hypot(first.x, first.y);
  if (speed < 1e-9) {
    return 0;
  }
  return Math.abs(first.x * second.y - first.y * second.x) / speed ** 3;
}

/** Samples a piece into points, including both of its ends, spaced by its chord length. */
function samplePiece(piece: BezierPiece): PlanePoint[] {
  const chord = distance(piece.start, piece.end);
  const count = Math.min(
    maximumSamplesPerPiece,
    Math.max(minimumSamplesPerPiece, Math.ceil(chord / sampleSpacingMeters)),
  );
  const points: PlanePoint[] = [];
  for (let step = 0; step <= count; step += 1) {
    points.push(pointAt(piece, step / count));
  }
  // The last sample is exactly the end station, not a rounded approximation.
  points[points.length - 1] = piece.end;
  return points;
}

/** Returns the point of a piece at parameter `t`. */
function pointAt(piece: BezierPiece, t: number): PlanePoint {
  const { start: a, startHandle: b, endHandle: c, end: d } = piece;
  const u = 1 - t;
  return {
    x: u * u * u * a.x + 3 * u * u * t * b.x + 3 * u * t * t * c.x + t * t * t * d.x,
    y: u * u * u * a.y + 3 * u * u * t * b.y + 3 * u * t * t * c.y + t * t * t * d.y,
  };
}

/** Returns the angle between the directions into and out of `middle`, in degrees. */
function turnAngleDegrees(before: PlanePoint, middle: PlanePoint, after: PlanePoint): number {
  const incoming = Math.atan2(middle.y - before.y, middle.x - before.x);
  const outgoing = Math.atan2(after.y - middle.y, after.x - middle.x);
  const turn = Math.abs(outgoing - incoming) % (2 * Math.PI);
  return ((turn > Math.PI ? 2 * Math.PI - turn : turn) * 180) / Math.PI;
}

function distance(from: PlanePoint, to: PlanePoint): number {
  return Math.hypot(to.x - from.x, to.y - from.y);
}

function midpoint(from: PlanePoint, to: PlanePoint): PlanePoint {
  return { x: (from.x + to.x) / 2, y: (from.y + to.y) / 2 };
}

/** Returns `point` reflected through `center`. */
function mirror(point: PlanePoint, center: PlanePoint): PlanePoint {
  return { x: 2 * center.x - point.x, y: 2 * center.y - point.y };
}
