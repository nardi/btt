<script lang="ts" module>
  /** The side of a place on which a new place goes. */
  export type InsertionSide = "before" | "after";
</script>

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
  import type { Snippet } from "svelte";
  import { fly, slide } from "svelte/transition";

  let {
    row,
    index,
    placeCount,
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
    touchDropIndex = null,
    onTouchDropChange,
    onInsert,
    insertionSide = null,
    insertionSearch,
    enterDelay,
  }: {
    row: PlaceRow;
    index: number;
    /** The number of places in the itinerary, which decides whether the place can move down. */
    placeCount: number;
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
    /** The index of the row under the finger during a touch drag, or `null`. */
    touchDropIndex?: number | null;
    /** Reports the row under the finger during a touch drag of this row. */
    onTouchDropChange?: (dropIndex: number | null) => void;
    /** Opens the search for a new place on one side of this place, or closes it. */
    onInsert?: (side: InsertionSide) => void;
    /** The side of this place on which the open search adds a place, or `null`. */
    insertionSide?: InsertionSide | null;
    /** The search for the new place, which exists while the search is open. */
    insertionSearch?: Snippet;
    /** Staggers the enter transition of the rows of a freshly opened trip. */
    enterDelay: number;
  } = $props();

  /** Whether the settings of the place are open. */
  let expanded = $state(false);
  let dropTarget = $state(false);
  // A place dragged down lands after this row, and one dragged up lands before it.
  const dropPosition = $derived(
    !(dropTarget || touchDropIndex === index) || draggedIndex === null || draggedIndex === index
      ? null
      : draggedIndex < index
        ? "after"
        : "before",
  );
  const placeId = $derived(row.place.id);
  const arrivalOverride = $derived(overriddenArrivalWindow(row.place.window_override));
  const departureOverride = $derived(overriddenDepartureWindow(row.place.window_override));
  const disabled = $derived(onEdit === undefined);
  // A read-only place shows only its stay. The windows, the preference for a longer stay and the
  // search radius only steered the planner, so a shared trip does not keep them.
  const hasSettings = $derived(onEdit !== undefined || row.stay !== null);

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

  const insertionSides: InsertionSide[] = ["before", "after"];

  /**
   * Whether the main pointer is a finger. The browser then starts a drag only after a long press,
   * which also selects text, so the row is not draggable and the grip starts a touch drag.
   */
  const coarsePointer = typeof matchMedia === "function" && matchMedia("(pointer: coarse)").matches;

  /** Changes the name of the place. An empty name keeps the old one. */
  function renamePlace(field: HTMLInputElement) {
    const name = field.value.trim();
    if (name === "") {
      field.value = row.place.name;
    } else if (name !== row.place.name) {
      onEdit?.({ RenamePlace: { place_id: placeId, name } });
    }
  }

  /** The state of a touch drag of this row, which starts on the grip. See `coarsePointer`. */
  let touchDrag: {
    pointerId: number;
    startY: number;
    lastY: number;
    scroller: HTMLElement | null;
    startScrollTop: number;
    frame: number;
  } | null = $state(null);
  /** The distance in pixels by which the dragged row follows the finger. */
  let touchOffset = $state(0);
  let rowElement: HTMLLIElement | undefined = $state();

  /** The nearest ancestor of the row that scrolls, which scrolls when the finger is at its edge. */
  function scrollingAncestor(element: HTMLElement): HTMLElement | null {
    for (let parent = element.parentElement; parent !== null; parent = parent.parentElement) {
      const overflow = getComputedStyle(parent).overflowY;
      if ((overflow === "auto" || overflow === "scroll") && parent.scrollHeight > parent.clientHeight) {
        return parent;
      }
    }
    return document.scrollingElement as HTMLElement | null;
  }

  /** The index of the place row under a point, or `null` when the point is outside the list. */
  function placeIndexAt(clientX: number, clientY: number): number | null {
    const rowUnderPoint = document.elementFromPoint(clientX, clientY)?.closest(".place-row");
    const indexText = rowUnderPoint?.getAttribute("data-index");
    return indexText === undefined || indexText === null ? null : Number(indexText);
  }

  /**
   * Moves the dragged row to the finger and marks the row where the place will land. Between two
   * place rows, such as over a leg, the last marked row stays marked.
   */
  function followTouch(clientX: number) {
    if (touchDrag === null) {
      return;
    }
    const scrolled = (touchDrag.scroller?.scrollTop ?? 0) - touchDrag.startScrollTop;
    touchOffset = touchDrag.lastY - touchDrag.startY + scrolled;
    // The dragged row is under the finger, so the hit test skips it for one moment.
    if (rowElement !== undefined) {
      rowElement.style.pointerEvents = "none";
    }
    const dropIndex = placeIndexAt(clientX, touchDrag.lastY);
    if (rowElement !== undefined) {
      rowElement.style.pointerEvents = "";
    }
    if (dropIndex !== null) {
      onTouchDropChange?.(dropIndex);
    }
  }

  /** Scrolls the list while the finger is near its top or bottom edge. */
  function scrollNearEdge(clientX: number) {
    if (touchDrag === null || touchDrag.scroller === null) {
      return;
    }
    const edgeSize = 48;
    const bounds =
      touchDrag.scroller === document.scrollingElement
        ? { top: 0, bottom: innerHeight }
        : touchDrag.scroller.getBoundingClientRect();
    const step =
      touchDrag.lastY < bounds.top + edgeSize
        ? -8
        : touchDrag.lastY > bounds.bottom - edgeSize
          ? 8
          : 0;
    if (step !== 0) {
      touchDrag.scroller.scrollTop += step;
      followTouch(clientX);
    }
    touchDrag.frame = requestAnimationFrame(() => scrollNearEdge(clientX));
  }

  function startTouchDrag(event: PointerEvent) {
    if (event.pointerType === "mouse" || onReorder === undefined || rowElement === undefined) {
      return;
    }
    event.preventDefault();
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    const scroller = scrollingAncestor(rowElement);
    touchDrag = {
      pointerId: event.pointerId,
      startY: event.clientY,
      lastY: event.clientY,
      scroller,
      startScrollTop: scroller?.scrollTop ?? 0,
      frame: 0,
    };
    touchOffset = 0;
    onDragChange?.(index);
    onTouchDropChange?.(index);
    touchDrag.frame = requestAnimationFrame(() => scrollNearEdge(event.clientX));
  }

  function moveTouchDrag(event: PointerEvent) {
    if (touchDrag === null || event.pointerId !== touchDrag.pointerId) {
      return;
    }
    touchDrag.lastY = event.clientY;
    cancelAnimationFrame(touchDrag.frame);
    followTouch(event.clientX);
    touchDrag.frame = requestAnimationFrame(() => scrollNearEdge(event.clientX));
  }

  /** Ends a touch drag. The place moves only when the drag ends on another row. */
  function endTouchDrag(event: PointerEvent, dropped: boolean) {
    if (touchDrag === null || event.pointerId !== touchDrag.pointerId) {
      return;
    }
    cancelAnimationFrame(touchDrag.frame);
    const dropIndex = touchDropIndex;
    touchDrag = null;
    touchOffset = 0;
    onTouchDropChange?.(null);
    onDragChange?.(null);
    if (dropped && dropIndex !== null && dropIndex !== index) {
      onReorder?.(placeId, dropIndex);
    }
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
  bind:this={rowElement}
  class="place-row"
  class:touch-dragged={touchDrag !== null}
  style:transform={touchDrag === null ? undefined : `translateY(${touchOffset}px)`}
  data-index={index}
  class:drop-before={dropPosition === "before"}
  class:drop-after={dropPosition === "after"}
  class:selected
  draggable={onReorder !== undefined && !coarsePointer}
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
        <!-- The grip shows that the row can be dragged to another position. On a touch screen,
             only the grip starts a drag. -->
        <span
          class="grip"
          title="Drag to move this place in the route"
          aria-hidden="true"
          onpointerdown={startTouchDrag}
          onpointermove={moveTouchDrag}
          onpointerup={(event) => endTouchDrag(event, true)}
          onpointercancel={(event) => endTouchDrag(event, false)}
        >
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
      {#if hasSettings}
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
      {/if}
    </div>

    {#if expanded && hasSettings}
      <div class="settings" id={`place-settings-${placeId}`} transition:slide={expandTransition()}>
        <div class="fields">
          {#if onEdit !== undefined}
            <label class="field">
              <span class="label">Name</span>
              <input
                class="name-field"
                type="text"
                value={row.place.name}
                onchange={(event) => renamePlace(event.currentTarget)}
                onkeydown={(event) => {
                  if (event.key === "Enter") {
                    event.currentTarget.blur();
                  }
                }}
              />
            </label>
          {/if}
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
            {#if row.stay.SameDay !== undefined && onEdit !== undefined}
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
          {#if row.role !== "origin" && onEdit !== undefined}
            <TimeWindowOverrideField
              label="Arrive here between"
              window={arrivalOverride}
              defaultWindow={defaultWindows.arrival}
              {disabled}
              onChange={(arrival) => setWindows(placeWindowOverride(arrival, departureOverride))}
            />
          {/if}
          {#if row.role !== "destination" && onEdit !== undefined}
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
        {#if onInsert !== undefined}
          <div class="field">
            <span class="label">Add a place</span>
            <span class="insert-buttons">
              {#each insertionSides as side (side)}
                <button
                  type="button"
                  class="with-icon"
                  class:on={insertionSide === side}
                  aria-pressed={insertionSide === side}
                  onclick={() => onInsert?.(side)}
                >
                  <Icon name="plus" size={16} />
                  {side === "before" ? "Before" : "After"}
                </button>
              {/each}
            </span>
          </div>
          {@render insertionSearch?.()}
        {/if}
        {#if onEdit !== undefined}
          <div class="footer">
            <p class="radius">
              Stations within {formatDistance(row.place.station_search_radius)}. Drag the ring on the map to
              change it.
            </p>
            <!-- The buttons move the place by one position, which is easier than a drag on a
                 small touch screen. -->
            {#if onReorder !== undefined}
              <span class="move-buttons">
                <button
                  type="button"
                  class="icon-button"
                  aria-label="Move place up"
                  title="Move place up"
                  disabled={index === 0}
                  onclick={() => onReorder?.(placeId, index - 1)}
                >
                  <Icon name="chevron" turned />
                </button>
                <button
                  type="button"
                  class="icon-button"
                  aria-label="Move place down"
                  title="Move place down"
                  disabled={index === placeCount - 1}
                  onclick={() => onReorder?.(placeId, index + 1)}
                >
                  <Icon name="chevron" />
                </button>
              </span>
            {/if}
            <button
              type="button"
              class="icon-button danger-hover"
              aria-label="Remove place"
              title="Remove place"
              onclick={() => onEdit?.({ RemovePlace: placeId })}
            >
              <Icon name="trash" />
            </button>
          </div>
        {/if}
      </div>
    {/if}
  </div>
</li>

<style>
  /* The row is a grid: the spine cell on the left, the place card 12px away from it. */
  .place-row {
    position: relative;
    display: grid;
    grid-template-columns: 34px minmax(0, 1fr);
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

  /* The row that follows the finger during a touch drag is above the other rows. */
  .place-row.touch-dragged {
    z-index: 2;
  }

  .place-row.touch-dragged .place-card {
    box-shadow: 0 8px 24px rgb(0 0 0 / 0.18);
  }

  .title {
    display: flex;
    flex: 1;
    align-items: baseline;
    gap: var(--space-3);
    min-width: 0;
  }

  /* A narrow list, such as on a phone, has the dates under the name. */
  @container itinerary (max-width: 480px) {
    .title {
      flex-direction: column;
      align-items: flex-start;
      gap: 0;
    }
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

  .field input.name-field {
    flex: 1;
    width: auto;
    min-width: 8em;
  }

  .insert-buttons {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }

  /* A button takes the accent colour while its search for a new place is open. */
  .insert-buttons button.on {
    color: var(--color-accent);
    border-color: var(--color-accent);
    background: color-mix(in srgb, var(--color-accent) 12%, var(--color-surface));
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

  .move-buttons {
    display: none;
    gap: var(--space-2);
    margin-left: auto;
  }

  /* A touch screen. The grip is the handle of a touch drag, so it is larger and the browser
     does not scroll or select text when a finger moves on it. The other words on the closed card
     are not selected by a long press either. */
  @media (pointer: coarse) {
    .move-buttons {
      display: flex;
    }

    .grip {
      align-self: stretch;
      align-items: center;
      padding: 0 4px 0 var(--space-2);
      margin-left: calc(-1 * var(--space-4));
      margin-block: calc(-1 * var(--space-3));
      opacity: 1;
      touch-action: none;
    }

    .head {
      -webkit-user-select: none;
      user-select: none;
      -webkit-touch-callout: none;
    }
  }
</style>
