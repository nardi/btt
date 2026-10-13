<script lang="ts">
  // A small turning ring that shows work is still going on.
  let {
    label,
  }: {
    /** What the work is, for screen readers and the tooltip. */
    label: string;
  } = $props();
</script>

<span class="spinner" role="status" aria-label={label} title={label}></span>

<style>
  .spinner {
    display: inline-block;
    width: 14px;
    height: 14px;
    vertical-align: -0.15em;
    border: 2px solid var(--color-border);
    border-top-color: var(--color-accent);
    border-radius: 50%;
    animation: spin 700ms linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation: none;
      border-top-color: var(--color-border);
    }
  }
</style>
