<script lang="ts">
  // One place of the itinerary with its dates and stay.
  import type {
    PlaceStay,
    PlaceWindowOverride,
    TravelDayWindows,
    TripEdit,
    TripPlaceId,
  } from "../bindings";
  import { formatDate, formatDistance } from "../format";
  import {
    overriddenArrivalWindow,
    overriddenDepartureWindow,
    type PlaceRow,
    placeWindowOverride,
    stayNights,
  } from "../itinerary";
  import TimeWindowOverrideField from "./TimeWindowOverrideField.svelte";

  let {
    row,
    index,
    selected,
    onSelect,
    onFocus,
    defaultWindows,
    onEdit,
    onReorder,
    draggedIndex = null,
    onDragChange,
  }: {
    row: PlaceRow;
    index: number;
    selected: boolean;
    onSelect: () => void;
    /** Moves the map to this place. */
    onFocus: () => void;
    defaultWindows: TravelDayWindows;
    /** Missing in a read-only view. */
    onEdit?: (edit: TripEdit) => void;
    /** Moves the dragged place to this row's index. Missing in a read-only view. */
    onReorder?: (draggedPlaceId: TripPlaceId, newIndex: number) => void;
    /** Position of the place being dragged in the list, if any. */
    draggedIndex?: number | null;
    /** Reports the start and end of dragging this row. */
    onDragChange?: (draggedIndex: number | null) => void;
  } = $props();

  let showWindows = $state(false);
  let dropTarget = $state(false);
  // A place dragged down lands after this row, and one dragged up lands before it.
  const dropPosition = $derived(
    !dropTarget || draggedIndex === null || draggedIndex === index
      ? null
      : draggedIndex < index
        ? "after"
        : "before",
  );
  const placeId = $derived(row.place.id);
  const arrivalOverride = $derived(overriddenArrivalWindow(row.place.window_override));
  const departureOverride = $derived(overriddenDepartureWindow(row.place.window_override));

  /** Turns a number of nights into a stay. Zero nights keeps existing same-day settings. */
  function stayWithNights(nights: number): PlaceStay {
    if (nights > 0) {
      return { Nights: Math.min(Math.floor(nights), 255) };
    }
    if (row.stay?.SameDay !== undefined) {
      return row.stay;
    }
    return { SameDay: { stay_range_override: null, prefers_longer_stay: false } };
  }

  function setPrefersLongerStay(prefersLongerStay: boolean) {
    const sameDay = row.stay?.SameDay;
    if (sameDay === undefined) {
      return;
    }
    onEdit?.({
      SetPlaceStay: {
        place_id: placeId,
        stay: { SameDay: { ...sameDay, prefers_longer_stay: prefersLongerStay } },
      },
    });
  }

  function setWindows(windowOverride: PlaceWindowOverride | null) {
    onEdit?.({ SetPlaceWindowOverride: { place_id: placeId, window_override: windowOverride } });
  }

  function dropPlace(event: DragEvent) {
    event.preventDefault();
    dropTarget = false;
    const draggedPlaceId = Number(event.dataTransfer?.getData("text/plain"));
    if (Number.isInteger(draggedPlaceId) && draggedPlaceId !== placeId) {
      onReorder?.(draggedPlaceId, index);
    }
  }
</script>

<li
  class="place-row"
  class:drop-before={dropPosition === "before"}
  class:drop-after={dropPosition === "after"}
  class:selected
  draggable={onReorder !== undefined}
  ondragstart={(event) => {
    event.dataTransfer?.setData("text/plain", String(placeId));
    onDragChange?.(index);
  }}
  ondragend={() => onDragChange?.(null)}
  ondragover={(event) => {
    if (onReorder !== undefined) {
      event.preventDefault();
      dropTarget = true;
    }
  }}
  ondragleave={() => (dropTarget = false)}
  ondrop={dropPlace}
>
  <div class="main">
    <span class="number" title={row.role}>{index + 1}</span>
    <button
      type="button"
      class="name-and-dates"
      title="Double-click to show on the map"
      onclick={onSelect}
      ondblclick={onFocus}
    >
      <span class="name">{row.place.name}</span>
      <span class="dates">
        {#if row.arrivalDate !== null}arrive {formatDate(row.arrivalDate)}{/if}
        {#if row.arrivalDate !== null && row.departureDate !== null}·{/if}
        {#if row.departureDate !== null}leave {formatDate(row.departureDate)}{/if}
        · stations within {formatDistance(row.place.station_search_radius)}
      </span>
    </button>

    {#if row.stay !== null}
      <label class="stay">
        <input
          type="number"
          min="0"
          max="255"
          value={stayNights(row.stay)}
          disabled={onEdit === undefined}
          onchange={(event) =>
            onEdit?.({
              SetPlaceStay: {
                place_id: placeId,
                stay: stayWithNights(event.currentTarget.valueAsNumber || 0),
              },
            })}
        />
        nights
      </label>
      {#if row.stay.SameDay !== undefined}
        <label>
          <input
            type="checkbox"
            checked={row.stay.SameDay.prefers_longer_stay}
            disabled={onEdit === undefined}
            onchange={(event) => setPrefersLongerStay(event.currentTarget.checked)}
          />
          longer is better
        </label>
      {/if}
    {/if}

    {#if onEdit !== undefined}
      <button type="button" onclick={() => (showWindows = !showWindows)} aria-expanded={showWindows}>
        Times
      </button>
      <button type="button" class="remove" onclick={() => onEdit?.({ RemovePlace: placeId })}>
        Remove
      </button>
    {/if}
  </div>

  {#if showWindows}
    <div class="windows">
      <!-- The arrival comes first, as it does at the place. The origin has no arrival and the
           destination no departure. -->
      {#if row.role !== "origin"}
        <TimeWindowOverrideField
          label="Arrive here between"
          window={arrivalOverride}
          defaultWindow={defaultWindows.arrival}
          onChange={(arrival) => setWindows(placeWindowOverride(arrival, departureOverride))}
        />
      {/if}
      {#if row.role !== "destination"}
        <TimeWindowOverrideField
          label="Leave here between"
          window={departureOverride}
          defaultWindow={defaultWindows.departure}
          onChange={(departure) => setWindows(placeWindowOverride(arrivalOverride, departure))}
        />
      {/if}
      {#if row.place.window_override !== null}
        <button type="button" onclick={() => setWindows(null)}>Use trip defaults</button>
      {/if}
    </div>
  {/if}
</li>

<style>
  .place-row {
    position: relative;
    list-style: none;
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    padding: var(--space-2) var(--space-3);
  }

  .place-row.selected {
    border-color: var(--color-accent);
    box-shadow: 0 0 0 1px var(--color-accent);
  }

  /* A bar on the edge of the row shows where the dragged place will land. It stays inside the
     row, because the scrolling list clips anything above the first row. */
  .place-row.drop-before::before,
  .place-row.drop-after::after {
    content: "";
    position: absolute;
    left: 0;
    right: 0;
    height: 4px;
    border-radius: 2px;
    background: var(--color-accent);
  }

  .place-row.drop-before::before {
    top: -1px;
  }

  .place-row.drop-after::after {
    bottom: -1px;
  }

  .main {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }

  .number {
    display: inline-grid;
    place-items: center;
    width: 1.8em;
    height: 1.8em;
    border-radius: 50%;
    background: var(--color-accent);
    color: var(--color-accent-contrast);
    font-weight: 600;
  }

  .name-and-dates {
    align-items: flex-start;
    padding: 0;
    border: none;
    background: none;
    text-align: left;
    display: flex;
    flex-direction: column;
    flex: 1;
  }

  .name {
    font-weight: 600;
  }

  .dates {
    color: var(--color-text-muted);
  }

  .stay input {
    width: 4em;
  }

  .remove {
    color: var(--color-danger);
  }

  .windows {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-2);
    margin-top: var(--space-2);
    padding-left: calc(1.8em + var(--space-3));
  }
</style>
