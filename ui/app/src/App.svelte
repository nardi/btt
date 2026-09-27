<script lang="ts">
  // The desktop app: a list of trips, or the itinerary of the open trip.
  import { commands, type TripId, type TripSummary } from "@btt/shared/bindings";
  import ItineraryView from "@btt/shared/components/ItineraryView.svelte";
  import TripMap from "@btt/shared/components/TripMap.svelte";
  import type { PlaceFocusRequest } from "@btt/shared/map-features";
  import { onMount } from "svelte";

  import { commandErrorMessage, TauriTripSource } from "./tauri-trip-source.svelte";
  import TripList from "./TripList.svelte";

  let trips: TripSummary[] = $state([]);
  let openSource: TauriTripSource | null = $state(null);
  let errorMessage: string | null = $state(null);
  let selectedPlaceId: number | null = $state(null);
  let focusRequest: PlaceFocusRequest | null = $state(null);

  async function refreshTrips() {
    const result = await commands.listTrips();
    if (result.status === "ok") {
      trips = result.data;
    } else {
      errorMessage = commandErrorMessage(result.error);
    }
  }

  async function openTrip(tripId: TripId) {
    const opened = await TauriTripSource.open(tripId);
    if (opened instanceof TauriTripSource) {
      openSource = opened;
      errorMessage = null;
    } else {
      errorMessage = commandErrorMessage(opened);
    }
  }

  async function createTrip(name: string, startDate: string) {
    const result = await commands.createTrip(name, startDate);
    if (result.status === "ok") {
      await openTrip(result.data.id);
    } else {
      errorMessage = commandErrorMessage(result.error);
    }
  }

  async function deleteTrip(tripId: TripId) {
    const result = await commands.deleteTrip(tripId);
    if (result.status === "error") {
      errorMessage = commandErrorMessage(result.error);
    }
    await refreshTrips();
  }

  async function closeTrip() {
    await openSource?.close();
    openSource = null;
    selectedPlaceId = null;
    focusRequest = null;
    await refreshTrips();
  }

  async function renameTrip(name: string) {
    const trimmed = name.trim();
    if (openSource !== null && trimmed !== "" && trimmed !== openSource.trip.name) {
      await openSource.applyEdit({ RenameTrip: trimmed });
    }
  }

  /** Selects a place and asks the map to move to it. */
  function focusPlace(placeId: number) {
    selectedPlaceId = placeId;
    focusRequest = { placeId, requestNumber: (focusRequest?.requestNumber ?? 0) + 1 };
  }

  onMount(() => {
    void refreshTrips();
  });
</script>

<main class:trip-open={openSource !== null}>
  {#if errorMessage !== null}
    <p class="error" role="alert">{errorMessage}</p>
  {/if}

  {#if openSource === null}
    <TripList {trips} onCreate={createTrip} onOpen={openTrip} onDelete={deleteTrip} />
  {:else}
    <header class="trip-header">
      <button type="button" onclick={closeTrip}>← Trips</button>
      <input
        class="trip-name"
        type="text"
        value={openSource.trip.name}
        aria-label="Trip name"
        onchange={(event) => renameTrip(event.currentTarget.value)}
      />
      <label>
        Start
        <input
          type="date"
          value={openSource.trip.start_date}
          onchange={(event) => {
            if (event.currentTarget.value !== "") {
              void openSource?.applyEdit({ SetStartDate: event.currentTarget.value });
            }
          }}
        />
      </label>
    </header>
    <div class="workspace">
      <div class="itinerary-column">
        <ItineraryView
          source={openSource}
          {selectedPlaceId}
          onSelect={(placeId) => (selectedPlaceId = placeId)}
          onFocus={focusPlace}
        />
      </div>
      <div class="map-column">
        <TripMap
          source={openSource}
          {selectedPlaceId}
          {focusRequest}
          onSelect={(placeId) => (selectedPlaceId = placeId)}
        />
      </div>
    </div>
  {/if}
</main>

<style>
  main {
    max-width: 880px;
    margin: 0 auto;
    padding: var(--space-6) var(--space-4);
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    box-sizing: border-box;
  }

  main.trip-open {
    max-width: none;
    height: 100vh;
    padding: var(--space-4);
  }

  .trip-header {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }

  .trip-name {
    flex: 1;
    font-size: 1.3rem;
    font-weight: 600;
  }

  .workspace {
    display: grid;
    grid-template-columns: minmax(420px, 1fr) minmax(300px, 1.2fr);
    gap: var(--space-4);
    flex: 1;
    min-height: 0;
  }

  .itinerary-column {
    overflow-y: auto;
    padding-right: var(--space-2);
  }

  .map-column {
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    overflow: hidden;
  }

  .error {
    margin: 0;
    color: var(--color-danger);
  }
</style>
