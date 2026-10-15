<script lang="ts">
  // A shared trip: the itinerary and the map. A narrow screen shows them as two tabs, and a wide
  // screen shows them side by side.
  import type { TripSnapshot } from "@btt/shared/bindings";
  import ItineraryView from "@btt/shared/components/ItineraryView.svelte";
  import TripMap from "@btt/shared/components/TripMap.svelte";
  import type { PlaceFocusRequest } from "@btt/shared/map-features";
  import { onMount } from "svelte";

  import { loadRailGraph, type RailGraph } from "./rail/rail-graph";
  import { withRidePaths } from "./rail/ride-paths";
  import { snapshotTripSource } from "./viewer-trip-source";

  let { snapshot }: { snapshot: TripSnapshot } = $props();

  /** Width in pixels below which the itinerary and the map take turns. */
  const narrowScreenWidth = 760;

  type Tab = "itinerary" | "map";

  /** The graph of the rails, which the map needs to draw the rides along the tracks. */
  let railGraph: RailGraph | null = $state.raw(null);
  /**
   * Names the trip for the map, which moves to the places of a trip once. A snapshot has no ID, so
   * every trip that the page opens gets its own, and the graph arriving does not change it.
   */
  const tripId = $derived.by(() => {
    void snapshot;
    return crypto.randomUUID();
  });
  const source = $derived(
    snapshotTripSource(railGraph === null ? snapshot : withRidePaths(snapshot, railGraph), tripId),
  );
  let tab: Tab = $state("itinerary");
  const narrowQuery = `(max-width: ${narrowScreenWidth}px)`;
  // The first render already knows the width, so a phone never mounts the map next to the list
  // and tears it down again.
  let narrow = $state(typeof matchMedia === "function" && matchMedia(narrowQuery).matches);
  let selectedPlaceId: number | null = $state(null);
  let focusRequest: PlaceFocusRequest | null = $state(null);

  onMount(() => {
    const query = matchMedia(narrowQuery);
    const update = (event: MediaQueryListEvent) => (narrow = event.matches);
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  });

  // The graph loads when the map first shows, so a phone that only reads the itinerary never
  // fetches it. Without the graph, or when it fails to load, the map draws straight lines.
  $effect(() => {
    if ((!narrow || tab === "map") && railGraph === null) {
      loadRailGraph()
        .then((graph) => (railGraph = graph))
        .catch(() => undefined);
    }
  });

  // A new trip opens on the itinerary with nothing selected.
  $effect(() => {
    void snapshot;
    tab = "itinerary";
    selectedPlaceId = null;
    focusRequest = null;
  });

  /** Selects a place and asks the map to move to it. A narrow screen switches to the map. */
  function focusPlace(placeId: number) {
    selectedPlaceId = placeId;
    focusRequest = { placeId, requestNumber: (focusRequest?.requestNumber ?? 0) + 1 };
    if (narrow) {
      tab = "map";
    }
  }
</script>

<div class="trip-page" class:narrow>
  {#if narrow}
    <nav class="tabs" aria-label="Views of the trip">
      {#each [["itinerary", "Itinerary"], ["map", "Map"]] as const as [name, label] (name)}
        <button
          type="button"
          class:active={tab === name}
          aria-current={tab === name ? "page" : undefined}
          onclick={() => (tab = name)}
        >
          {label}
        </button>
      {/each}
    </nav>
  {/if}
  <div class="workspace">
    {#if !narrow || tab === "itinerary"}
      <div class="itinerary-column">
        <ItineraryView
          {source}
          {selectedPlaceId}
          onSelect={(placeId) => (selectedPlaceId = placeId)}
          onFocus={focusPlace}
        />
      </div>
    {/if}
    {#if !narrow || tab === "map"}
      <div class="map-column">
        <TripMap
          {source}
          {selectedPlaceId}
          {focusRequest}
          onSelect={(placeId) => (selectedPlaceId = placeId)}
        />
      </div>
    {/if}
  </div>
</div>

<style>
  .trip-page {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    gap: var(--space-3);
  }

  .tabs {
    display: flex;
    gap: var(--space-1);
    border-bottom: 1px solid var(--color-border);
  }

  .tabs button {
    flex: 1;
    border: none;
    border-bottom: 2px solid transparent;
    border-radius: 0;
    background: none;
    padding: var(--space-2) var(--space-3) calc(var(--space-2) + 2px);
    color: var(--color-text-muted);
    font-weight: 500;
  }

  .tabs button:hover {
    background: none;
    color: var(--color-text);
  }

  .tabs button.active {
    border-bottom-color: var(--color-accent);
    color: var(--color-text);
    font-weight: 600;
  }

  .workspace {
    display: grid;
    grid-template-columns: minmax(380px, 1fr) minmax(300px, 1.2fr);
    gap: var(--space-4);
    flex: 1;
    min-height: 0;
  }

  .narrow .workspace {
    grid-template-columns: minmax(0, 1fr);
  }

  .itinerary-column {
    overflow-y: auto;
    padding-right: var(--space-2);
  }

  .narrow .itinerary-column {
    padding-right: 0;
  }

  .map-column {
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    overflow: hidden;
    min-height: 0;
  }
</style>
