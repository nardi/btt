<script lang="ts">
  // Shares the open trip with the web viewer, as a link or as a file. The link holds the whole
  // trip, so the dialog says who can read it.
  import { commands, type SharedTrip, type TripId } from "@btt/shared/bindings";
  import { sharedLegs } from "@btt/shared/shared-legs";
  import type { TripViewState } from "@btt/shared/trip-source";
  import { onMount, untrack } from "svelte";

  import { commandErrorMessage } from "./tauri-trip-source.svelte";

  let {
    tripId,
    viewState,
    onClose,
  }: {
    tripId: TripId;
    viewState: TripViewState;
    onClose: () => void;
  } = $props();

  /**
   * The legs as they are when the dialog opens, which the link and the file hold. A plan that
   * arrives while the dialog is open does not change them, so the link and the file agree.
   */
  const shared = untrack(() => sharedLegs(viewState));

  /** The link, once the backend wrote it. */
  let link: SharedTrip | null = $state(null);
  /** Why there is no link, such as a trip that is too big for one. */
  let linkProblem: string | null = $state(null);
  let message: string | null = $state(null);
  let saving = $state(false);

  /** Formats a number of characters, such as "3.2 K". */
  function formatCharacters(count: number): string {
    return count < 1000 ? String(count) : `${(count / 1000).toFixed(1)} K`;
  }

  onMount(async () => {
    if (shared.unfinishedLegs > 0) {
      return;
    }
    const result = await commands.shareTrip(tripId, shared.legs);
    if (result.status === "ok") {
      link = result.data;
    } else if (result.error === "TripTooLargeForLink") {
      linkProblem = "This trip is too big for a link. Save it as a file and send the file instead.";
    } else {
      linkProblem = commandErrorMessage(result.error);
    }
  });

  async function copyLink() {
    if (link === null) {
      return;
    }
    try {
      await navigator.clipboard.writeText(link.url);
      message = "Link copied.";
    } catch {
      message = "The link could not be copied. Select it and copy it by hand.";
    }
  }

  async function saveFile() {
    saving = true;
    message = null;
    const result = await commands.exportTripFile(tripId, shared.legs);
    saving = false;
    if (result.status === "error") {
      message = commandErrorMessage(result.error);
    } else if (result.data !== null) {
      message = `Saved to ${result.data}`;
    }
  }
</script>

<div class="backdrop" role="presentation">
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="share-title">
    <h2 id="share-title">Share this trip</h2>
    <p>
      Anyone with the link or the file can read the whole trip: the places, the dates and the
      journeys with their prices. They hold no traveller names or ages. The link carries the trip
      after the # sign, which browsers never send to a server.
    </p>

    {#if shared.unfinishedLegs > 0}
      <p class="message">
        {shared.unfinishedLegs === 1 ? "One leg is" : `${shared.unfinishedLegs} legs are`} still being planned.
        Share the trip once the planner is done, so it holds every journey.
      </p>
    {:else if link !== null}
      <label class="link">
        <span class="label">Link</span>
        <input type="text" readonly value={link.url} onfocus={(event) => event.currentTarget.select()} />
      </label>
      <p class="size">{formatCharacters(link.link_characters)} characters.</p>
      {#if link.long_for_messengers}
        <p class="message">
          This link is long, and some messengers cut long links. A cut link says so when it opens,
          but you may prefer to send the file.
        </p>
      {/if}
    {:else if linkProblem !== null}
      <p class="message">{linkProblem}</p>
    {:else}
      <p class="message">Writing the link…</p>
    {/if}

    {#if message !== null}
      <p class="message" role="status">{message}</p>
    {/if}
    <div class="actions">
      <button type="button" onclick={onClose}>Close</button>
      <button type="button" disabled={saving || shared.unfinishedLegs > 0} onclick={saveFile}>
        {saving ? "Saving…" : "Save as file…"}
      </button>
      <button type="button" class="primary" disabled={link === null} onclick={copyLink}>Copy link</button>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 10;
    display: grid;
    place-items: center;
    background: rgb(0 0 0 / 35%);
  }

  .dialog {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    width: min(560px, calc(100vw - 2 * var(--space-4)));
    padding: var(--space-4) var(--space-6);
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    background: var(--color-surface);
  }

  h2 {
    margin: 0;
    font-weight: 700;
    font-size: var(--text-heading);
  }

  p {
    margin: 0;
  }

  .link {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  .link .label {
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }

  .link input {
    font-family: monospace;
    font-size: var(--text-small);
  }

  .size {
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }

  .message {
    color: var(--color-text-muted);
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }
</style>
