<script lang="ts" module>
  // The icons of the interface, drawn as outlines on a 24 by 24 grid.

  /** The name of an icon. */
  export type IconName =
    | "chevron"
    | "trash"
    | "pin"
    | "moon"
    | "undo"
    | "redo"
    | "plus"
    | "retry"
    | "back"
    | "close";

  /** The drawing of one icon. */
  interface IconShape {
    /** The path data of the strokes. */
    paths: readonly string[];
    /** The circles of the drawing, which a path cannot hold. */
    circles?: readonly { cx: number; cy: number; r: number }[];
    /** Whether the icon is filled with the text color and not drawn as an outline. */
    filled?: boolean;
  }

  const shapes: Record<IconName, IconShape> = {
    chevron: { paths: ["M6 9l6 6 6-6"] },
    trash: {
      paths: ["M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"],
    },
    pin: {
      paths: ["M12 21s-6-5.5-6-11a6 6 0 1 1 12 0c0 5.5-6 11-6 11z"],
      circles: [{ cx: 12, cy: 10, r: 2.2 }],
    },
    moon: { paths: ["M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z"], filled: true },
    undo: { paths: ["M4 9h10a5 5 0 0 1 0 10H8M4 9l4-4M4 9l4 4"] },
    redo: { paths: ["M20 9H10a5 5 0 0 0 0 10h6M20 9l-4-4M20 9l-4 4"] },
    plus: { paths: ["M12 5v14M5 12h14"] },
    retry: { paths: ["M20 12a8 8 0 1 1-2.34-5.66M20 4v5h-5"] },
    back: { paths: ["M19 12H5M11 6l-6 6 6 6"] },
    close: { paths: ["M6 6l12 12M18 6L6 18"] },
  };
</script>

<script lang="ts">
  let {
    name,
    size = 16,
    turned = false,
  }: {
    name: IconName;
    /** The width and height in pixels. */
    size?: number;
    /** Whether the icon is turned over, such as a chevron of an open card. */
    turned?: boolean;
  } = $props();

  const shape = $derived(shapes[name]);
</script>

<svg
  class="icon"
  class:filled={shape.filled === true}
  class:turned
  width={size}
  height={size}
  viewBox="0 0 24 24"
  aria-hidden="true"
  focusable="false"
>
  {#each shape.paths as path (path)}
    <path d={path} />
  {/each}
  {#each shape.circles ?? [] as circle (circle.cx)}
    <circle cx={circle.cx} cy={circle.cy} r={circle.r} />
  {/each}
</svg>

<style>
  .icon {
    flex: none;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
    transition: transform 130ms ease;
  }

  .icon.filled {
    fill: currentColor;
    stroke: none;
  }

  .icon.turned {
    transform: rotate(180deg);
  }
</style>
