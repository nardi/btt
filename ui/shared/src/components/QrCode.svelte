<script lang="ts">
  // A QR code as a picture. The code is black on white in both themes, because scanners need the
  // contrast and a dark theme with light modules on dark often fails to scan.
  import { qrCodePath, type QrCodeMatrix, quietZoneModules } from "../qr";

  let { matrix, label }: { matrix: QrCodeMatrix; label: string } = $props();

  const side = $derived(matrix.size + 2 * quietZoneModules);
  const path = $derived(qrCodePath(matrix));
</script>

<svg
  class="qr-code"
  viewBox="0 0 {side} {side}"
  role="img"
  aria-label={label}
  shape-rendering="crispEdges"
>
  <rect width={side} height={side} fill="#ffffff" />
  <path d={path} fill="#000000" />
</svg>

<style>
  .qr-code {
    display: block;
    width: min(100%, 360px);
    height: auto;
    aspect-ratio: 1;
    border-radius: var(--radius);
  }
</style>
