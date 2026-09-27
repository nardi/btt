import { describe, expect, it } from "vitest";

import { circleRing, destinationPoint, haversineMeters } from "./geo";

const vienna = { latitude: 48.2085, longitude: 16.3725 };

describe("destinationPoint", () => {
  it("lands at the requested distance", () => {
    for (const bearing of [0, 90, 180, 270]) {
      const point = destinationPoint(vienna, bearing, 5000);
      expect(haversineMeters(vienna, point)).toBeCloseTo(5000, 3);
    }
  });

  it("goes east along the same latitude for a bearing of 90 degrees", () => {
    const point = destinationPoint(vienna, 90, 1000);
    expect(point.latitude).toBeCloseTo(vienna.latitude, 3);
    expect(point.longitude).toBeGreaterThan(vienna.longitude);
  });
});

describe("circleRing", () => {
  it("closes the ring", () => {
    const ring = circleRing(vienna, 2000, 16);
    expect(ring).toHaveLength(17);
    expect(ring[0]![0]).toBeCloseTo(ring[16]![0], 9);
    expect(ring[0]![1]).toBeCloseTo(ring[16]![1], 9);
  });
});

describe("haversineMeters", () => {
  it("matches the Rust result for Amsterdam to Berlin", () => {
    const amsterdam = { latitude: 52.3789, longitude: 4.9003 };
    const berlin = { latitude: 52.5251, longitude: 13.3694 };
    const kilometers = haversineMeters(amsterdam, berlin) / 1000;
    expect(kilometers).toBeGreaterThan(570);
    expect(kilometers).toBeLessThan(580);
  });
});
