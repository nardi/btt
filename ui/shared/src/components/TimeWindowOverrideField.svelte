<script lang="ts">
  // Edits a time window that may replace a default. Empty fields show the default as a
  // placeholder and mean "use the default". Text fields are used because the native time input
  // shows no placeholder.
  import type { DailyTimeWindow } from "../bindings";
  import { dailyTimeWindow, formatMinutesOfDay, parseMinutesOfDay } from "../format";

  let {
    label,
    window,
    defaultWindow,
    onChange,
  }: {
    label: string;
    /** The overriding window, or `null` when the default applies. */
    window: DailyTimeWindow | null;
    defaultWindow: DailyTimeWindow;
    /** Receives the new overriding window, or `null` when both fields are empty. */
    onChange: (window: DailyTimeWindow | null) => void;
  } = $props();

  const startPlaceholder = $derived(formatMinutesOfDay(defaultWindow.start));
  const endPlaceholder = $derived(formatMinutesOfDay(defaultWindow.start + defaultWindow.length));

  // The texts follow the saved window, and typing changes them until a valid entry is saved.
  let startText = $state("");
  let endText = $state("");
  let invalid = $state(false);
  $effect(() => {
    startText = window === null ? "" : formatMinutesOfDay(window.start);
    endText = window === null ? "" : formatMinutesOfDay(window.start + window.length);
    invalid = false;
  });

  /**
   * Saves the window when both fields are empty or hold valid times. A field left empty next
   * to a filled one takes its value from the default.
   */
  function commit() {
    if (startText.trim() === "" && endText.trim() === "") {
      invalid = false;
      if (window !== null) {
        onChange(null);
      }
      return;
    }
    const start = startText.trim() === "" ? defaultWindow.start : parseMinutesOfDay(startText);
    const end =
      endText.trim() === ""
        ? (defaultWindow.start + defaultWindow.length) % 1440
        : parseMinutesOfDay(endText);
    if (start === null || end === null) {
      invalid = true;
      return;
    }
    invalid = false;
    onChange(dailyTimeWindow(start, end));
  }
</script>

<label class="time-window-override-field">
  <span class="label">{label}</span>
  <input
    type="text"
    inputmode="numeric"
    size="5"
    placeholder={startPlaceholder}
    aria-invalid={invalid}
    bind:value={startText}
    onchange={commit}
  />
  <span aria-hidden="true">to</span>
  <input
    type="text"
    inputmode="numeric"
    size="5"
    placeholder={endPlaceholder}
    aria-invalid={invalid}
    bind:value={endText}
    onchange={commit}
  />
  {#if invalid}
    <span class="error">Enter times as HH:MM</span>
  {/if}
</label>

<style>
  .time-window-override-field {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .label {
    min-width: 10em;
    color: var(--color-text-muted);
  }

  input[aria-invalid="true"] {
    border-color: var(--color-danger);
  }

  .error {
    color: var(--color-danger);
  }
</style>
