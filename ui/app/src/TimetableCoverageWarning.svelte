<script lang="ts">
  // A warning mark next to the start date, which lists the operators with no trains on the days
  // of the trip. Hovering or focusing the mark opens the list, and a click keeps it open.
  import { formatDateWithYear } from "@btt/shared/format";
  import Icon from "@btt/shared/components/Icon.svelte";
  import type { OperatorGap } from "@btt/shared/timetable-coverage";

  let {
    gaps,
    lastDate,
  }: {
    gaps: OperatorGap[];
    /** The day the trip reaches its destination, as `YYYY-MM-DD`. */
    lastDate: string;
  } = $props();

  /** Whether the pointer or the keyboard focus is on the mark or the list. */
  let attended = $state(false);
  /** Whether a click keeps the list open. */
  let pinned = $state(false);
  let container: HTMLElement | undefined = $state();

  const open = $derived(attended || pinned);
  const summary = $derived(
    `${gaps.length === 1 ? "1 operator has" : `${gaps.length} operators have`} no train data for all days of this trip`,
  );

  function closeOnOutsideClick(event: MouseEvent) {
    if (pinned && container !== undefined && !container.contains(event.target as Node)) {
      pinned = false;
    }
  }

  function closeOnEscape(event: KeyboardEvent) {
    if (event.key === "Escape") {
      pinned = false;
      attended = false;
    }
  }
</script>

<svelte:window onclick={closeOnOutsideClick} onkeydown={closeOnEscape} />

<span
  class="coverage-warning"
  bind:this={container}
  role="presentation"
  onmouseenter={() => (attended = true)}
  onmouseleave={() => (attended = false)}
  onfocusin={() => (attended = true)}
  onfocusout={() => (attended = false)}
>
  <button
    type="button"
    class="icon-button warning"
    aria-label={summary}
    aria-expanded={open}
    onclick={() => (pinned = !pinned)}
  >
    <Icon name="warning" />
  </button>
  {#if open}
    <div class="anchor">
      <div class="panel" role="dialog" aria-label="Operators without train data">
        <p>
          Some operators have no train data up to {formatDateWithYear(lastDate)}, the last
          day of this trip.
        </p>
        <ul>
          {#each gaps as gap (gap.name)}
            <li>
              <span class="name">{gap.name}</span>
              <span class="detail">until {formatDateWithYear(gap.lastDepartureDate)}</span>
            </li>
          {/each}
        </ul>
      </div>
    </div>
  {/if}
</span>

<style>
  .coverage-warning {
    position: relative;
    display: inline-flex;
  }

  .warning {
    color: var(--color-notice);
  }

  /* Holds the gap below the mark, so the pointer can move onto the list without leaving. */
  .anchor {
    position: absolute;
    top: 100%;
    left: 0;
    z-index: 20;
    padding-top: var(--space-1);
  }

  .panel {
    width: max-content;
    max-width: min(24rem, 90vw);
    padding: var(--space-3);
    border: 1px solid var(--color-border);
    border-radius: 8px;
    background: var(--color-surface);
    box-shadow: 0 4px 16px rgb(0 0 0 / 0.18);
  }

  .panel p {
    margin: 0 0 var(--space-2);
    color: var(--color-text-muted);
  }

  .panel ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .panel li {
    display: flex;
    justify-content: space-between;
    gap: var(--space-3);
  }

  .detail {
    color: var(--color-text-muted);
    white-space: nowrap;
  }
</style>
