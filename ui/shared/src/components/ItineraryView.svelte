<script lang="ts">
  // The itinerary of one trip: its places, the legs between them and the trip preferences.
  import type {
    PlaceSearchResult,
    TripEdit,
    TripEditNotice,
    TripPlaceId,
    TripPreferences,
  } from "../bindings";
  import { formatMinutesOfDay } from "../format";
  import { itineraryRows, legLine, plannedArrivalDate, storedJourneyChoice, tripPlaces } from "../itinerary";
  import { staggerDelay } from "../motion";
  import { legViewState, type TripSource } from "../trip-source";
  import LegRowView from "./LegRowView.svelte";
  import PlaceRowView from "./PlaceRowView.svelte";
  import PlaceSearch from "./PlaceSearch.svelte";

  /** Search radius of places added through search, in meters. */
  const defaultStationSearchRadius = 5000;

  let {
    source,
    selectedPlaceId,
    onSelect,
    onFocus,
    onOpenSetup,
  }: {
    source: TripSource;
    selectedPlaceId: TripPlaceId | null;
    onSelect: (placeId: TripPlaceId | null) => void;
    /** Moves the map to a place. */
    onFocus: (placeId: TripPlaceId) => void;
    /** Shows the setup screen. */
    onOpenSetup: () => void;
  } = $props();

  const viewState = $derived(source.viewState);
  const trip = $derived(viewState.trip);
  // The journeys of the picked pass scenario decide the dates, like they decide the legs.
  const plannedArrivalDates = $derived(
    new Map(
      [...viewState.legStatesByArrivalPlace.keys()].flatMap((placeId) => {
        const legState = legViewState(viewState, placeId);
        return legState.status === "found" ? [[placeId, plannedArrivalDate(legState.legPlan)] as const] : [];
      }),
    ),
  );
  const rows = $derived(itineraryRows(trip, plannedArrivalDates));
  const editable = $derived(source.kind === "editable" ? source : null);
  const onEdit = $derived(
    editable === null ? undefined : (edit: TripEdit) => void editable.applyEdit(edit),
  );

  function placeName(placeId: TripPlaceId): string {
    return tripPlaces(trip.itinerary).find((place) => place.id === placeId)?.name ?? "a place";
  }

  function noticeMessage(notice: TripEditNotice): string {
    const { departure_place_id, arrival_place_id } = notice.OvernightOptionDropped;
    return (
      `The night train from ${placeName(departure_place_id)} to ${placeName(arrival_place_id)} ` +
      "was removed, because those places are no longer next to each other."
    );
  }

  function addPlace(result: PlaceSearchResult) {
    onEdit?.({
      AddPlace: {
        place: {
          name: result.name,
          coordinates: result.coordinates,
          station_search_radius: defaultStationSearchRadius,
        },
        insert_at: { AtIndex: rows.places.length },
      },
    });
  }

  /** Describes the default time windows of the trip in one line. */
  function preferencesSummary(preferences: TripPreferences): string {
    const { departure, arrival } = preferences.default_windows;
    return (
      "Using the default settings of departure between " +
      `${formatMinutesOfDay(departure.start)} and ${formatMinutesOfDay(departure.start + departure.length)}, ` +
      `arrival between ${formatMinutesOfDay(arrival.start)} and ${formatMinutesOfDay(arrival.start + arrival.length)}.`
    );
  }

  /** Position of the place being dragged, so rows can show where it would land. */
  let draggedIndex: number | null = $state(null);

  /** The places the view has shown, so a single added place enters without a stagger. */
  let knownPlaceIds = new Set<TripPlaceId>();

  /** Whether every place is new to the view, such as when a trip opens, which staggers the rows. */
  const staggered = $derived(rows.places.every((row) => !knownPlaceIds.has(row.place.id)));

  $effect(() => {
    knownPlaceIds = new Set(rows.places.map((row) => row.place.id));
  });

  function reorderPlace(draggedPlaceId: TripPlaceId, newIndex: number) {
    onEdit?.({ ReorderPlace: { place_id: draggedPlaceId, new_index: newIndex } });
  }
</script>

<section class="itinerary">
  {#if viewState.errorMessage !== null}
    <p class="error" role="alert">{viewState.errorMessage}</p>
  {/if}
  {#each viewState.notices as notice, noticeIndex (noticeIndex)}
    <p class="notice" role="status">{noticeMessage(notice)}</p>
  {/each}

  <ol class="rows">
    {#each rows.places as row, index (row.place.id)}
      {#if index > 0}
        {@const leg = rows.legs[index - 1]!}
        <LegRowView
          {leg}
          legState={legViewState(viewState, leg.arrivalPlace.id)}
          legLine={legLine(leg.travelMode, index)}
          nearbyDay={viewState.nearbyDaysByArrivalPlace.get(leg.arrivalPlace.id) ?? null}
          storedChoice={storedJourneyChoice(trip, leg)}
          livePrices={viewState.livePricesByJourney}
          {onEdit}
          enterDelay={staggered ? staggerDelay(2 * index - 1) : 0}
        />
      {/if}
      <PlaceRowView
        {row}
        {index}
        lineAbove={index > 0 ? legLine(rows.legs[index - 1]!.travelMode, index) : null}
        lineBelow={index < rows.legs.length ? legLine(rows.legs[index]!.travelMode, index + 1) : null}
        selected={row.place.id === selectedPlaceId}
        onSelect={() => onSelect(row.place.id)}
        onFocus={() => onFocus(row.place.id)}
        defaultWindows={trip.preferences.default_windows}
        {onEdit}
        onReorder={editable === null ? undefined : reorderPlace}
        {draggedIndex}
        onDragChange={(newDraggedIndex) => (draggedIndex = newDraggedIndex)}
        enterDelay={staggered ? staggerDelay(2 * index) : 0}
      />
    {/each}
  </ol>

  {#if rows.places.length === 0}
    <p class="empty">
      Search for the place where the trip starts, or double-click it on the map.
    </p>
  {:else if rows.places.length === 1}
    <p class="empty">
      Add the next place to plan the first leg. Double-click a place to show it on the map.
    </p>
  {/if}

  {#if editable !== null}
    <PlaceSearch searchPlaces={(query) => editable.searchPlaces(query)} onPick={addPlace} />
  {/if}

  <p class="preferences-summary">
    {preferencesSummary(trip.preferences)}
    <button type="button" class="small" onclick={onOpenSetup}>Change in Setup</button>
  </p>
</section>

<style>
  .itinerary {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }

  /* The rows stack with no gap between them, so the spine line stays continuous. The cards
     inside the rows carry their own margin. */
  .rows {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  /* The summary of the default windows and the button to the setup screen share a line. */
  .preferences-summary {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-2);
    margin: 0;
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }

  .error,
  .notice {
    margin: 0;
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius);
  }

  .error {
    color: var(--color-danger);
    border: 1px solid var(--color-danger);
  }

  .notice {
    background: var(--color-notice-fill);
    color: var(--color-notice);
    font-size: var(--text-small);
  }

  .empty {
    color: var(--color-text-muted);
    margin: 0;
  }
</style>
