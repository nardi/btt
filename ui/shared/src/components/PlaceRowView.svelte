<script lang="ts">
  // One place of the itinerary. The closed card is one line with the name, the dates and the
  // stay. The open card adds the settings of the place.
  import type {
    PlaceStay,
    PlaceWindowOverride,
    TravelDayWindows,
    TripEdit,
    TripPlaceId,
  } from "../bindings";
  import { formatDate, formatDistance } from "../format";
  import {
    type LegLine,
    overriddenArrivalWindow,
    overriddenDepartureWindow,
    type PlaceRow,
    placeWindowOverride,
    stayLabel,
    stayNights,
  } from "../itinerary";
  import { expandTransition, rowEnter, rowExit } from "../motion";
  import Icon from "./Icon.svelte";
  import TimeWindowOverrideField from "./TimeWindowOverrideField.svelte";
  import { fly, slide } from "svelte/transition";

  let {
    row,
    index,
    lineAbove,
    lineBelow,
    selected,
    onSelect,
    onFocus,
    defaultWindows,
    onEdit,
    onReorder,
    draggedIndex = null,
    onDragChange,
    enterDelay,
  }: {
    row: PlaceRow;
    index: number;
    /** The leg that arrives here, whose colour draws the spine above the glyph. */
    lineAbove: LegLine | null;
    /** The leg that leaves here, whose colour draws the spine below the glyph. */
    lineBelow: LegLine | null;
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
    /** Staggers the enter transition of the rows of a freshly opened trip. */
    enterDelay: number;
  } = $props();

  /** Whether the settings of the place are open. */
  let expanded = $state(false);
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
  const disabled = $derived(onEdit === undefined);

  /** The dates of the place on its closed card, such as "Sat 11 Oct to Sun 12 Oct". */
  const dates = $derived.by(() => {
    const first = row.arrivalDate ?? row.departureDate;
    if (first === null) {
      return "";
    }
    if (row.arrivalDate !== null && row.departureDate !== null && row.arrivalDate !== row.departureDate) {
      return `${formatDate(row.arrivalDate)} to ${formatDate(row.departureDate)}`;
    }
    return formatDate(first);
  });

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
  in:fly={rowEnter(enterDelay)}
  out:fly={rowExit()}
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
  <div class="spine" aria-hidden="true">
    {#if lineAbove !== null}
      <div
        class="line above"
        class:night={lineAbove.night}
        style:--line-colour={lineAbove.colour}
      ></div>
    {/if}
    <div class="glyph" class:via={row.role === "via"}></div>
    {#if lineBelow !== null}
      <div
        class="line below"
        class:night={lineBelow.night}
        style:--line-colour={lineBelow.colour}
      ></div>
    {/if}
  </div>
  <div class="place-card">
    <div class="head">
      {#if onReorder !== undefined}
        <!-- The grip shows that the whole row can be dragged to another position. -->
        <span class="grip" title="Drag to move this place in the route" aria-hidden="true">
          <Icon name="grip" />
        </span>
      {/if}
      <span class="title">
        <span class="name">{row.place.name}</span>
        <span class="dates numbers">{dates}</span>
      </span>
      <span class="stay">{stayLabel(row.role, row.stay)}</span>
      <button
        type="button"
        class="icon-button"
        aria-label="Show on the map"
        title="Show on the map"
        onclick={() => {
          onSelect();
          onFocus();
        }}
      >
        <Icon name="pin" />
      </button>
      <button
        type="button"
        class="icon-button"
        aria-expanded={expanded}
        aria-controls={`place-settings-${placeId}`}
        aria-label={expanded ? "Hide settings" : "Show settings"}
        title={expanded ? "Hide settings" : "Show settings"}
        onclick={() => (expanded = !expanded)}
      >
        <Icon name="chevron" turned={expanded} />
      </button>
    </div>

    {#if expanded}
      <div class="settings" id={`place-settings-${placeId}`} transition:slide={expandTransition()}>
        <div class="fields">
          {#if row.stay !== null}
            <label class="field">
              <span class="label">Stay</span>
              <input
                class="numbers"
                type="number"
                min="0"
                max="255"
                value={stayNights(row.stay)}
                {disabled}
                onchange={(event) =>
                  onEdit?.({
                    SetPlaceStay: {
                      place_id: placeId,
                      stay: stayWithNights(event.currentTarget.valueAsNumber || 0),
                    },
                  })}
              />
              <span class="hint">nights</span>
            </label>
            {#if row.stay.SameDay !== undefined}
              <label class="switch-row">
                <input
                  type="checkbox"
                  class="switch"
                  checked={row.stay.SameDay.prefers_longer_stay}
                  {disabled}
                  onchange={(event) => setPrefersLongerStay(event.currentTarget.checked)}
                />
                <span>Longer is better</span>
              </label>
            {/if}
          {/if}
          <!-- The arrival comes first, as it does at the place. The origin has no arrival and the
               destination no departure. -->
          {#if row.role !== "origin"}
            <TimeWindowOverrideField
              label="Arrive here between"
              window={arrivalOverride}
              defaultWindow={defaultWindows.arrival}
              {disabled}
              onChange={(arrival) => setWindows(placeWindowOverride(arrival, departureOverride))}
            />
          {/if}
          {#if row.role !== "destination"}
            <TimeWindowOverrideField
              label="Leave here between"
              window={departureOverride}
              defaultWindow={defaultWindows.departure}
              {disabled}
              onChange={(departure) => setWindows(placeWindowOverride(arrivalOverride, departure))}
            />
          {/if}
          {#if row.place.window_override !== null && onEdit !== undefined}
            <button type="button" class="small" onclick={() => setWindows(null)}>Use trip defaults</button>
          {/if}
        </div>
        <div class="footer">
          <p class="radius">
            Stations within {formatDistance(row.place.station_search_radius)}.
            {#if onEdit !== undefined}Drag the ring on the map to change it.{/if}
          </p>
          {#if onEdit !== undefined}
            <button
              type="button"
              class="icon-button danger-hover"
              aria-label="Remove place"
              title="Remove place"
              onclick={() => onEdit?.({ RemovePlace: placeId })}
            >
              <Icon name="trash" />
            </button>
          {/if}
        </div>
      </div>
    {/if}
  </div>
</li>

<style>
  /* The row is a grid: the spine cell on the left, the place card 12px away from it. */
  .place-row {
    position: relative;
    display: grid;
    grid-template-columns: 34px 1fr;
    column-gap: var(--space-3);
    list-style: none;
  }

  .spine {
    position: relative;
  }

  .line {
    position: absolute;
    left: 50%;
    transform: translateX(-50%);
    width: 4px;
    border-radius: var(--radius-pill);
  }

  .line.above {
    top: 0;
    height: 26px;
    background: var(--line-colour);
  }

  .line.below {
    top: 26px;
    bottom: 0;
    background: var(--line-colour);
  }

  /* A night leg draws dashed, in the purple that marks night trains everywhere. */
  .line.night {
    background: repeating-linear-gradient(
      to bottom,
      var(--line-colour) 0 8px,
      transparent 8px 13px
    );
  }

  /* The place glyph: a circle for the ends, a square for via places. */
  .glyph {
    position: absolute;
    left: 50%;
    top: 16px;
    transform: translateX(-50%);
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--color-text);
    border: 3px solid var(--color-background);
    box-sizing: content-box;
  }

  .glyph.via {
    border-radius: 3px;
  }

  .place-card {
    margin-bottom: var(--space-3);
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    padding: var(--space-3) var(--space-4);
  }

  .place-row.selected .place-card {
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
    z-index: 1;
  }

  .place-row.drop-before::before {
    top: -1px;
  }

  .place-row.drop-after::after {
    bottom: -1px;
  }

  .head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .grip {
    display: flex;
    margin-left: calc(-1 * var(--space-2));
    color: var(--color-text-muted);
    opacity: 0.6;
    cursor: grab;
    transition: opacity 130ms ease;
  }

  .place-row:hover .grip {
    opacity: 1;
  }

  .place-row[draggable="true"] .head {
    cursor: grab;
  }

  .title {
    display: flex;
    flex: 1;
    align-items: baseline;
    gap: var(--space-3);
    min-width: 0;
  }

  .name {
    font-size: var(--text-lead);
    font-weight: 700;
  }

  .dates {
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }

  .stay {
    padding-right: var(--space-1);
    color: var(--color-text-muted);
    font-size: var(--text-caption);
    white-space: nowrap;
  }

  /* The settings sit under a hairline, apart from the line of the closed card. */
  .settings {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    margin-top: var(--space-3);
    padding-top: var(--space-3);
    border-top: 1px solid var(--color-border);
  }

  .fields {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }

  .field {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
  }

  .label {
    min-width: 9em;
    color: var(--color-text-muted);
  }

  .field input {
    width: 3em;
  }

  .hint {
    color: var(--color-text-muted);
  }

  .footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }

  .radius {
    margin: 0;
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }
</style>
