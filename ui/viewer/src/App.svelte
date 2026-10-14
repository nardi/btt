<script lang="ts">
  // The viewer: reads the trip from the link or from a trip file and shows it read-only.
  import type { TripSnapshot } from "@btt/shared/bindings";
  import ThemeSwitch from "@btt/shared/components/ThemeSwitch.svelte";
  import { onMount } from "svelte";

  import AboutPage from "./AboutPage.svelte";
  import { SnapshotLinkError } from "./link/errors";
  import { parseSnapshotFile } from "./link/snapshot-file";
  import { decodeSnapshotLink } from "./link/snapshot-link";
  import TripPage from "./TripPage.svelte";

  /** What the page shows in its main area. */
  type Content =
    | { phase: "welcome" }
    | { phase: "loading" }
    | { phase: "failed"; error: SnapshotLinkError }
    | { phase: "trip"; snapshot: TripSnapshot };

  let content: Content = $state({ phase: "welcome" });
  let showAbout = $state(false);
  let fileInput: HTMLInputElement | undefined = $state();

  /** Reads the trip from the fragment of the address, which is empty when the page has no trip. */
  async function openFragment() {
    const fragment = location.hash;
    if (fragment.length <= 1) {
      content = { phase: "welcome" };
      return;
    }
    content = { phase: "loading" };
    try {
      content = { phase: "trip", snapshot: await decodeSnapshotLink(fragment) };
    } catch (error) {
      content = { phase: "failed", error: asLinkError(error) };
    }
  }

  /** Reads a trip file the traveller picked. */
  async function openFile(event: Event & { currentTarget: HTMLInputElement }) {
    const file = event.currentTarget.files?.[0];
    event.currentTarget.value = "";
    if (file === undefined) {
      return;
    }
    content = { phase: "loading" };
    try {
      content = { phase: "trip", snapshot: parseSnapshotFile(await file.text()) };
      showAbout = false;
    } catch (error) {
      content = { phase: "failed", error: asLinkError(error) };
    }
  }

  /** Turns anything a reader threw into an error with a kind, so the page can explain it. */
  function asLinkError(error: unknown): SnapshotLinkError {
    return error instanceof SnapshotLinkError
      ? error
      : new SnapshotLinkError("malformed", "the trip could not be read");
  }

  /** Explains a failure in plain words, with what the traveller can do about it. */
  function failureMessage(error: SnapshotLinkError): string {
    switch (error.kind) {
      case "cut":
        return (
          "This link looks cut off. Some chat apps drop the last characters of a long link. " +
          "Ask for the link again, and copy the whole link this time."
        );
      case "newer-schema":
        return (
          "This trip was made with a newer version of BigTrainTrip than this page knows. " +
          "Reload the page to get the newest viewer."
        );
      case "too-large":
        return "This link is larger than this page reads, so it is not a trip link from BigTrainTrip.";
      default:
        return "This link does not hold a trip from BigTrainTrip. Check that you copied the whole link.";
    }
  }

  /** Returns a date as a day and a month, such as "11 October". */
  function formatDay(isoDate: string): string {
    return new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "long", timeZone: "UTC" }).format(
      new Date(`${isoDate}T00:00:00Z`),
    );
  }

  onMount(() => {
    void openFragment();
    const reopen = () => {
      showAbout = false;
      void openFragment();
    };
    window.addEventListener("hashchange", reopen);
    return () => window.removeEventListener("hashchange", reopen);
  });

  // The tab title names the trip.
  $effect(() => {
    document.title = content.phase === "trip" ? `${content.snapshot.trip.name} on BigTrainTrip` : "BigTrainTrip trip";
  });
</script>

<main class:trip-open={content.phase === "trip" && !showAbout}>
  <header>
    <div class="title">
      <span class="brand">BigTrainTrip</span>
      {#if content.phase === "trip"}
        <h1 class="trip-name">{content.snapshot.trip.name}</h1>
      {/if}
    </div>
    <div class="actions">
      <button type="button" onclick={() => fileInput?.click()}>Open a trip file</button>
      <button type="button" onclick={() => (showAbout = !showAbout)} aria-pressed={showAbout}>About</button>
      <ThemeSwitch />
    </div>
    <input
      bind:this={fileInput}
      class="file-input"
      type="file"
      accept=".btt,application/json"
      onchange={openFile}
      tabindex="-1"
      aria-hidden="true"
    />
  </header>

  {#if showAbout}
    <AboutPage onClose={() => (showAbout = false)} />
  {:else if content.phase === "trip"}
    <p class="planned">
      Planned on {formatDay(content.snapshot.planned_at)}. Prices are estimates as of that day.
    </p>
    <TripPage snapshot={content.snapshot} />
  {:else if content.phase === "loading"}
    <p class="message">Opening the trip…</p>
  {:else if content.phase === "failed"}
    <section class="message" role="alert">
      <p>{failureMessage(content.error)}</p>
      {#if content.error.kind === "newer-schema"}
        <button type="button" onclick={() => location.reload()}>Reload the page</button>
      {/if}
    </section>
  {:else}
    <section class="welcome">
      <h1>A trip planned in BigTrainTrip</h1>
      <p>
        Open the link someone sent you, and the trip shows up here. You can also open a trip file with
        the button above. Nothing you open leaves your device, apart from the map tiles.
      </p>
    </section>
  {/if}
</main>

<style>
  main {
    max-width: 880px;
    margin: 0 auto;
    padding: var(--space-4);
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    box-sizing: border-box;
    min-height: 100dvh;
  }

  main.trip-open {
    max-width: none;
    height: 100dvh;
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    flex-wrap: wrap;
  }

  .title {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .brand {
    color: var(--color-text-muted);
    font-size: var(--text-caption);
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .trip-name {
    margin: 0;
    font-size: var(--text-title);
    line-height: 1.15;
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .file-input {
    display: none;
  }

  .planned {
    margin: 0;
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }

  .message,
  .welcome {
    max-width: 560px;
    margin: var(--space-6) auto;
  }

  .welcome h1 {
    font-size: var(--text-title);
    margin: 0 0 var(--space-3);
  }

  @media (max-width: 760px) {
    .trip-name {
      font-size: var(--text-section);
    }
  }
</style>
