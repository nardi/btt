<script lang="ts">
  // The Alternatives button of a leg and the popup it opens. The popup uses the `popover`
  // attribute, so it sits in the top layer and the scrolling itinerary cannot clip it, and
  // Escape or a click outside closes it.
  import type { EuroRange, Journey } from "../bindings";
  import { type AlternativeRow, describeDifference } from "../alternatives";
  import { formatDate, formatDuration } from "../format";
  import { journeyOverview, transferSummary } from "../itinerary";
  import { journeyKey } from "../live-ranking";
  import { describeJourneyPrice, formatJourneyPrice } from "../tickets";
  import Icon from "./Icon.svelte";

  let {
    rows,
    chosen,
    kindsTooltip,
    livePrices,
    onTake,
  }: {
    /** The alternatives and the nearby day of the leg. */
    rows: readonly AlternativeRow[];
    /** The journey of the leg, which every row is compared with. */
    chosen: Journey;
    /** What the alternatives include, such as a much cheaper journey, or `null` for nothing special. */
    kindsTooltip: string | null;
    /** Live prices of journeys from the booking sites, by `journeyKey`. */
    livePrices: ReadonlyMap<string, EuroRange>;
    /** Takes a journey of the leg. Missing in a read-only view. */
    onTake?: (journey: Journey) => void;
  } = $props();

  const id = $props.id();

  /** Space between the button and the popup, which the caret of the popup crosses. */
  const gap = 12;

  let button: HTMLButtonElement | undefined = $state();
  let popup: HTMLDivElement | undefined = $state();
  let open = $state(false);
  /** Whether the popup sits below the button or above it. */
  let side: "below" | "above" = $state("below");
  /** The distance of the caret from the left edge of the popup, in pixels. */
  let caretLeft = $state(24);

  /**
   * Places the popup under its button, or above it when the space below is too small. Its edges
   * follow the content box of the card that holds the button, and the caret points at the
   * middle of the button.
   */
  function place() {
    if (button === undefined || popup === undefined) {
      return;
    }
    const anchor = button.getBoundingClientRect();
    const card = button.closest<HTMLElement>("[data-popup-edge]") ?? button;
    const cardBox = card.getBoundingClientRect();
    const style = getComputedStyle(card);
    const left = cardBox.left + (parseFloat(style.paddingLeft) || 0);
    const width = cardBox.width - (parseFloat(style.paddingLeft) || 0) - (parseFloat(style.paddingRight) || 0);
    popup.style.width = `${width}px`;
    const height = popup.getBoundingClientRect().height;
    const fitsBelow = anchor.bottom + gap + height <= innerHeight - 8;
    side = fitsBelow ? "below" : "above";
    popup.style.left = `${left}px`;
    popup.style.top = `${fitsBelow ? anchor.bottom + gap : Math.max(8, anchor.top - gap - height)}px`;
    caretLeft = Math.max(16, Math.min(anchor.left + anchor.width / 2 - left - 6, width - 28));
  }

  // The popup drifts away from its button when the page moves, so it closes then.
  $effect(() => {
    if (!open) {
      return;
    }
    const close = (event: Event) => {
      if (event.target instanceof Node && popup?.contains(event.target)) {
        return;
      }
      popup?.hidePopover();
    };
    addEventListener("scroll", close, true);
    addEventListener("resize", close);
    return () => {
      removeEventListener("scroll", close, true);
      removeEventListener("resize", close);
    };
  });

  /** Names how a journey of another day differs in date, such as "A day earlier". */
  function dayTag(offset: number): string {
    return offset < 0 ? "A day earlier" : "A day later";
  }
</script>

<button
  type="button"
  class="small with-icon alt-button"
  class:open
  bind:this={button}
  popovertarget={id}
  aria-expanded={open}
  title={kindsTooltip ?? undefined}
>
  Alternatives ({rows.length})
  <Icon name="chevron" turned={open} />
</button>

<div
  {id}
  popover="auto"
  class="popup"
  data-side={side}
  style:--caret-left={`${caretLeft}px`}
  bind:this={popup}
  ontoggle={(event) => {
    open = event.newState === "open";
    if (open) {
      place();
    }
  }}
>
  <div class="heading">
    <h4>Other journeys</h4>
    <button
      type="button"
      class="icon-button"
      popovertarget={id}
      popovertargetaction="hide"
      aria-label="Close"
      title="Close"
    >
      <Icon name="close" />
    </button>
  </div>
  <ol class="rows">
    {#each rows as row, rowIndex (rowIndex)}
      {@const overview = journeyOverview(row.plan, row.journey)}
      {@const price = row.journey.cost_parts?.price ?? null}
      {@const live = livePrices.has(journeyKey(row.journey))}
      {@const priceText = formatJourneyPrice(price, live)}
      <li>
        <span class="info">
          <span class="times numbers">
            {#if row.day !== null}
              <span>{formatDate(row.day.date)}</span>
            {/if}
            <span>{overview.boarding.time} → {overview.alighting.time}</span>
            {#if row.journey.alternative_kind !== null && row.journey.alternative_kind !== undefined}
              <span
                class="tag"
                title={row.journey.alternative_kind === "Cheaper"
                  ? "Much cheaper than the chosen journey and a little longer. Found by a search at another price weight."
                  : "Much faster than the chosen journey and a little more expensive. Found by a search at another price weight."}
              >
                {row.journey.alternative_kind}
              </span>
            {/if}
            {#if row.day !== null}
              <span class="tag">{dayTag(row.day.offset)}</span>
            {/if}
          </span>
          <span class="detail numbers">
            {formatDuration(overview.minutes)} · {transferSummary(row.journey)} · {overview.trains.join(", ")}
          </span>
          <span class="difference numbers">{describeDifference(row.journey, chosen)}</span>
        </span>
        <span class="price numbers" title={price === null ? undefined : describeJourneyPrice(price, live)}>
          {#if priceText !== null}
            {priceText}
            <span class="dot" class:open={!live}></span>
          {/if}
        </span>
        {#if row.day === null && onTake !== undefined}
          <button
            type="button"
            class="small"
            onclick={() => {
              onTake(row.journey);
              popup?.hidePopover();
            }}
          >
            Take
          </button>
        {:else}
          <span></span>
        {/if}
      </li>
    {/each}
  </ol>
</div>

<style>
  /* The button of an open popup keeps its pressed look, with the border of the popup, so the
     two read as one. */
  .alt-button {
    margin-left: auto;
  }

  .alt-button.open {
    border-color: var(--color-text-muted);
    background: var(--color-surface-muted);
  }

  /* The popup has a border that contrasts more than the border of a card, a caret that points
     at the button and a close button. The script sets its position and its width. */
  .popup {
    position: fixed;
    inset: auto;
    box-sizing: border-box;
    margin: 0;
    overflow: visible;
    padding: var(--space-3) var(--space-4);
    border: 1.5px solid var(--color-text-muted);
    border-radius: var(--radius);
    background: var(--color-surface);
    color: var(--color-text);
  }

  /* The caret is a rotated square that shares the border of the popup. */
  .popup::before {
    content: "";
    position: absolute;
    left: var(--caret-left);
    width: 12px;
    height: 12px;
    background: var(--color-surface);
    transform: rotate(45deg);
  }

  .popup[data-side="below"]::before {
    top: -8px;
    border-top: 1.5px solid var(--color-text-muted);
    border-left: 1.5px solid var(--color-text-muted);
  }

  .popup[data-side="above"]::before {
    bottom: -8px;
    border-bottom: 1.5px solid var(--color-text-muted);
    border-right: 1.5px solid var(--color-text-muted);
  }

  .heading {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    margin-bottom: var(--space-1);
  }

  h4 {
    margin: 0;
    font-size: var(--text-button);
    font-weight: 600;
  }

  /* A row has the journey, its price and its action in three columns. A journey of another day
     has no action, so its last cell stays empty and the prices keep their column. */
  .rows {
    display: flex;
    flex-direction: column;
    max-height: min(340px, calc(100vh - 160px));
    margin: 0;
    padding: 0;
    overflow-y: auto;
    list-style: none;
  }

  li {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto 3.4rem;
    align-items: center;
    column-gap: var(--space-3);
    padding: var(--space-2) 0;
  }

  li + li {
    border-top: 1px solid var(--color-border);
  }

  .info {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .times {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-2);
    font-weight: 700;
  }

  .detail {
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }

  .difference {
    font-size: var(--text-small);
  }

  .price {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    font-weight: 600;
    white-space: nowrap;
  }
</style>
