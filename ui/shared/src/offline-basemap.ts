// The base map that the app draws without a network connection. The land, the lakes, the country
// borders and the biggest cities are in the file `map-data/basemap.geojson`, which
// `tools/offline-basemap.js` writes. The style draws them under the layers of OpenFreeMap and
// hides them as soon as a tile of OpenFreeMap has loaded, so the detailed map replaces them.

import { addProtocol, type Map as MapLibreMap } from "maplibre-gl";
import type { LayerSpecification, SourceSpecification, StyleSpecification } from "maplibre-gl";

import basemapUrl from "./map-data/basemap.geojson?url";

/** The glyph files in this package, by the path of the file. */
const bundledGlyphs = import.meta.glob<string>("./map-data/glyphs/*/*.pbf", {
  query: "?url",
  import: "default",
  eager: true,
});

/** The scheme of the glyph URLs in the styles. */
const glyphScheme = "btt-glyphs";
/** The template of the glyph URLs in the styles. */
export const glyphTemplate = `${glyphScheme}://{fontstack}/{range}`;
/** The glyph server of OpenFreeMap, which serves the ranges that this package does not have. */
const onlineGlyphUrl = "https://tiles.openfreemap.org/fonts";

/** The ID of the source that holds the data of the offline base map. */
export const offlineSourceId = "offline-basemap";
/** The ID of the source of the tiles of OpenFreeMap. */
const onlineSourceId = "openmaptiles";
/** The prefix of the IDs of the layers that the app hides when the tiles of OpenFreeMap load. */
const offlineLayerPrefix = "offline-";

/** The colours of the offline base map in one theme. */
export interface OfflinePalette {
  /** The colour of the sea and of the lakes. */
  water: string;
  /** The colour of the land. */
  land: string;
  /** The colour of the country borders. */
  border: string;
  /** The colour of the names of the cities. */
  text: string;
  /** The colour of the outline of the names of the cities, which is the colour of the land. */
  halo: string;
}

/** The source of the offline base map. MapLibre needs an absolute URL for the file. */
export function offlineSource(): SourceSpecification {
  return {
    type: "geojson",
    data: new URL(basemapUrl, globalThis.document?.baseURI ?? "http://localhost/").href,
    attribution: "© Natural Earth, GeoNames",
  };
}

/** The layers under the layers of OpenFreeMap: sea, land, lakes and borders. */
export function offlineBaseLayers(palette: OfflinePalette): LayerSpecification[] {
  return [
    {
      id: `${offlineLayerPrefix}sea`,
      type: "background",
      paint: { "background-color": palette.water },
    },
    {
      id: `${offlineLayerPrefix}land`,
      type: "fill",
      source: offlineSourceId,
      filter: ["==", ["get", "kind"], "land"],
      paint: { "fill-color": palette.land, "fill-antialias": false },
    },
    {
      id: `${offlineLayerPrefix}lake`,
      type: "fill",
      source: offlineSourceId,
      filter: ["==", ["get", "kind"], "lake"],
      paint: { "fill-color": palette.water },
    },
    {
      id: `${offlineLayerPrefix}border`,
      type: "line",
      source: offlineSourceId,
      filter: ["==", ["get", "kind"], "border"],
      paint: { "line-color": palette.border, "line-width": 1 },
    },
  ];
}

/** The layer of the city names, above all other layers. A bigger city shows at a lower zoom. */
export function offlineLabelLayer(palette: OfflinePalette): LayerSpecification {
  return {
    id: `${offlineLayerPrefix}city`,
    type: "symbol",
    source: offlineSourceId,
    filter: ["==", ["get", "kind"], "city"],
    layout: {
      "text-field": ["get", "name"],
      "text-font": ["Noto Sans Regular"],
      "text-size": 12,
      "symbol-sort-key": ["get", "rank"],
    },
    paint: { "text-color": palette.text, "text-halo-color": palette.halo, "text-halo-width": 1.5 },
  };
}

/**
 * Returns the style with the offline layers added. The first layer of the online style is its
 * background, which has the colour of the land, so the sea and the land go directly above it.
 * The style also takes its glyphs through the scheme that `registerGlyphProtocol` registers.
 */
export function withOfflineBasemap(online: StyleSpecification, palette: OfflinePalette): StyleSpecification {
  const [background, ...onlineLayers] = online.layers;
  return {
    ...online,
    sources: { ...online.sources, [offlineSourceId]: offlineSource() },
    glyphs: glyphTemplate,
    layers: [
      ...(background === undefined ? [] : [background]),
      ...offlineBaseLayers(palette),
      ...onlineLayers,
      offlineLabelLayer(palette),
    ],
  };
}

/** Returns the URL of the glyph range, from this package when it has the file. */
function glyphUrl(fontstack: string, range: string): string {
  return bundledGlyphs[`./map-data/glyphs/${fontstack}/${range}.pbf`] ?? `${onlineGlyphUrl}/${fontstack}/${range}.pbf`;
}

let protocolRegistered = false;

/** Registers the glyph scheme with MapLibre. Calling it again does nothing. */
export function registerGlyphProtocol(): void {
  if (protocolRegistered) {
    return;
  }
  protocolRegistered = true;
  addProtocol(glyphScheme, async (parameters, abortController) => {
    const path = decodeURIComponent(parameters.url.slice(`${glyphScheme}://`.length));
    const separator = path.lastIndexOf("/");
    const url = glyphUrl(path.slice(0, separator), path.slice(separator + 1));
    const response = await fetch(url, { signal: abortController.signal });
    if (!response.ok) {
      throw new Error(`Glyph request failed with status ${response.status}.`);
    }
    return { data: await response.arrayBuffer() };
  });
}

/**
 * Shows the offline layers while the tiles of OpenFreeMap are missing and hides them when a tile
 * loads. A style change resets the layers, so the choice is applied again after each one.
 */
export function watchOnlineTiles(map: MapLibreMap): void {
  let online = false;

  function applyVisibility() {
    if (map.getStyle() === undefined) {
      return;
    }
    for (const layer of map.getStyle().layers) {
      if (layer.id.startsWith(offlineLayerPrefix)) {
        map.setLayoutProperty(layer.id, "visibility", online ? "none" : "visible");
      }
    }
  }

  function setOnline(value: boolean) {
    if (value !== online) {
      online = value;
      applyVisibility();
    }
  }

  map.on("sourcedata", (event) => {
    if (event.sourceId === onlineSourceId && "tile" in event && event.tile !== undefined) {
      setOnline(true);
    }
  });
  map.on("error", (event) => {
    if ((event as { sourceId?: string }).sourceId === onlineSourceId) {
      setOnline(false);
    }
  });
  map.on("styledata", applyVisibility);
}
