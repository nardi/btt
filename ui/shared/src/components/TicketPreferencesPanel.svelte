<script lang="ts">
  // Trip-wide choices that decide how tickets are priced. The pass settings sit behind the
  // switch that makes them matter, because the optimizer reads them only for passes.
  import type { FareType, TicketPreferences, Traveller } from "../bindings";
  import { RESIDENCE_COUNTRIES } from "../traveller";

  let {
    preferences,
    travellers = [],
    onSaveTraveller,
    onChange,
  }: {
    preferences: TicketPreferences;
    /** The travellers travelling on this trip, whose residence the pass rules read. */
    travellers?: readonly Traveller[];
    /** Saves a change to a traveller, such as their residence. */
    onSaveTraveller?: (traveller: Traveller) => void;
    /** Missing in a read-only view. */
    onChange?: (preferences: TicketPreferences) => void;
  } = $props();

  const disabled = $derived(onChange === undefined);

  function update(changes: Partial<TicketPreferences>) {
    onChange?.({ ...preferences, ...changes });
  }

  /** Saves the residence of a traveller, which the Interrail pass rules read. */
  function setResidence(traveller: Traveller, residenceCountry: string) {
    onSaveTraveller?.({ ...traveller, residence_country: residenceCountry });
  }
</script>

<section class="card">
  <h3>Tickets</h3>
  <div class="fields">
    <label class="switch-row">
      <input
        type="checkbox"
        class="switch"
        checked={preferences.consider_passes}
        {disabled}
        onchange={(event) => update({ consider_passes: event.currentTarget.checked })}
      />
      <span>Compare with an <span class="term">Interrail</span> pass</span>
    </label>
    {#if preferences.consider_passes}
      <div class="pass-settings">
        {#each travellers as traveller (traveller.id)}
          <label>
            <span class="label">{traveller.name} lives in</span>
            <select
              value={traveller.residence_country}
              disabled={onSaveTraveller === undefined}
              onchange={(event) => setResidence(traveller, event.currentTarget.value)}
            >
              {#each RESIDENCE_COUNTRIES as country (country.code)}
                <option value={country.code}>{country.name}</option>
              {/each}
            </select>
          </label>
        {/each}
        <label class="switch-row">
          <input
            type="checkbox"
            class="switch"
            checked={preferences.consider_families}
            {disabled}
            onchange={(event) => update({ consider_families: event.currentTarget.checked })}
          />
          <span>Children travel free with an adult on a pass</span>
        </label>
      </div>
    {/if}
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
    <label class="switch-row">
      <input
        type="checkbox"
        class="switch"
        checked={preferences.reserve_optional_seats}
        {disabled}
        onchange={(event) => update({ reserve_optional_seats: event.currentTarget.checked })}
      />
      <span>Reserve seats where a reservation is optional</span>
    </label>
    <label class="switch-row">
      <input
        type="checkbox"
        class="switch"
        checked={preferences.live_prices}
        {disabled}
        onchange={(event) => update({ live_prices: event.currentTarget.checked })}
      />
      <span>Look up live prices on booking sites</span>
      <span class="hint">
        unofficial: <span class="term">SBB</span>, <span class="term">DB</span>,
        <span class="term">ÖBB</span>, <span class="term">PKP Intercity</span>,
        <span class="term">Trenitalia</span> and <span class="term">NS</span>, and
        <span class="term">SBB</span> for France
      </span>
    </label>
    <label>
      <span class="label">Buying on</span>
      <input
        type="date"
        class="numbers"
        value={preferences.assumed_purchase_date ?? ""}
        {disabled}
        onchange={(event) => update({ assumed_purchase_date: event.currentTarget.value || null })}
      />
      <span class="hint">empty means today</span>
    </label>
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

  /* The pass settings sit indented under the switch that shows them. */
  .pass-settings {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding-left: calc(36px + 10px);
  }

  .label {
    min-width: 8.5em;
    color: var(--color-text-muted);
  }

  .hint {
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }
</style>
