<script lang="ts">
  // The tickets screen: the trains of the trip on the left, the tickets to buy on the right.
  import type { Price, TicketItem, TicketPlan, TicketStrategy } from "../bindings";
  import { formatDate, formatTimeInZone } from "../format";
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
    ridesByDate,
    ticketColour,
  } from "../tickets";
  import Spinner from "./Spinner.svelte";

  let {
    plan,
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

  /** Describes how much more a strategy costs than the cheapest. */
  function difference(strategy: TicketStrategy): string {
    const cheapest = strategies.reduce((best, other) => (middle(other) < middle(best) ? other : best), strategies[0]!);
    if (strategy === cheapest) {
      return "cheapest";
    }
    return `${formatEuro(Math.round(middle(strategy) - middle(cheapest)))} more`;
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
    class="retry"
    aria-label="Look up the live price again"
    title="Look up the live price again"
    onclick={() => onRetryLivePrice(item)}
  >
    <svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true" focusable="false">
      <path
        d="M20 12a8 8 0 1 1-2.34-5.66M20 4v5h-5"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
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
        <h3>{formatDate(group.date)}</h3>
        <ol>
          {#each group.indices as index (index)}
            {@const ride = plan.rides[index]!}
            <li>
              <div class="ride">
                <span class="name">{ride.train_label.category} {ride.train_label.number}</span>
                <span class="route">
                  {formatTimeInZone(ride.departure_time, ride.time_zone)}
                  {ride.from_name} to {ride.to_name}
                </span>
                <span class="chips">
                  {#each selected.covers[index] ?? [] as ticketId (ticketId)}
                    {@const ticket = selected.items.find((item) => item.id === ticketId)}
                    <span class="chip" style:--ticket-colour={ticketColour(ticketId)}>
                      {ticket?.title.split(",")[0] ?? "Ticket"}
                    </span>
                  {:else}
                    <span class="chip none">no ticket</span>
                  {/each}
                </span>
              </div>
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
        <span class="total-price">{formatTotal(totalOf(selected))}</span>
        {#if totalPending(selected)}
          {@render spinner("Looking up live prices")}
        {/if}
        <span class="badge kind-{totalOf(selected).kind}">
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
            <span>
              {formatTotal(totalOf(strategy))}
              {#if totalPending(strategy)}{@render spinner("Looking up live prices")}{/if}
            </span>
            <span class="muted">
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
            <div class="item-header">
              <span class="item-title">{item.title}</span>
              <span class="item-price" title={sourceText(price)}>
                {formatPrice(price)}
                {#if isRetrying(item) || isLivePending(item, livePrices, liveLoading)}
                  {@render spinner("Looking up the live price")}
                {:else if isLiveUnavailable(item, livePrices)}
                  <span class="badge unavailable" title="The booking site shows this trip but sells no ticket for it now, for example because the sale has not opened or it is sold out.">Unavailable</span>
                  {@render retryButton(item)}
                {:else if isLiveFailed(item, livePrices)}
                  <span class="badge failed" title="The booking site gave no price, for example because it did not answer.">No live price</span>
                  {@render retryButton(item)}
                {:else}
                  <span class="badge kind-{price.kind}">{priceKindLabel(price.kind)}</span>
                {/if}
              </span>
            </div>
            <p class="detail">{item.detail}</p>
            {#if livePriceOf(item, livePrices) !== null}
              <p class="detail">
                Estimate was {formatPrice(item.price)}.
              </p>
            {/if}
            {#if item.booking_link !== null}
              {@const link = item.booking_link}
              <button type="button" class="link" onclick={() => onOpenLink(link.url)}>
                {link.label}
              </button>
              <span class="muted">
                {link.kind === "Product" ? "opens the page that sells it" : "opens the shop's search"}
              </span>
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
    grid-template-columns: minmax(320px, 1fr) minmax(360px, 1fr);
    gap: var(--space-6);
    overflow-y: auto;
    min-height: 0;
    align-items: start;
  }

  h2,
  h3 {
    margin: 0 0 var(--space-2);
  }

  h3 {
    margin-top: var(--space-3);
    font-size: 1rem;
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

  li {
    padding: var(--space-2) 0;
    border-bottom: 1px solid var(--color-border);
  }

  .ride {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1) var(--space-3);
    align-items: baseline;
  }

  .name {
    font-weight: 600;
  }

  .route {
    flex: 1;
    min-width: 12em;
  }

  .chips {
    display: flex;
    gap: var(--space-1);
    flex-wrap: wrap;
  }

  .chip {
    background: var(--ticket-colour);
    color: white;
    border-radius: 999px;
    padding: 0 var(--space-2);
    font-size: 0.85em;
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
  }

  .total-price {
    font-size: 1.6rem;
    font-weight: 700;
  }

  .strategies {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  .strategies button {
    display: flex;
    gap: var(--space-3);
    align-items: baseline;
    text-align: left;
  }

  .strategies button.selected {
    border-color: var(--color-accent);
    outline: 1px solid var(--color-accent);
  }

  .strategy-title {
    flex: 1;
    font-weight: 600;
  }

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
    padding: var(--space-2) var(--space-3);
  }

  .item-header {
    display: flex;
    justify-content: space-between;
    gap: var(--space-3);
  }

  .item-title {
    font-weight: 600;
  }

  .item-price {
    white-space: nowrap;
  }

  .detail,
  .note {
    margin: var(--space-1) 0;
    color: var(--color-text-muted);
  }

  .notice {
    margin: 0;
    padding: var(--space-2) var(--space-3);
    background: var(--color-notice);
    border-radius: var(--radius);
  }

  .badge {
    font-size: 0.8em;
    border-radius: 4px;
    padding: 0 var(--space-1);
    border: 1px solid var(--color-border);
  }

  .badge.kind-Formula {
    border-color: #2f8f5b;
  }

  .badge.kind-Live {
    border-color: #1f6fb2;
  }

  .badge.kind-Range {
    border-color: #b2851f;
  }

  .badge.unavailable {
    border-color: var(--color-danger);
    color: var(--color-danger);
  }

  .badge.failed {
    color: var(--color-text-muted);
  }

  .retry {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 2px;
    margin-left: var(--space-1);
    border: 1px solid var(--color-border);
    border-radius: 50%;
    background: none;
    color: var(--color-text-muted);
    vertical-align: middle;
    cursor: pointer;
  }

  .retry:hover,
  .retry:focus-visible {
    color: var(--color-accent);
    border-color: var(--color-accent);
  }

  .muted {
    color: var(--color-text-muted);
  }

  .link {
    border: none;
    background: none;
    padding: 0;
    color: var(--color-accent);
    text-decoration: underline;
  }
</style>
