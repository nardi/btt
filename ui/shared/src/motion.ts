// The transitions of the itinerary, shared by place rows, leg rows and their expanded parts.

import type { FlyParams, SlideParams } from "svelte/transition";

/** Whether the traveller asked the system to keep motion to a minimum. */
export const prefersReducedMotion =
  typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

/** The longest delay of a row of a freshly opened trip, so no animation outlasts 200 ms. */
const longestStagger = 200;

/** Returns the delay of the row at a position, 40 ms per row up to the longest delay. */
export function staggerDelay(position: number): number {
  return Math.min(Math.max(position, 0) * 40, longestStagger);
}

/** Returns how a row enters: a short slide and fade, staggered by its position in the list. */
export function rowEnter(delay: number): FlyParams {
  return prefersReducedMotion ? { duration: 0 } : { y: 8, duration: 200, delay };
}

/** Returns how a row leaves, which plays the enter animation backwards. */
export function rowExit(): FlyParams {
  return prefersReducedMotion ? { duration: 0 } : { y: 8, duration: 150 };
}

/** Returns how the expanded part of a card opens and closes, a short slide. */
export function expandTransition(): SlideParams {
  return prefersReducedMotion ? { duration: 0 } : { duration: 150 };
}
