<script lang="ts">
  // The setup screen: who travels, and the preferences that hold for the whole trip. Fields the
  // backend does not read stay hidden until they have an effect.
  import type { Traveller, TravellerId, TripEdit } from "../bindings";
  import { blankTraveller } from "../traveller";
  import { itineraryRows } from "../itinerary";
  import type { TripSource } from "../trip-source";
  import Icon from "./Icon.svelte";
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

  /** The travellers travelling on this trip, which is whose fields the pricing reads. */
  const travelling = $derived(
    travellers.filter((traveller) => trip.preferences.traveller_ids.includes(traveller.id)),
  );

  /** Whether the trip has a same-day stop, which opens the same-day disclosure by default. */
  const hasSameDayStop = $derived(
    itineraryRows(trip).places.some((row) => row.stay?.SameDay !== undefined),
  );

  /** Adds or removes a traveller from the trip. */
  function setTravelling(travellerId: TravellerId, travellingOnTrip: boolean) {
    const others = trip.preferences.traveller_ids.filter((id) => id !== travellerId);
    onEdit?.({
      SetTripPreferences: {
        ...trip.preferences,
        traveller_ids: travellingOnTrip ? [...others, travellerId] : others,
      },
    });
  }
</script>

<section class="setup">
  <div class="column">
    <section class="card">
      <h3>Travellers</h3>
      {#each travellers as traveller (traveller.id)}
        <!-- Every traveller is a block of the card, set apart from the next by a hairline. -->
        <div class="traveller">
          <div class="head">
            <label class="switch-row">
              <input
                type="checkbox"
                class="switch"
                checked={trip.preferences.traveller_ids.includes(traveller.id)}
                disabled={onEdit === undefined}
                onchange={(event) => setTravelling(traveller.id, event.currentTarget.checked)}
              />
              <span>Travelling on this trip</span>
            </label>
            <button
              type="button"
              class="icon-button danger-hover"
              aria-label="Remove traveller"
              title="Remove traveller"
              onclick={() => onDeleteTraveller(traveller.id)}
            >
              <Icon name="trash" />
            </button>
          </div>
          <TravellerForm {traveller} {today} onChange={onSaveTraveller} />
        </div>
      {/each}
      {#if travellers.length === 0}
        <p class="empty">Add the people who travel. Their age decides the ticket prices.</p>
      {/if}
      <button
        type="button"
        class="add with-icon"
        onclick={() => {
          const traveller = blankTraveller();
          onSaveTraveller(traveller);
          setTravelling(traveller.id, true);
        }}
      >
        <Icon name="plus" />
        Add a traveller
      </button>
    </section>
  </div>
  <div class="column">
    <PreferencesPanel preferences={trip.preferences} {hasSameDayStop} {onEdit} />
    <TicketPreferencesPanel
      preferences={trip.preferences.ticket_preferences}
      travellers={travelling}
      onSaveTraveller={onSaveTraveller}
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
    gap: 20px;
    overflow-y: auto;
    min-height: 0;
    align-items: start;
  }

  .column {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .traveller {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .traveller + .traveller {
    padding-top: 14px;
    border-top: 1px solid var(--color-border);
  }

  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }

  .add {
    align-self: flex-start;
  }

  .empty {
    margin: 0;
    color: var(--color-text-muted);
  }
</style>
