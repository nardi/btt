import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";

// GitHub Pages serves the viewer under the name of the repository, so a build links its files
// under `/btt/`. The development server serves from the root.
export default defineConfig(({ command }) => ({
  plugins: [svelte()],
  base: command === "build" ? "/btt/" : "/",
  build: {
    target: "es2022",
  },
}));
