<script lang="ts">
  // Trip-wide planning preferences.
  import type { TripEdit, TripPreferences } from "../bindings";
  import TimeWindowField from "./TimeWindowField.svelte";

  let {
    preferences,
    onEdit,
  }: {
    preferences: TripPreferences;
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

<section class="preferences">
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
    <label>
      <span class="label">Transfers</span>
      <span class="hint">shortest time</span>
      <input
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
        type="number"
        min="0"
        max="120"
        value={preferences.minimum_transfer_time}
        {disabled}
        onchange={(event) =>
          update({ minimum_transfer_time: Math.max(0, event.currentTarget.valueAsNumber || 0) })}
      />
      minutes
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
    <label>
      <span class="label">Same-day stops</span>
      at least
      <input
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
      hours, at most
      <input
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
      hours
    </label>
  </div>
</section>

<style>
  .preferences {
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    padding: var(--space-2) var(--space-3);
  }

  h3 {
    margin: 0;
    font-weight: 600;
  }

  .fields {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    margin-top: var(--space-3);
  }

  label {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .label {
    min-width: 7em;
    color: var(--color-text-muted);
  }

  .hint {
    color: var(--color-text-muted);
  }

  input[type="number"] {
    width: 5em;
  }
</style>
