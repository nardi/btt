<script lang="ts">
  // Explains the timetable bundle and downloads it. The app ships without one to stay small and
  // cannot plan without one, so this dialog stays open on the first start until a bundle is
  // installed. It also offers newer bundles later on.
  import {
    commands,
    type DownloadProgress,
    type TimetableStatus,
  } from "@btt/shared/bindings";
  import { formatDate } from "@btt/shared/format";
  import { Channel } from "@tauri-apps/api/core";

  import { commandErrorMessage } from "./tauri-trip-source.svelte";

  let {
    status,
    onStatus,
    onClose,
  }: {
    status: TimetableStatus;
    /** Called with the status after every check or download. */
    onStatus: (status: TimetableStatus) => void;
    onClose: () => void;
  } = $props();

  /** The progress of the running download, or `null` when none runs. */
  let progress = $state<DownloadProgress | null>(null);
  let checking = $state(false);
  let message: string | null = $state(null);

  const downloading = $derived(progress !== null);
  /** Whether the app has no timetable, so the dialog cannot close before a download. */
  const required = $derived(status.installed === null);
  /** Fraction from 0 to 1 of the download that arrived, or `null` while it unpacks. */
  const fraction = $derived(
    progress === null || progress.stage === "Unpacking" || progress.total_bytes === 0
      ? null
      : progress.received_bytes / progress.total_bytes,
  );

  /** Formats a size in bytes, such as "24.3 MB". */
  function formatBytes(bytes: number): string {
    return `${(bytes / 1_000_000).toFixed(1)} MB`;
  }

  /** Asks the server for the newest bundle. */
  async function check() {
    checking = true;
    message = null;
    const result = await commands.checkTimetable();
    checking = false;
    if (result.status === "error") {
      message = commandErrorMessage(result.error);
    } else {
      onStatus(result.data);
    }
  }

  /** Downloads the newest bundle and shows the progress. */
  async function download() {
    message = null;
    progress = { stage: "Downloading", received_bytes: 0, total_bytes: 0 };
    const events = new Channel<DownloadProgress>();
    events.onmessage = (update) => (progress = update);
    const result = await commands.downloadTimetable(events);
    progress = null;
    if (result.status === "error") {
      message = commandErrorMessage(result.error);
    } else {
      onStatus(result.data);
    }
  }

  /** Saves whether the app checks for a newer bundle on startup. */
  async function setChecks(enabled: boolean) {
    const result = await commands.setTimetableChecks(enabled);
    if (result.status === "error") {
      message = commandErrorMessage(result.error);
    } else {
      onStatus({ ...status, checks_enabled: enabled });
    }
  }
</script>

<div class="backdrop" role="presentation">
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="timetable-title">
    {#if status.restart_needed}
      <h2 id="timetable-title">Timetable downloaded</h2>
      <p>The app plans with the new timetable after it restarts.</p>
    {:else if status.installed === null}
      <h2 id="timetable-title">Timetable needed</h2>
      <p>
        Planning a journey needs the train timetables of Europe. The app does not ship with them,
        which keeps the app small and lets the timetables stay up to date. The app cannot plan a
        trip until it has them.
      </p>
    {:else if status.installed.days_left < 0}
      <h2 id="timetable-title">Timetable expired</h2>
      <p>
        The timetable ended on {formatDate(status.installed.valid_until)}. The app finds no
        journeys on later days.
      </p>
    {:else}
      <h2 id="timetable-title">Timetable update</h2>
      <p>
        The installed timetable covers {formatDate(status.installed.valid_from)} to {formatDate(
          status.installed.valid_until,
        )}.
      </p>
    {/if}

    {#if !status.restart_needed}
      {#if status.available === null}
        {#if status.check_error !== null}
          <p class="message" role="alert">
            The app could not reach the timetable server. Check the internet connection and try
            again. {status.check_error}
          </p>
        {:else if !status.checks_enabled}
          <p class="message">The app does not check for timetables on its own.</p>
        {:else}
          <p class="message">Looking for the newest timetable…</p>
        {/if}
      {:else if !status.available.readable}
        <p class="message">
          The newest timetable needs app version {status.available.minimum_app_version} or newer.
          Update the app to download it.
        </p>
      {:else if status.available.newer}
        <p>
          The newest timetable covers until {formatDate(status.available.valid_until)}. The
          download is {formatBytes(status.available.size_bytes)}.
        </p>
      {:else}
        <p class="message">The installed timetable is the newest.</p>
      {/if}
    {/if}

    {#if downloading}
      <div class="progress">
        <progress value={fraction ?? undefined} max="1"></progress>
        <p class="size">
          {#if progress?.stage === "Unpacking"}
            Unpacking and checking the timetable…
          {:else if progress !== null}
            {formatBytes(progress.received_bytes)} of {formatBytes(progress.total_bytes)}
          {/if}
        </p>
      </div>
    {/if}

    {#if message !== null}
      <p class="message" role="alert">{message}</p>
    {/if}

    {#if !required}
      <label class="option">
        <input
          type="checkbox"
          checked={status.checks_enabled}
          disabled={downloading}
          onchange={(event) => void setChecks(event.currentTarget.checked)}
        />
        Look for a newer timetable when the app starts
      </label>
    {/if}

    <div class="actions">
      {#if status.restart_needed}
        {#if !required}
          <button type="button" onclick={onClose}>Later</button>
        {/if}
        <button type="button" class="primary" onclick={() => void commands.restartApp()}>
          Restart now
        </button>
      {:else}
        {#if !required}
          <button type="button" disabled={downloading} onclick={onClose}>Close</button>
        {/if}
        {#if status.available === null}
          <button type="button" disabled={checking} onclick={() => void check()}>
            {checking ? "Checking…" : status.check_error !== null ? "Try again" : "Check now"}
          </button>
        {:else if status.available.readable && status.available.newer}
          <button type="button" class="primary" disabled={downloading} onclick={() => void download()}>
            {#if downloading}
              Downloading…
            {:else if message !== null}
              Try again
            {:else}
              Download {formatBytes(status.available.size_bytes)}
            {/if}
          </button>
        {/if}
      {/if}
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
    width: min(520px, calc(100vw - 2 * var(--space-4)));
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

  .message,
  .size {
    color: var(--color-text-muted);
    overflow-wrap: anywhere;
  }

  .size {
    font-size: var(--text-small);
  }

  .progress {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  progress {
    width: 100%;
  }

  .option {
    display: flex;
    align-items: flex-start;
    gap: var(--space-2);
    font-size: var(--text-small);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }
</style>
