import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";

// Tauri loads the dev server at this fixed port, see `src-tauri/tauri.conf.json`.
const tauriDevServerPort = 5173;

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: tauriDevServerPort,
    strictPort: true,
  },
  build: {
    target: "es2022",
  },
});
