<script lang="ts">
  // The line of a journey: a dot at each end, one segment per train and a square at each change.
  let {
    rideCount,
    night,
  }: {
    /** The number of trains of the journey, at least one. */
    rideCount: number;
    /** Whether the leg is a night leg, which draws its segments dashed. */
    night: boolean;
  } = $props();

  /** The positions of the segments, one per train. */
  const segments = $derived(Array.from({ length: Math.max(1, rideCount) }, (_, index) => index));
</script>

<div class="journey-line" aria-hidden="true">
  <span class="station"></span>
  {#each segments as segment, index (segment)}
    <span class="segment" class:night></span>
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

  .segment {
    flex: 1;
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
