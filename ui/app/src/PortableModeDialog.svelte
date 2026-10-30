<script lang="ts">
  // Asks the user where the portable build keeps its data. The question appears on the first launch only.
  import { commands } from "@btt/shared/bindings";
  import { commandErrorMessage } from "./tauri-trip-source.svelte";

  let { onAnswered }: { onAnswered: () => void } = $props();

  /** The message of the last failed command, or `null` when none failed. */
  let message: string | null = $state(null);

  /** Whether a command runs, which disables the choices. */
  let saving = $state(false);

  /** Keeps the data in a folder next to the app. The app restarts, so the data moves there. */
  async function keepNextToApp() {
    saving = true;
    const result = await commands.setPortableMode(true);
    if (result.status === "error") {
      message = commandErrorMessage(result.error);
      saving = false;
      return;
    }
    await commands.restartApp();
  }

  /** Keeps the data in the user profile, where the app already keeps it. */
  async function keepInProfile() {
    saving = true;
    const result = await commands.setPortableMode(false);
    if (result.status === "error") {
      message = commandErrorMessage(result.error);
      saving = false;
      return;
    }
    onAnswered();
  }
</script>

<div class="backdrop" role="presentation">
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="portable-title">
    <h2 id="portable-title">Where should BigTrainTrip keep your data?</h2>
    <p>
      Your trips, travellers and timetable are stored on this computer either way. Next to the
      app, they go into a folder named <code>data</code> beside BigTrainTrip. Copy that folder with
      the app to take everything to another computer.
    </p>
    <p>
      In your user profile, the data stays with your user account. You can change this later on
      the About page.
    </p>

    {#if message !== null}
      <p class="message" role="alert">{message}</p>
    {/if}

    <div class="actions">
      <button type="button" disabled={saving} onclick={() => void keepInProfile()}>
        Keep it in my profile
      </button>
      <button type="button" class="primary" disabled={saving} onclick={() => void keepNextToApp()}>
        Keep it next to the app
      </button>
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

  .message {
    color: var(--color-text-muted);
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: var(--space-2);
  }
</style>
