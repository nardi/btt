import { fileURLToPath } from "node:url";

import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";

/** Returns the path of a file in this package. */
function here(path: string): string {
  return fileURLToPath(new URL(path, import.meta.url));
}

// The web app is the desktop interface with the Tauri API swapped for a worker that runs the
// backend as WebAssembly. The aliases below are the only places where the two differ.
export default defineConfig(({ command }) => ({
  // The viewer owns the root of the site, so the app lives in a folder beside it.
  base: command === "build" ? "/btt/app/" : "/",
  plugins: [svelte()],
  resolve: {
    alias: {
      "@tauri-apps/api/core": here("./src/shims/tauri-core.ts"),
      "@tauri-apps/plugin-opener": here("./src/shims/tauri-opener.ts"),
      "@btt/app-source": here("../app/src"),
    },
  },
  worker: { format: "es" },
  build: { target: "es2022" },
}));
