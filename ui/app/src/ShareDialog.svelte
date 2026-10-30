<script lang="ts">
  // Shares the open trip with the web viewer, as a link, as a QR code or as a file. The link
  // holds the whole trip, so the dialog says who can read it.
  import { commands, type SharedTrip, type TripId } from "@btt/shared/bindings";
  import QrCode from "@btt/shared/components/QrCode.svelte";
  import { qrCodeImage, qrCodeOf } from "@btt/shared/qr";
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

  /** Whether the link holds the alternative journeys of every leg, which make it longer. */
  let includeAlternatives = $state(false);
  /** Whether the link holds the price estimates of the journeys, which make it longer. */
  let includePrices = $state(true);
  /**
   * Whether the link leaves out the names of places that the city dictionary cannot shorten,
   * and the viewer names them by their nearest station.
   */
  let namePlacesByStation = $state(false);
  /** Whether the link is written in the big alphabet, which has less than half the characters. */
  let compact = $state(true);

  /** The link, once the backend wrote it. */
  let link: SharedTrip | null = $state(null);
  /** Why there is no link, such as a trip that is too big for one. */
  let linkProblem: string | null = $state(null);
  let message: string | null = $state(null);
  let saving = $state(false);

  /** The QR code of the trip, or `null` when the trip needs a bigger code than the app draws. */
  const qrCode = $derived.by(() => {
    const qr = link?.qr;
    return qr == null ? null : qrCodeOf(qr.address, qr.payload);
  });

  /** Formats a number of characters, such as "3.2 K". */
  function formatCharacters(count: number): string {
    return count < 1000 ? String(count) : `${(count / 1000).toFixed(1)} K`;
  }

  /** Numbers the requests for a link, so that a late answer to an older request is dropped. */
  let requestNumber = 0;

  /** Asks the backend for the link with the current choices of what it holds and how. */
  async function writeLink() {
    if (shared.unfinishedLegs > 0) {
      return;
    }
    requestNumber += 1;
    const thisRequest = requestNumber;
    const result = await commands.shareTrip(
      tripId,
      shared.legs,
      includeAlternatives,
      includePrices,
      namePlacesByStation,
      compact,
    );
    if (thisRequest !== requestNumber) {
      return;
    }
    if (result.status === "ok") {
      link = result.data;
      linkProblem = null;
    } else if (result.error === "TripTooLargeForLink") {
      link = null;
      linkProblem = includeAlternatives
        ? "Too big for a link with alternatives. Leave them out or save as a file."
        : "Too big for a link. Save as a file instead.";
    } else {
      link = null;
      linkProblem = commandErrorMessage(result.error);
    }
  }

  onMount(writeLink);

  async function copyLink() {
    if (link === null) {
      return;
    }
    try {
      await navigator.clipboard.writeText(link.url);
      message = "Link copied.";
    } catch {
      message = "Could not copy. Select the link and copy it by hand.";
    }
  }

  /** Copies the QR code as a picture, to paste in a message or a document. */
  async function copyQrCode() {
    if (qrCode === null) {
      return;
    }
    try {
      const image = qrCodeImage(qrCode, 8);
      const canvas = document.createElement("canvas");
      canvas.width = image.width;
      canvas.height = image.height;
      canvas.getContext("2d")?.putImageData(new ImageData(image.data, image.width, image.height), 0, 0);
      const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
      if (blob === null) {
        throw new Error("no picture");
      }
      await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
      message = "QR code copied.";
    } catch {
      message = "Could not copy the QR code.";
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
    <p>Anyone with the link, QR code or file can read the trip. Traveller names and ages are not included.</p>

    {#if shared.unfinishedLegs === 0}
      <div class="options">
        <label class="option">
          <input type="checkbox" bind:checked={compact} onchange={() => void writeLink()} />
          Compact link (~50% size reduction)
        </label>
        <label class="option">
          <input
            type="checkbox"
            bind:checked={includeAlternatives}
            onchange={() => void writeLink()}
          />
          Include alternative journeys (~3x larger)
        </label>
        <label class="option">
          <input type="checkbox" bind:checked={includePrices} onchange={() => void writeLink()} />
          Include price estimates
        </label>
        <label class="option">
          <input
            type="checkbox"
            bind:checked={namePlacesByStation}
            onchange={() => void writeLink()}
          />
          Name places by nearest station
        </label>
      </div>
    {/if}

    {#if shared.unfinishedLegs > 0}
      <p class="message">
        {shared.unfinishedLegs === 1 ? "One leg is" : `${shared.unfinishedLegs} legs are`} still being planned.
        Share the trip once planning is done.
      </p>
    {:else if link !== null}
      <label class="link">
        <span class="label">Link</span>
        <input type="text" readonly value={link.url} onfocus={(event) => event.currentTarget.select()} />
      </label>
      <p class="size">{formatCharacters(link.link_characters)} characters.</p>
      {#if link.long_for_messengers}
        <p class="message">
          This link is long. Some messengers cut long links, so a file may be safer.
        </p>
      {/if}
      {#if qrCode !== null}
        <div class="qr">
          <QrCode matrix={qrCode} label="QR code of the trip" />
          <p class="size">Scan it with a phone camera.</p>
          <button type="button" onclick={copyQrCode}>Copy QR code</button>
        </div>
      {:else}
        <p class="message">
          Too big for a QR code.
          {includeAlternatives ? "Leave out the alternatives for a smaller code." : "Send the link or the file."}
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
    overflow-y: auto;
    background: rgb(0 0 0 / 35%);
  }

  .dialog {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    width: min(600px, calc(100vw - 2 * var(--space-4)));
    max-height: calc(100vh - 2 * var(--space-4));
    overflow-y: auto;
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

  .options {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .option {
    display: flex;
    align-items: flex-start;
    gap: var(--space-2);
    font-size: var(--text-small);
  }

  .qr {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-2);
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
