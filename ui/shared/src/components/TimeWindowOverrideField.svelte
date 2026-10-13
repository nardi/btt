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
    disabled = false,
    onChange,
  }: {
    label: string;
    /** The overriding window, or `null` when the default applies. */
    window: DailyTimeWindow | null;
    defaultWindow: DailyTimeWindow;
    /** Whether the fields only show the window, as in a read-only view. */
    disabled?: boolean;
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
    placeholder={startPlaceholder}
    aria-invalid={invalid}
    {disabled}
    bind:value={startText}
    onchange={commit}
  />
  <span aria-hidden="true">to</span>
  <input
    type="text"
    inputmode="numeric"
    placeholder={endPlaceholder}
    aria-invalid={invalid}
    {disabled}
    bind:value={endText}
    onchange={commit}
  />
  {#if invalid}
    <span class="error">Enter times as HH:MM</span>
  {/if}
</label>

<style>
  /* The field wraps rather than widening the itinerary column, which is narrow next to the map. */
  .time-window-override-field {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-1) var(--space-2);
  }

  .label {
    min-width: 9em;
    color: var(--color-text-muted);
  }

  input {
    width: 4.5em;
    min-width: 0;
  }

  input[aria-invalid="true"] {
    border-color: var(--color-danger);
  }

  .error {
    flex-basis: 100%;
    color: var(--color-danger);
  }
</style>
