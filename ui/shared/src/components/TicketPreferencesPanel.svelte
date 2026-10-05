<script lang="ts">
  // Trip-wide choices that decide how tickets are priced.
  import type { FareType, TicketPreferences } from "../bindings";

  let {
    preferences,
    onChange,
  }: {
    preferences: TicketPreferences;
    /** Missing in a read-only view. */
    onChange?: (preferences: TicketPreferences) => void;
  } = $props();

  const disabled = $derived(onChange === undefined);

  function update(changes: Partial<TicketPreferences>) {
    onChange?.({ ...preferences, ...changes });
  }
</script>

<section class="ticket-preferences">
  <h3>Tickets</h3>
  <label>
    <input
      type="checkbox"
      checked={preferences.consider_passes}
      {disabled}
      onchange={(event) => update({ consider_passes: event.currentTarget.checked })}
    />
    Compare with an Interrail pass
  </label>
  <label>
    <input
      type="checkbox"
      checked={preferences.consider_families}
      {disabled}
      onchange={(event) => update({ consider_families: event.currentTarget.checked })}
    />
    Children travel free with an adult on a pass
  </label>
  <label>
    <span class="label">Single tickets</span>
    <select
      value={preferences.fare_type}
      {disabled}
      onchange={(event) => update({ fare_type: event.currentTarget.value as FareType })}
    >
      <option value="Flexible">Flexible</option>
      <option value="Saver">Saver, tied to one train</option>
    </select>
  </label>
  <label>
    <input
      type="checkbox"
      checked={preferences.reserve_optional_seats}
      {disabled}
      onchange={(event) => update({ reserve_optional_seats: event.currentTarget.checked })}
    />
    Reserve seats where a reservation is optional
  </label>
  <label>
    <input
      type="checkbox"
      checked={preferences.live_prices}
      {disabled}
      onchange={(event) => update({ live_prices: event.currentTarget.checked })}
    />
    Look up live prices on booking sites
    <span class="hint">unofficial, sbb.ch only for now</span>
  </label>
  <label>
    <span class="label">Buying on</span>
    <input
      type="date"
      value={preferences.assumed_purchase_date ?? ""}
      {disabled}
      onchange={(event) => update({ assumed_purchase_date: event.currentTarget.value || null })}
    />
    <span class="hint">empty means today</span>
  </label>
</section>

<style>
  .ticket-preferences {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }

  h3 {
    margin: 0;
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
</style>
