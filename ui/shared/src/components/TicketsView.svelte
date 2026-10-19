<script lang="ts">
  // The tickets screen: the trains of the trip on the left, the tickets to buy on the right.
  import type { Price, TicketItem, TicketPlan, TicketStrategy, TripPlaceId } from "../bindings";
  import { formatDate, formatTimeInZone } from "../format";
  import type { LegLine } from "../itinerary";
  import {
    adjustedTotal,
    effectivePrice,
    formatEuro,
    formatPrice,
    formatTotal,
    hasPendingLivePrice,
    isLivePending,
    isLiveFailed,
    isLiveUnavailable,
    liveQuoteKey,
    livePriceOf,
    type LivePrices,
    priceKindDescription,
    priceKindLabel,
    layoutLinkTitle,
    ridesByDate,
    ticketColour,
  } from "../tickets";
  import Icon from "./Icon.svelte";
  import Spinner from "./Spinner.svelte";
  import TrainChip from "./TrainChip.svelte";

  let {
    plan,
    legLines,
    loading,
    livePrices,
    liveLoading,
    retrying,
    unplannedLegs,
    onOpenLink,
    onRetryLivePrice,
  }: {
    /** The plan, or `null` before the first one arrives. */
    plan: TicketPlan | null;
    /** The color of each leg by its arrival place, which colors the train of each ride. */
    legLines: ReadonlyMap<TripPlaceId, LegLine>;
    loading: boolean;
    /** Live prices from booking sites, by ticket. They replace the estimates they are for. */
    livePrices: LivePrices;
    /** Whether live prices are still being looked up. */
    liveLoading: boolean;
    /** The keys of the tickets whose live price is being looked up again. */
    retrying: ReadonlySet<string>;
    /** Legs without a journey, which the plan leaves out. */
    unplannedLegs: number;
    /** Opens a booking link. */
    onOpenLink: (url: string) => void;
    /** Looks up the live price of a ticket again, whatever the site answered before. */
    onRetryLivePrice: (item: TicketItem) => void;
  } = $props();

  /** The strategy the traveller looks at, by position. The cheapest comes first. */
  let selectedIndex = $state(0);
  const strategies = $derived(plan?.strategies ?? []);
  const selected = $derived(strategies[Math.min(selectedIndex, strategies.length - 1)] ?? null);
  const groups = $derived(plan === null ? [] : ridesByDate(plan.rides));

  /** Whether the live price of a ticket is being looked up again. */
  function isRetrying(item: TicketItem): boolean {
    const key = liveQuoteKey(item);
    return key !== null && retrying.has(key);
  }

  /** The total of a strategy, with live prices where there are some. */
  function totalOf(strategy: TicketStrategy) {
    return adjustedTotal(strategy, livePrices);
  }

  /** Whether the total of a strategy waits for a live price. */
  function totalPending(strategy: TicketStrategy): boolean {
    return hasPendingLivePrice(strategy, livePrices, liveLoading);
  }

  /** Whether any strategy waits for a live price, which changes every comparison between them. */
  const anyPending = $derived(strategies.some(totalPending));

  /** The middle of a strategy's total, for comparing strategies. */
  function middle(strategy: TicketStrategy): number {
    const total = totalOf(strategy);
    return (total.low + total.high) / 2;
  }

  /**
   * Describes how much more a strategy costs than the cheapest. The difference is exact only
   * when both totals are exact, and an estimate of an estimate carries a tilde.
   */
  function difference(strategy: TicketStrategy): string {
    const cheapest = strategies.reduce((best, other) => (middle(other) < middle(best) ? other : best), strategies[0]!);
    if (strategy === cheapest) {
      return "cheapest";
    }
    const exact = totalOf(strategy).kind !== "Range" && totalOf(cheapest).kind !== "Range";
    const cents = middle(strategy) - middle(cheapest);
    // A difference of estimates is shown in whole euro, because cents would claim precision.
    return `${exact ? formatEuro(Math.round(cents)) : `~${formatEuro(Math.round(cents / 100) * 100)}`} more`;
  }

  /** Says where a price comes from, for its tooltip. */
  function sourceText(price: Price): string {
    const date = price.source.as_of === null ? "" : ` As of ${price.source.as_of}.`;
    return `${price.source.name}. ${price.source.note}${date}`;
  }
</script>

{#snippet spinner(label: string)}
  <Spinner {label} />
{/snippet}

{#snippet retryButton(item: TicketItem)}
  <button
    type="button"
    class="icon-button retry"
    aria-label="Look up the live price again"
    title="Look up the live price again"
    onclick={() => onRetryLivePrice(item)}
  >
    <Icon name="retry" size={14} />
  </button>
{/snippet}

<section class="tickets">
  {#if plan === null}
    <p class="empty">{loading ? "Pricing the trip…" : "Plan the trip to see its tickets."}</p>
  {:else if selected === null}
    <p class="empty">{plan.notes[0] ?? "The trip has no trains yet."}</p>
  {:else}
    <div class="trains">
      <h2>Trains</h2>
      {#each groups as group (group.date)}
        <h3 class="numbers">{formatDate(group.date)}</h3>
        <ol class="rides">
          {#each group.indices as index (index)}
            {@const ride = plan.rides[index]!}
            <li class="ride-row">
              <div class="run">
                <TrainChip
                  label={`${ride.train_label.category} ${ride.train_label.number}`}
                  colour={legLines.get(ride.leg_arrival_place_id)?.colour ?? "var(--color-text-muted)"}
                />
                {#each ride.layout_links ?? [] as link (link.url)}
                  <button
                    type="button"
                    class="icon-button layout-link"
                    aria-label={layoutLinkTitle(link.kind)}
                    title={layoutLinkTitle(link.kind)}
                    onclick={() => onOpenLink(link.url)}
                  >
                    <Icon name="coaches" size={14} />
                  </button>
                {/each}
                <span class="chips">
                  {#each selected.covers[index] ?? [] as ticketId (ticketId)}
                    {@const ticket = selected.items.find((item) => item.id === ticketId)}
                    <span class="chip" style:--ticket-colour={ticketColour(ticketId)}>
                      <span class="chip-dot" aria-hidden="true"></span>
                      <span>{ticket?.title.split(",")[0] ?? "Ticket"}</span>
                    </span>
                  {:else}
                    <span class="chip none">no ticket</span>
                  {/each}
                </span>
              </div>
              <span class="route numbers">
                {formatTimeInZone(ride.departure_time, ride.time_zone)}
                {ride.from_name} to {ride.to_name}
              </span>
            </li>
          {/each}
        </ol>
      {/each}
    </div>

    <div class="buy">
      <h2>Tickets</h2>
      {#if unplannedLegs > 0}
        <p class="notice">
          {unplannedLegs === 1 ? "One leg has" : `${unplannedLegs} legs have`} no journey yet and are
          left out.
        </p>
      {/if}
      <div class="total" title={priceKindDescription(totalOf(selected).kind)}>
        <span class="total-price numbers">{formatTotal(totalOf(selected))}</span>
        {#if totalPending(selected)}
          {@render spinner("Looking up live prices")}
        {/if}
        <span class="badge">
          {priceKindLabel(totalOf(selected).kind)}
        </span>
        {#if plan.traveller_name !== null}
          <span class="muted">for {plan.traveller_name}</span>
        {/if}
      </div>
      <div class="strategies" role="radiogroup" aria-label="Ways to buy">
        {#each strategies as strategy, strategyIndex (strategyIndex)}
          <button
            type="button"
            role="radio"
            aria-checked={strategy === selected}
            class:selected={strategy === selected}
            onclick={() => (selectedIndex = strategyIndex)}
          >
            <span class="strategy-title">{strategy.title}</span>
            <span class="strategy-total numbers">
              {formatTotal(totalOf(strategy))}
              {#if totalPending(strategy)}{@render spinner("Looking up live prices")}{/if}
            </span>
            <span class="difference muted numbers">
              {#if anyPending}{@render spinner("Comparing")}{:else}{difference(strategy)}{/if}
            </span>
          </button>
        {/each}
      </div>
      {#each [...plan.notes, ...selected.notes] as note (note)}
        <p class="note">{note}</p>
      {/each}
      <ul class="items">
        {#each selected.items as item (item.id)}
          {@const price = effectivePrice(item, livePrices)}
          <li class="item" style:--ticket-colour={ticketColour(item.id)}>
            <span class="item-title">{item.title}</span>
            <span class="item-price numbers" title={sourceText(price)}>
              {formatPrice(price)}
              {#if isRetrying(item) || isLivePending(item, livePrices, liveLoading)}
                {@render spinner("Looking up the live price")}
              {:else if isLiveUnavailable(item, livePrices)}
                <span
                  class="badge unavailable"
                  title="The booking site shows this trip but sells no ticket for it now, for example because the sale has not opened or it is sold out."
                >Unavailable</span>
                {@render retryButton(item)}
              {:else if isLiveFailed(item, livePrices)}
                <span
                  class="badge failed"
                  title="The booking site gave no price, for example because it did not answer."
                >No live price</span>
                {@render retryButton(item)}
              {:else if price.kind === "Live"}
                <!-- A filled green dot marks a live price. -->
                <span class="dot" title="A live price from the booking site."></span>
              {:else if price.kind === "Range"}
                <!-- An open dot marks an estimate. -->
                <span class="dot open" title="An estimate from the price model."></span>
              {:else}
                <span class="badge">{priceKindLabel(price.kind)}</span>
              {/if}
            </span>
            <p class="detail">{item.detail}</p>
            {#if livePriceOf(item, livePrices) !== null}
              <p class="detail">
                Estimate was {formatPrice(item.price)}.
              </p>
            {/if}
            {#if item.booking_link !== null}
              {@const link = item.booking_link}
              <button type="button" class="item-buy" onclick={() => onOpenLink(link.url)}>
                {link.label}
              </button>
            {/if}
          </li>
        {/each}
      </ul>
    </div>
  {/if}
</section>

<style>
  .tickets {
    display: grid;
    grid-template-columns: minmax(320px, 1fr) minmax(380px, 1.2fr);
    gap: var(--space-6);
    overflow-y: auto;
    min-height: 0;
    align-items: start;
  }

  h2 {
    margin: 0 0 var(--space-1);
    font-weight: 700;
    font-size: var(--text-section);
  }

  h3 {
    margin: var(--space-3) 0 var(--space-1);
    font-size: var(--text-small);
    font-weight: 400;
    color: var(--color-text-muted);
  }

  ol,
  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .empty {
    grid-column: 1 / -1;
    color: var(--color-text-muted);
  }

  .rides {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .ride-row {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px var(--space-4);
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
  }

  .run {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }

  /* Smaller than other icon buttons, so the train chip stays the main thing in the row. */
  .layout-link {
    width: 24px;
    height: 24px;
    color: var(--color-text-muted);
  }

  .route {
    color: var(--color-text-muted);
    font-size: var(--text-caption);
  }

  .chips {
    display: flex;
    gap: var(--space-1);
    flex-wrap: wrap;
  }

  /* A chip keeps its whole label in one child, so the flex gap cannot split the words. The
     ticket colour links each train to the ticket that covers it. */
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 2px 10px 2px 8px;
    border-radius: var(--radius-pill);
    font-size: var(--text-caption);
    font-weight: 600;
    background: color-mix(in srgb, var(--ticket-colour) 14%, var(--color-surface));
  }

  .chip .chip-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--ticket-colour);
  }

  .chip.none {
    background: var(--color-surface-muted);
    color: var(--color-text-muted);
  }

  .buy {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }

  .total {
    display: flex;
    align-items: baseline;
    gap: var(--space-3);
    flex-wrap: wrap;
    padding: var(--space-3) var(--space-4);
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
  }

  .total-price {
    font-size: var(--text-figure);
    font-weight: 700;
  }

  /* The strategy choices sit above a rule, apart from the ticket list under it. */
  .strategies {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding-bottom: var(--space-4);
    border-bottom: 1px solid var(--color-border);
  }

  .strategies button {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    text-align: left;
    padding: 10px 16px;
    border-radius: var(--radius);
  }

  .strategies button.selected {
    border: 2px solid var(--color-accent);
    padding: 9px 15px;
  }

  .strategy-title {
    font-size: var(--text-button);
    font-weight: 600;
  }

  .strategy-total {
    font-weight: 700;
    font-size: var(--text-lead);
  }

  .difference {
    font-size: var(--text-caption);
  }

  /* A ticket card: title and detail on the left, the price with the buy button under it on the
     right, so the eye links the two. */
  .items {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .item {
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-left: 4px solid var(--ticket-colour);
    border-radius: var(--radius);
    padding: var(--space-3) var(--space-4);
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    column-gap: var(--space-4);
    row-gap: var(--space-1);
    align-items: start;
  }

  .item-title {
    grid-column: 1;
    font-weight: 600;
  }

  .item-price {
    grid-column: 2;
    justify-self: end;
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    font-weight: 700;
    white-space: nowrap;
  }

  .detail {
    grid-column: 1;
    margin: 0;
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }

  .item-buy {
    grid-column: 2;
    grid-row: 2;
    justify-self: end;
    padding: 5px 13px;
    font-size: var(--text-small);
  }

  .notice {
    margin: 0;
    padding: 10px var(--space-4);
    background: var(--color-notice-fill);
    color: var(--color-notice);
    border-radius: var(--radius);
    font-size: var(--text-small);
  }

  .note {
    margin: 0;
    color: var(--color-text-muted);
  }

  .badge {
    font-size: var(--text-caption);
    border-radius: var(--radius-pill);
    padding: 3px 10px;
    background: var(--color-surface-muted);
    color: var(--color-text-muted);
  }

  .badge.unavailable {
    background: color-mix(in srgb, var(--color-danger) 8%, transparent);
    color: var(--color-danger);
  }

  .badge.failed {
    background: none;
    border: 1px solid var(--color-border);
  }

  /* The small round button that asks the booking site again. */
  .retry {
    width: 24px;
    height: 24px;
    margin-left: var(--space-1);
    color: var(--color-text-muted);
  }

  .muted {
    color: var(--color-text-muted);
  }
</style>
