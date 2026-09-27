// Small spherical geometry helpers for the map.

import type { GeoCoordinates } from "./bindings";

/** Mean Earth radius, the same value the Rust side uses. */
const earthRadiusMeters = 6_371_008.8;

const toRadians = (degrees: number) => (degrees * Math.PI) / 180;
const toDegrees = (radians: number) => (radians * 180) / Math.PI;

/** Returns the great-circle distance between two points, using the haversine formula. */
export function haversineMeters(from: GeoCoordinates, to: GeoCoordinates): number {
  const latitudeFrom = toRadians(from.latitude);
  const latitudeTo = toRadians(to.latitude);
  const latitudeDelta = latitudeTo - latitudeFrom;
  const longitudeDelta = toRadians(to.longitude - from.longitude);
  const halfChord =
    Math.sin(latitudeDelta / 2) ** 2 +
    Math.cos(latitudeFrom) * Math.cos(latitudeTo) * Math.sin(longitudeDelta / 2) ** 2;
  return 2 * earthRadiusMeters * Math.asin(Math.sqrt(halfChord));
}

/** Returns the point at a distance and compass bearing from a start point. */
export function destinationPoint(
  start: GeoCoordinates,
  bearingDegrees: number,
  distanceMeters: number,
): GeoCoordinates {
  const angularDistance = distanceMeters / earthRadiusMeters;
  const bearing = toRadians(bearingDegrees);
  const latitudeStart = toRadians(start.latitude);
  const longitudeStart = toRadians(start.longitude);
  const latitude = Math.asin(
    Math.sin(latitudeStart) * Math.cos(angularDistance) +
      Math.cos(latitudeStart) * Math.sin(angularDistance) * Math.cos(bearing),
  );
  const longitude =
    longitudeStart +
    Math.atan2(
      Math.sin(bearing) * Math.sin(angularDistance) * Math.cos(latitudeStart),
      Math.cos(angularDistance) - Math.sin(latitudeStart) * Math.sin(latitude),
    );
  return { latitude: toDegrees(latitude), longitude: toDegrees(longitude) };
}

/** Returns the ring of a circle on the Earth's surface as `[longitude, latitude]` pairs. */
export function circleRing(
  center: GeoCoordinates,
  radiusMeters: number,
  steps = 64,
): [number, number][] {
  const ring: [number, number][] = [];
  for (let step = 0; step <= steps; step++) {
    const point = destinationPoint(center, (step / steps) * 360, radiusMeters);
    ring.push([point.longitude, point.latitude]);
  }
  return ring;
}
