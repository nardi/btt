<script lang="ts">
  // Lists stored trips and creates new ones.
  import type { TripId, TripSummary } from "@btt/shared/bindings";
  import { formatDate } from "@btt/shared/format";
  import Icon from "@btt/shared/components/Icon.svelte";
  import TripRouteMap from "@btt/shared/components/TripRouteMap.svelte";
  import ThemeSwitch from "@btt/shared/components/ThemeSwitch.svelte";
  import Logo from "@btt/shared/components/Logo.svelte";

  let {
    trips,
    firstDate,
    lastDate,
    onCreate,
    onOpen,
    onDelete,
    onImport,
    onAbout,
  }: {
    trips: TripSummary[];
    /** The first day the timetable covers as `YYYY-MM-DD`, or `null` when the app has none. */
    firstDate: string | null;
    /** The last day the timetable covers as `YYYY-MM-DD`, or `null` when the app has none. */
    lastDate: string | null;
    onCreate: (name: string, startDate: string) => void;
    onOpen: (tripId: TripId) => void;
    onDelete: (tripId: TripId) => void;
    /** Stores the text of a `.btt` file as a new trip. */
    onImport: (contents: string) => void;
    /** Opens the about screen. */
    onAbout: () => void;
  } = $props();

  /** Returns the date two weeks from today, a typical planning horizon. */
  function dateInTwoWeeks(): string {
    const date = new Date();
    date.setDate(date.getDate() + 14);
    return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
  }

  /** Moves a date into the days the timetable covers. ISO dates compare in date order. */
  function clampToTimetable(date: string): string {
    if (lastDate !== null && date > lastDate) {
      return lastDate;
    }
    if (firstDate !== null && date < firstDate) {
      return firstDate;
    }
    return date;
  }

  let newTripName = $state("");
  let newTripStartDate = $state(dateInTwoWeeks());
  // The timetable can arrive after this list shows, so the date follows the days it covers.
  $effect(() => {
    newTripStartDate = clampToTimetable(newTripStartDate);
  });

  /** The hidden file field that the import button opens. */
  let fileInput: HTMLInputElement | undefined = $state();

  /** Reads the picked trip file and empties the field, so that the same file can be picked again. */
  async function importPickedFile(input: HTMLInputElement) {
    const file = input.files?.[0];
    input.value = "";
    if (file !== undefined) {
      onImport(await file.text());
    }
  }

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
  <div class="title-row">
    <Logo height={48} />
    <div class="title-actions">
      <button type="button" title="Open a .btt trip file as a new trip" onclick={() => fileInput?.click()}>
        Import trip…
      </button>
      <input
        bind:this={fileInput}
        type="file"
        accept=".btt,application/json"
        hidden
        onchange={(event) => void importPickedFile(event.currentTarget)}
      />
      <button type="button" onclick={onAbout}>About</button>
      <ThemeSwitch />
    </div>
  </div>
  <form class="new-trip" onsubmit={create}>
    <input type="text" placeholder="Name of a new trip" bind:value={newTripName} required />
    <input
      type="date"
      bind:value={newTripStartDate}
      min={firstDate}
      max={lastDate}
      required
    />
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
          <button type="button" class="open-button" onclick={() => onOpen(trip.trip_id)}>Open</button>
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

  .title-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .title-actions {
    display: flex;
    align-items: center;
    gap: var(--space-3);
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

  .name {
    overflow-wrap: anywhere;
  }

  .details,
  .empty {
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }

  /* A phone. The row of a trip opens the trip, so the open button goes. */
  @media (max-width: 719px) {
    .title-row {
      flex-wrap: wrap;
      gap: var(--space-3);
    }

    .title-actions {
      gap: var(--space-2);
    }

    .new-trip {
      flex-wrap: wrap;
    }

    .new-trip input[type="text"] {
      flex-basis: 100%;
    }

    .new-trip input[type="date"] {
      flex: 1;
      min-width: 0;
    }

    li {
      padding: var(--space-2) var(--space-3);
    }

    .open-button {
      display: none;
    }
  }
</style>
