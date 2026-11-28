import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import type { StyleSpecification } from "maplibre-gl";

import lightStyle from "./map-styles/light.json";
import darkStyle from "./map-styles/dark.json";
import { withOfflineBasemap } from "./offline-basemap";

const palette = { water: "#000", land: "#fff", border: "#888", text: "#444", halo: "#fff" };

/** The features of the data file that the offline base map draws. */
const basemap = JSON.parse(
  readFileSync(new URL("./map-data/basemap.geojson", import.meta.url), "utf8"),
) as { features: { properties: { kind: string; name?: string }; geometry: { type: string } }[] };

describe("offline base map data", () => {
  it("has land, lakes, borders and cities", () => {
    const kinds = new Set(basemap.features.map((feature) => feature.properties.kind));
    expect([...kinds].sort()).toEqual(["border", "city", "lake", "land"]);
  });

  it("has Vienna among the cities", () => {
    const names = basemap.features
      .filter((feature) => feature.properties.kind === "city")
      .map((feature) => feature.properties.name);
    expect(names).toContain("Vienna");
  });
});

describe("map style", () => {
  for (const [theme, online] of [["light", lightStyle], ["dark", darkStyle]] as const) {
    it(`draws the offline layers under the online layers in the ${theme} theme`, () => {
      const style = withOfflineBasemap(online as unknown as StyleSpecification, palette);
      const ids = style.layers.map((layer) => layer.id);
      expect(ids[0]).toBe("background");
      expect(ids.slice(1, 5)).toEqual([
        "offline-sea",
        "offline-land",
        "offline-lake",
        "offline-border",
      ]);
      expect(ids.indexOf("water")).toBeGreaterThan(ids.indexOf("offline-border"));
      expect(ids[ids.length - 1]).toBe("offline-city");
      expect(Object.keys(style.sources)).toEqual(["openmaptiles", "offline-basemap"]);
    });
  }
});
