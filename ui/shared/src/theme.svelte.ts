// The theme the app shows and the paint colours the map needs in code, because the map cannot
// read the CSS variables. The app follows the system preference, as the restyle plan asks.

import type { StyleSpecification } from "maplibre-gl";

import lightStyle from "./map-styles/light.json";
import darkStyle from "./map-styles/dark.json";

/** The theme the app can show. */
export type Theme = "light" | "dark";

/** Returns the theme the system prefers right now. */
function systemTheme(): Theme {
  return typeof matchMedia === "function" && matchMedia("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

/** The theme the app currently shows, which follows the system preference. */
export const theme = $state({ current: systemTheme() });

if (typeof matchMedia === "function") {
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", (event) => {
    theme.current = event.matches ? "dark" : "light";
  });
}

/** The derived base style of the map, per theme. Both keep the OpenFreeMap attribution. */
export function mapStyle(current: Theme): StyleSpecification {
  return (current === "dark" ? darkStyle : lightStyle) as unknown as StyleSpecification;
}

/** The leg palette as paint colours, which cycles by leg position. */
export const legPalette: Record<Theme, [string, string, string, string]> = {
  light: ["#e4484f", "#f2a93b", "#2fa876", "#0fb0c9"],
  dark: ["#ff6b6b", "#ffb84d", "#3ed598", "#45d4e8"],
};

/** The colour of a night leg, per theme. */
export const nightColour: Record<Theme, string> = { light: "#6d28d9", dark: "#a78bfa" };

/** The ink of the map dots and lines that are not part of the palette. */
export const mapInk: Record<Theme, string> = { light: "#1c1d21", dark: "#ecedf0" };

/** The accent, which marks the interactive parts, such as a selected place. */
export const mapAccent: Record<Theme, string> = { light: "#2456c8", dark: "#8ab0ff" };

/** The muted ink of the ways between a place and its stations. */
export const mapMuted: Record<Theme, string> = { light: "#62666f", dark: "#9a9fab" };

/** The surface colour, which the transfer dots fill with. */
export const mapSurface: Record<Theme, string> = { light: "#ffffff", dark: "#191a21" };
