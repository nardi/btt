<script lang="ts">
  // Trip-wide planning preferences.
  import type { TripEdit, TripPreferences } from "../bindings";
  import Icon from "./Icon.svelte";
  import TimeWindowField from "./TimeWindowField.svelte";

  let {
    preferences,
    hasSameDayStop = false,
    onEdit,
  }: {
    preferences: TripPreferences;
    /** Whether the trip has a same-day stop, which opens the disclosure by default. */
    hasSameDayStop?: boolean;
    /** Missing in a read-only view. */
    onEdit?: (edit: TripEdit) => void;
  } = $props();

  const disabled = $derived(onEdit === undefined);
  const stayMaximum = $derived(
    preferences.default_same_day_stay.maximum_beyond_minimum === null
      ? null
      : preferences.default_same_day_stay.minimum +
          preferences.default_same_day_stay.maximum_beyond_minimum,
  );

  function update(changes: Partial<TripPreferences>) {
    onEdit?.({ SetTripPreferences: { ...preferences, ...changes } });
  }

  /** Sets the same-day stay range from hours. A maximum below the minimum is raised to it. */
  function updateStayRange(minimumHours: number, maximumHours: number | null) {
    const minimum = Math.max(0, Math.round(minimumHours * 60));
    const maximum = maximumHours === null ? null : Math.max(minimum, Math.round(maximumHours * 60));
    update({
      default_same_day_stay: {
        minimum,
        maximum_beyond_minimum: maximum === null ? null : maximum - minimum,
      },
    });
  }

  /** Reads a number input, treating an empty field as `null`. */
  function optionalNumber(input: HTMLInputElement): number | null {
    return input.value === "" ? null : input.valueAsNumber;
  }

  /** The presets the weight control offers, with the weight each stores. */
  const PRICE_WEIGHTS: { label: string; weight: number }[] = [
    { label: "Time first", weight: 0 },
    { label: "Balanced, 1 min per €", weight: 100 },
    { label: "Frugal, 3 min per €", weight: 300 },
    { label: "Cheapest, 8 min per €", weight: 800 },
  ];
</script>

<section class="card">
  <h3>Planning</h3>
  <div class="fields">
    <TimeWindowField
      label="Depart between"
      window={preferences.default_windows.departure}
      {disabled}
      onChange={(departure) =>
        update({ default_windows: { ...preferences.default_windows, departure } })}
    />
    <TimeWindowField
      label="Arrive between"
      window={preferences.default_windows.arrival}
      {disabled}
      onChange={(arrival) => update({ default_windows: { ...preferences.default_windows, arrival } })}
    />
    <label class="slider-row">
      <span class="label">Transfers</span>
      <span class="hint">shortest time</span>
      <input
        class="range"
        type="range"
        min="0"
        max="100"
        value={preferences.transfer_aversion}
        {disabled}
        onchange={(event) => update({ transfer_aversion: event.currentTarget.valueAsNumber })}
      />
      <span class="hint">fewest transfers</span>
    </label>
    <label>
      <span class="label">Minimum transfer</span>
      <input
        class="numbers"
        type="number"
        min="0"
        max="120"
        value={preferences.minimum_transfer_time}
        {disabled}
        onchange={(event) =>
          update({ minimum_transfer_time: Math.max(0, event.currentTarget.valueAsNumber || 0) })}
      />
      <span class="hint">minutes</span>
    </label>
    <label>
      <span class="label">Price</span>
      <select
        value={preferences.price_weight}
        {disabled}
        onchange={(event) => update({ price_weight: Number(event.currentTarget.value) })}
      >
        {#each PRICE_WEIGHTS as preset (preset.weight)}
          <option value={preset.weight}>{preset.label}</option>
        {/each}
      </select>
    </label>
    <!-- The same-day stay defaults matter only once the trip has a same-day stop. -->
    <details class="disclosure" open={hasSameDayStop}>
      <summary>
        Same-day stops
        <Icon name="chevron" />
      </summary>
      <label>
        <span class="label">Stay between</span>
        <span class="hint">at least</span>
        <input
          class="numbers"
          type="number"
          min="0"
          max="24"
          step="0.5"
          value={preferences.default_same_day_stay.minimum / 60}
          {disabled}
          onchange={(event) =>
            updateStayRange(
              event.currentTarget.valueAsNumber || 0,
              stayMaximum === null ? null : stayMaximum / 60,
            )}
        />
        <span class="hint">hours, at most</span>
        <input
          class="numbers"
          type="number"
          min="0"
          max="24"
          step="0.5"
          placeholder="no limit"
          value={stayMaximum === null ? "" : stayMaximum / 60}
          {disabled}
          onchange={(event) =>
            updateStayRange(
              preferences.default_same_day_stay.minimum / 60,
              optionalNumber(event.currentTarget),
            )}
        />
        <span class="hint">hours</span>
      </label>
    </details>
  </div>
</section>

<style>
  .fields {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }

  label {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
  }

  .label {
    min-width: 8.5em;
    color: var(--color-text-muted);
  }

  .hint {
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }

  input[type="number"] {
    width: 5em;
  }

  /* The slider row keeps its two hints on the line, and the slider takes what is left. */
  .slider-row {
    flex-wrap: nowrap;
  }

  .range {
    flex: 1;
    min-width: 3em;
    padding: 0;
  }

  /* The summary of the disclosure is drawn as a button, with a chevron that turns over when the
     disclosure is open. The disclosure opens by default when the trip has a same-day stop. */
  .disclosure summary {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 5px 14px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-pill);
    background: var(--color-surface);
    font-size: var(--text-button);
    font-weight: 600;
    cursor: pointer;
    list-style: none;
    transition:
      background-color 130ms ease,
      border-color 130ms ease;
  }

  .disclosure summary::-webkit-details-marker {
    display: none;
  }

  .disclosure summary:hover {
    background: var(--color-surface-muted);
    border-color: var(--color-text-muted);
  }

  .disclosure summary:focus-visible {
    outline: 2px solid var(--color-accent);
    outline-offset: 1px;
  }

  .disclosure[open] summary :global(.icon) {
    transform: rotate(180deg);
  }

  .disclosure label {
    margin-top: var(--space-3);
  }
</style>
