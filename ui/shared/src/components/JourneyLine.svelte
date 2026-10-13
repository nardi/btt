<script lang="ts">
  // The line of a journey: a dot at each end, one segment per train and a square at each change.
  // Each segment is as long as its train takes, but never too short to see.
  let {
    rideMinutes,
    night,
  }: {
    /** How long each train of the journey rides, in minutes, in travel order. */
    rideMinutes: readonly number[];
    /** Whether the leg is a night leg, which draws its segments dashed. */
    night: boolean;
  } = $props();

  /** The minutes of each segment, with one segment for a journey without rides. */
  const segments = $derived(rideMinutes.length === 0 ? [1] : rideMinutes.map((minutes) => Math.max(1, minutes)));
</script>

<div class="journey-line" aria-hidden="true">
  <span class="station"></span>
  {#each segments as minutes, index (index)}
    <span class="segment" class:night style:flex-grow={minutes}></span>
    <!-- A change of train sits between two segments. -->
    {#if index < segments.length - 1}
      <span class="change"></span>
    {/if}
  {/each}
  <span class="station"></span>
</div>

<style>
  .journey-line {
    display: flex;
    align-items: center;
    margin-left: 8px;
  }

  /* The segments share the width by their minutes, and a short train keeps a legible minimum. */
  .segment {
    flex: 1 1 0;
    min-width: 12px;
    height: 4px;
    border-radius: var(--radius-pill);
    background: var(--leg-colour);
  }

  .segment.night {
    background: repeating-linear-gradient(
      to right,
      var(--leg-colour) 0 9px,
      transparent 9px 15px
    );
  }

  .station {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--color-text);
  }

  .change {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 2px;
    background: var(--color-text);
  }
</style>
