// The theme the app shows and the paint colours the map needs in code, because the map cannot
// read the CSS variables. The app follows the system preference unless the traveller picks a
// theme, and the pick stays in the browser storage of this device.

import type { StyleSpecification } from "maplibre-gl";

import lightStyle from "./map-styles/light.json";
import darkStyle from "./map-styles/dark.json";
import { withOfflineBasemap, type OfflinePalette } from "./offline-basemap";

/** The theme the app can show. */
export type Theme = "light" | "dark";

/** What the traveller picked: a theme, or to follow the system. */
export type ThemeChoice = Theme | "system";

/** The key of the traveller's pick in the browser storage. */
const themeChoiceKey = "btt.theme";

/** Returns the theme the system prefers right now. */
function systemTheme(): Theme {
  return typeof matchMedia === "function" && matchMedia("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

/** Returns the pick stored on this device, or to follow the system when there is none. */
function storedChoice(): ThemeChoice {
  try {
    const stored = localStorage.getItem(themeChoiceKey);
    return stored === "light" || stored === "dark" ? stored : "system";
  } catch {
    return "system";
  }
}

/** The traveller's pick and the theme the app currently shows. */
export const theme = $state<{ choice: ThemeChoice; current: Theme }>({ choice: "system", current: "light" });

/** Sets the theme the app shows from the pick and the system, and marks the document with it. */
function applyTheme(): void {
  theme.current = theme.choice === "system" ? systemTheme() : theme.choice;
  if (typeof document !== "undefined") {
    document.documentElement.dataset.theme = theme.current;
  }
}

/** Stores the traveller's pick and shows its theme. */
export function setThemeChoice(choice: ThemeChoice): void {
  theme.choice = choice;
  try {
    if (choice === "system") {
      localStorage.removeItem(themeChoiceKey);
    } else {
      localStorage.setItem(themeChoiceKey, choice);
    }
  } catch {
    // Without storage the pick lasts until the app closes.
  }
  applyTheme();
}

theme.choice = storedChoice();
applyTheme();
if (typeof matchMedia === "function") {
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", applyTheme);
}

/** The colours of the offline base map, which match the land and the water of each style. */
const offlinePalette: Record<Theme, OfflinePalette> = {
  light: { water: "#d9e2e8", land: "#fafaf9", border: "#c4c4c0", text: "#62666f", halo: "#fafaf9" },
  dark: { water: "#20262e", land: "#101014", border: "#3a3d48", text: "#9a9fab", halo: "#101014" },
};

/**
 * The base style of the map, per theme. It draws the offline base map under the layers of
 * OpenFreeMap, and both keep the OpenFreeMap attribution. See `offline-basemap.ts`.
 */
export function mapStyle(current: Theme): StyleSpecification {
  const online = (current === "dark" ? darkStyle : lightStyle) as unknown as StyleSpecification;
  return withOfflineBasemap(online, offlinePalette[current]);
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
