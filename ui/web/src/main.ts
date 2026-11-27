import "@btt/shared/styles/theme.css";
// Marks the document with the theme before the app mounts, so the first paint has its colours.
import "@btt/shared/theme.svelte";

import { mount } from "svelte";

import { setAppFeatures } from "@btt/shared/app-features";
import App from "@btt/app-source/App.svelte";

const target = document.getElementById("app");
if (target === null) {
  throw new Error("index.html has no #app element");
}

// The web app has no backend for live prices, problem reports or the data location. The
// interface leaves these features out.
setAppFeatures({
  livePrices: false,
  problemReports: false,
  dataLocation: false,
  runsInBrowser: true,
});

export default mount(App, { target });
