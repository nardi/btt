<script lang="ts">
  // Edits a daily time window as a local start and end time.
  import type { DailyTimeWindow } from "../bindings";
  import { dailyTimeWindow, formatMinutesOfDay, parseMinutesOfDay } from "../format";

  let {
    label,
    window,
    disabled = false,
    onChange,
  }: {
    label: string;
    window: DailyTimeWindow;
    disabled?: boolean;
    onChange: (window: DailyTimeWindow) => void;
  } = $props();

  const startText = $derived(formatMinutesOfDay(window.start));
  const endText = $derived(formatMinutesOfDay(window.start + window.length));

  /** Sends the new window when both inputs hold a valid time. */
  function changeBoundary(boundary: "start" | "end", text: string) {
    const minutes = parseMinutesOfDay(text);
    if (minutes === null) {
      return;
    }
    const start = boundary === "start" ? minutes : window.start;
    const end = boundary === "end" ? minutes : (window.start + window.length) % 1440;
    onChange(dailyTimeWindow(start, end));
  }
</script>

<label class="time-window-field">
  <span class="label">{label}</span>
  <input
    type="time"
    value={startText}
    {disabled}
    onchange={(event) => changeBoundary("start", event.currentTarget.value)}
  />
  <span aria-hidden="true">to</span>
  <input
    type="time"
    value={endText}
    {disabled}
    onchange={(event) => changeBoundary("end", event.currentTarget.value)}
  />
</label>

<style>
  .time-window-field {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .label {
    min-width: 7em;
    color: var(--color-text-muted);
  }
</style>
