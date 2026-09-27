<script lang="ts">
  // Lists stored trips and creates new ones.
  import type { TripId, TripSummary } from "@btt/shared/bindings";
  import { formatDate } from "@btt/shared/format";

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
    <button type="submit" class="primary">Create trip</button>
  </form>

  {#if trips.length === 0}
    <p class="empty">No trips yet.</p>
  {:else}
    <ul>
      {#each trips as trip (trip.trip_id)}
        <li>
          <button type="button" class="open" onclick={() => onOpen(trip.trip_id)}>
            <span class="name">{trip.name}</span>
            <span class="details">
              from {formatDate(trip.start_date)} · {trip.place_count}
              {trip.place_count === 1 ? "place" : "places"}
            </span>
          </button>
          <button type="button" class="delete" onclick={() => onDelete(trip.trip_id)}>Delete</button>
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
    font-size: 1.4rem;
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
    gap: var(--space-2);
  }

  .open {
    display: flex;
    flex: 1;
    flex-direction: column;
    align-items: flex-start;
    padding: var(--space-2) var(--space-3);
  }

  .name {
    font-weight: 600;
  }

  .details,
  .empty {
    color: var(--color-text-muted);
  }

  .delete {
    color: var(--color-danger);
  }
</style>
