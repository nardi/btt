<script lang="ts">
  // The BigTrainTrip logo, a slanted "btt" over two rails. It takes its
  // colors from the theme tokens, so one drawing serves both themes.

  let {
    height = 44,
  }: {
    /** The height in pixels. The width follows from the proportions of the drawing. */
    height?: number;
  } = $props();
</script>

<svg
  class="logo"
  viewBox="-6 16 180 122"
  width={(height * 180) / 122}
  {height}
  role="img"
  aria-label="BigTrainTrip"
>
  <!-- The letters are strokes, so the logo needs no font. A thin stroke along the baseline ties
       the bowl of the b to both t shapes. -->
  <g transform="skewX(-10)">
    <g class="letters">
      <g stroke-width="17">
        <path d="M26 20V104.5M26 44H58Q70 44 70 56V84Q70 96 58 96H26" />
        <path d="M104 20V78Q104 96 122 96H132" />
        <path d="M156 20V78Q156 96 174 96H184" />
        <path d="M84 44H124M136 44H176" />
      </g>
      <path d="M58 100H184" stroke-width="9" />
    </g>
    <g class="rails">
      <rect x="22" y="113" width="162" height="7" rx="3.5" />
      <rect x="62" y="125" width="122" height="7" rx="3.5" />
    </g>
  </g>
</svg>

<style>
  .logo {
    flex: none;
  }

  .letters {
    fill: none;
    stroke: var(--color-accent);
    stroke-linejoin: round;
  }

  .rails {
    fill: var(--leg-1);
  }
</style>
