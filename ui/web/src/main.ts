import "@btt/shared/styles/theme.css";
// Marks the document with the theme before the app mounts, so the first paint has its colours.
import "@btt/shared/theme.svelte";

import { mount } from "svelte";

import App from "@btt/app-source/App.svelte";

const target = document.getElementById("app");
if (target === null) {
  throw new Error("index.html has no #app element");
}

// The web app has no backend for problem reports, so the header leaves the button out.
export default mount(App, { target, props: { canReportProblems: false } });
