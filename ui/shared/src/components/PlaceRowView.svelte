<script lang="ts">
  // One place of the itinerary with its dates and stay.
  import type { PlaceStay, TravelDayWindows, TripEdit, TripPlaceId } from "../bindings";
  import { formatDate } from "../format";
  import { type PlaceRow, stayNights } from "../itinerary";
  import TimeWindowField from "./TimeWindowField.svelte";

  let {
    row,
    index,
    defaultWindows,
    onEdit,
    onReorder,
  }: {
    row: PlaceRow;
    index: number;
    defaultWindows: TravelDayWindows;
    /** Missing in a read-only view. */
    onEdit?: (edit: TripEdit) => void;
    /** Moves the dragged place to this row's index. Missing in a read-only view. */
    onReorder?: (draggedPlaceId: TripPlaceId, newIndex: number) => void;
  } = $props();

  let showWindows = $state(false);
  let dropTarget = $state(false);
  const placeId = $derived(row.place.id);
  const windows = $derived(row.place.window_override ?? defaultWindows);

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

  function setWindows(windowOverride: TravelDayWindows | null) {
    onEdit?.({ SetPlaceWindowOverride: { place_id: placeId, windows: windowOverride } });
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
  class:drop-target={dropTarget}
  draggable={onReorder !== undefined}
  ondragstart={(event) => event.dataTransfer?.setData("text/plain", String(placeId))}
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
    <div class="name-and-dates">
      <span class="name">{row.place.name}</span>
      <span class="dates">
        {#if row.arrivalDate !== null}arrive {formatDate(row.arrivalDate)}{/if}
        {#if row.arrivalDate !== null && row.departureDate !== null}·{/if}
        {#if row.departureDate !== null}leave {formatDate(row.departureDate)}{/if}
      </span>
    </div>

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
      <TimeWindowField
        label="Depart between"
        window={windows.departure}
        onChange={(departure) => setWindows({ ...windows, departure })}
      />
      <TimeWindowField
        label="Arrive between"
        window={windows.arrival}
        onChange={(arrival) => setWindows({ ...windows, arrival })}
      />
      {#if row.place.window_override !== null}
        <button type="button" onclick={() => setWindows(null)}>Use trip defaults</button>
      {:else}
        <span class="hint">These are the trip's default times.</span>
      {/if}
    </div>
  {/if}
</li>

<style>
  .place-row {
    list-style: none;
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    padding: var(--space-2) var(--space-3);
  }

  .place-row.drop-target {
    border-color: var(--color-accent);
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
    display: flex;
    flex-direction: column;
    flex: 1;
  }

  .name {
    font-weight: 600;
  }

  .dates,
  .hint {
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
