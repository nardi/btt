<script lang="ts">
  // The name of a train on a pill, with a swatch in the color of its leg.
  let {
    label,
    colour,
    sleeping = false,
    small = false,
  }: {
    /** The name of the train, such as `ICE 125`. */
    label: string;
    /** A CSS color value for the swatch and the tint, such as `var(--leg-3)`. */
    colour: string;
    /** Whether the traveller sleeps on this train, which fills the pill with the night color. */
    sleeping?: boolean;
    /** Whether the pill is the smaller one that sits inside a line of text. */
    small?: boolean;
  } = $props();
</script>

<!-- The whole label is one child, so the gap of the flex row cannot split its words. -->
<span class="train-chip" class:sleeping class:small style:--leg-colour={colour}>
  <span class="swatch" aria-hidden="true"></span>
  <span class="numbers">{label}</span>
</span>

<style>
  .train-chip {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    padding: 4px 12px 4px 10px;
    border-radius: var(--radius-pill);
    font-size: var(--text-chip);
    font-weight: 600;
    white-space: nowrap;
    background: color-mix(in srgb, var(--leg-colour) 12%, var(--color-surface));
  }

  .train-chip.small {
    padding: 2px 10px 2px 8px;
    font-size: var(--text-small);
  }

  .swatch {
    width: 4px;
    height: 16px;
    border-radius: var(--radius-pill);
    background: var(--leg-colour);
  }

  /* The train the traveller sleeps on, in the purple of night trains. */
  .train-chip.sleeping {
    background: var(--color-night);
    color: var(--color-accent-contrast);
  }

  .train-chip.sleeping .swatch {
    background: var(--color-accent-contrast);
  }
</style>
