<script lang="ts">
  // Lists stored trips and creates new ones.
  import type { TripId, TripSummary } from "@btt/shared/bindings";
  import { formatDate } from "@btt/shared/format";
  import Icon from "@btt/shared/components/Icon.svelte";
  import TripRouteMap from "@btt/shared/components/TripRouteMap.svelte";

  let {
    trips,
    onCreate,
    onOpen,
    onDelete,
  }: {
    trips: TripSummary[];
    onCreate: (name: string, startDate: string) => void;
    onOpen: (tripId: TripId) => void;
    onDelete: (tripId: TripId) => void;
  } = $props();

  /** Returns the date two weeks from today, a typical planning horizon. */
  function dateInTwoWeeks(): string {
    const date = new Date();
    date.setDate(date.getDate() + 14);
    return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
  }

  let newTripName = $state("");
  let newTripStartDate = $state(dateInTwoWeeks());

  function create(event: SubmitEvent) {
    event.preventDefault();
    const name = newTripName.trim();
    if (name !== "" && newTripStartDate !== "") {
      onCreate(name, newTripStartDate);
      newTripName = "";
    }
  }
</script>

<section class="trip-list">
  <h1>Trips</h1>
  <form class="new-trip" onsubmit={create}>
    <input type="text" placeholder="Name of a new trip" bind:value={newTripName} required />
    <input type="date" bind:value={newTripStartDate} required />
    <button type="submit" class="primary with-icon">
      <Icon name="plus" />
      Create trip
    </button>
  </form>

  {#if trips.length === 0}
    <p class="empty">No trips yet.</p>
  {:else}
    <ul>
      {#each trips as trip (trip.trip_id)}
        <li>
          <!-- The map and the name open the trip too, as the larger target. -->
          <button type="button" class="open-area" onclick={() => onOpen(trip.trip_id)}>
            <TripRouteMap route={trip.route} />
            <span class="text">
              <span class="name">{trip.name}</span>
              <span class="details numbers">
                from {formatDate(trip.start_date)} · {trip.place_count}
                {trip.place_count === 1 ? "place" : "places"}
              </span>
            </span>
          </button>
          <button type="button" onclick={() => onOpen(trip.trip_id)}>Open</button>
          <button
            type="button"
            class="icon-button danger-hover"
            aria-label="Delete trip"
            title="Delete trip"
            onclick={() => onDelete(trip.trip_id)}
          >
            <Icon name="trash" />
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .trip-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  h1 {
    margin: 0;
    font-size: 1.75rem;
    font-weight: 700;
  }

  .new-trip {
    display: flex;
    gap: var(--space-2);
  }

  .new-trip input[type="text"] {
    flex: 1;
  }

  ul {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
  }

  .open-area {
    display: flex;
    flex: 1;
    align-items: center;
    gap: var(--space-3);
    min-width: 0;
    padding: 0;
    border: none;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }

  .open-area:hover {
    background: none;
  }

  .open-area:hover .name {
    color: var(--color-accent);
  }

  .text {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: var(--space-1);
  }

  .name {
    font-weight: 700;
    font-size: var(--text-lead);
  }

  .details,
  .empty {
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }
</style>
