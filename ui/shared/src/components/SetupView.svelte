<script lang="ts">
  // The setup screen: who travels, and the preferences that hold for the whole trip.
  import type { Traveller, TravellerId, TripEdit } from "../bindings";
  import { blankTraveller } from "../traveller";
  import type { TripSource } from "../trip-source";
  import PreferencesPanel from "./PreferencesPanel.svelte";
  import TicketPreferencesPanel from "./TicketPreferencesPanel.svelte";
  import TravellerForm from "./TravellerForm.svelte";

  let {
    source,
    travellers,
    onSaveTraveller,
    onDeleteTraveller,
  }: {
    source: TripSource;
    /** Every saved traveller profile. */
    travellers: readonly Traveller[];
    onSaveTraveller: (traveller: Traveller) => void;
    onDeleteTraveller: (travellerId: TravellerId) => void;
  } = $props();

  const trip = $derived(source.viewState.trip);
  const editable = $derived(source.kind === "editable" ? source : null);
  const onEdit = $derived(
    editable === null ? undefined : (edit: TripEdit) => void editable.applyEdit(edit),
  );
  const today = new Date().toISOString().slice(0, 10);

  /** Adds or removes a traveller from the trip. */
  function setTravelling(travellerId: TravellerId, travelling: boolean) {
    const others = trip.preferences.traveller_ids.filter((id) => id !== travellerId);
    onEdit?.({
      SetTripPreferences: {
        ...trip.preferences,
        traveller_ids: travelling ? [...others, travellerId] : others,
      },
    });
  }
</script>

<section class="setup">
  <div class="column">
    <h2>Travellers</h2>
    {#each travellers as traveller (traveller.id)}
      <label class="travelling">
        <input
          type="checkbox"
          checked={trip.preferences.traveller_ids.includes(traveller.id)}
          disabled={onEdit === undefined}
          onchange={(event) => setTravelling(traveller.id, event.currentTarget.checked)}
        />
        Travelling on this trip
      </label>
      <TravellerForm
        {traveller}
        {today}
        onChange={onSaveTraveller}
        onDelete={() => onDeleteTraveller(traveller.id)}
      />
    {/each}
    {#if travellers.length === 0}
      <p class="empty">Add the people who travel. Their age and cards decide the ticket prices.</p>
    {/if}
    <button
      type="button"
      onclick={() => {
        const traveller = blankTraveller();
        onSaveTraveller(traveller);
        setTravelling(traveller.id, true);
      }}
    >
      Add a traveller
    </button>
  </div>
  <div class="column">
    <h2>Trip defaults</h2>
    <PreferencesPanel preferences={trip.preferences} {onEdit} />
    <TicketPreferencesPanel
      preferences={trip.preferences.ticket_preferences}
      onChange={onEdit === undefined
        ? undefined
        : (ticket_preferences) =>
            onEdit({ SetTripPreferences: { ...trip.preferences, ticket_preferences } })}
    />
  </div>
</section>

<style>
  .setup {
    display: grid;
    grid-template-columns: minmax(320px, 1fr) minmax(320px, 1fr);
    gap: var(--space-6);
    overflow-y: auto;
    min-height: 0;
    align-items: start;
  }

  .column {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }

  h2 {
    margin: 0;
  }

  .travelling {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .empty {
    margin: 0;
    color: var(--color-text-muted);
  }
</style>
