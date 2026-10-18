<script lang="ts">
  // A warning mark next to the start date, which lists the operators with no trains on the days
  // of the trip.
  import { formatDate } from "@btt/shared/format";
  import Icon from "@btt/shared/components/Icon.svelte";
  import type { OperatorGap } from "@btt/shared/timetable-coverage";

  let {
    gaps,
    lastDepartureDate,
  }: {
    gaps: OperatorGap[];
    /** The day the last leg of the trip departs, as `YYYY-MM-DD`. */
    lastDepartureDate: string;
  } = $props();

  let open = $state(false);
  let container: HTMLElement | undefined = $state();

  const summary = $derived(
    `${gaps.length === 1 ? "1 operator has" : `${gaps.length} operators have`} no trains on all days of this trip`,
  );

  function closeOnOutsideClick(event: MouseEvent) {
    if (open && container !== undefined && !container.contains(event.target as Node)) {
      open = false;
    }
  }

  function closeOnEscape(event: KeyboardEvent) {
    if (open && event.key === "Escape") {
      open = false;
    }
  }
</script>

<svelte:window onclick={closeOnOutsideClick} onkeydown={closeOnEscape} />

<span class="coverage-warning" bind:this={container}>
  <button
    type="button"
    class="icon-button warning"
    aria-label={summary}
    aria-expanded={open}
    title={summary}
    onclick={() => (open = !open)}
  >
    <Icon name="warning" />
  </button>
  {#if open}
    <div class="panel" role="dialog" aria-label="Operators without trains">
      <p>No train data for these operators on {formatDate(lastDepartureDate)}, the last day of the trip.</p>
      <ul>
        {#each gaps as gap (gap.name)}
          <li>
            <span class="name">{gap.name}</span>
            <span class="detail">
              {gap.runsAtStart ? `until ${formatDate(gap.lastDepartureDate)}` : "none on the trip days"}
            </span>
          </li>
        {/each}
      </ul>
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

  .panel {
    position: absolute;
    top: calc(100% + var(--space-1));
    left: 0;
    z-index: 20;
    width: max-content;
    max-width: min(22rem, 90vw);
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
  }
</style>
