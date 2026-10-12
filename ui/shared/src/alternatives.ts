// How the alternatives of a leg differ from its chosen journey.

import type { AlternativeKind, Journey, LegPlan } from "./bindings";
import { formatDuration } from "./format";
import { formatEuro } from "./tickets";

/** Prices closer than this, in euro cents, count as the same. */
const SAME_PRICE_CENTS = 100;

/** Door-to-door times closer than this, in minutes, count as the same. */
const SAME_TIME_MINUTES = 5;

/**
 * Returns how many minutes longer a candidate takes door to door than the chosen journey, which
 * is negative when it is faster.
 */
export function minutesLonger(candidate: Journey, chosen: Journey): number {
  return (candidate.cost_parts?.door_to_door_minutes ?? 0) - (chosen.cost_parts?.door_to_door_minutes ?? 0);
}

/**
 * Returns how many euro cents a candidate costs more than the chosen journey, which is negative
 * when it is cheaper, or `null` when either price is not known.
 */
export function centsMore(candidate: Journey, chosen: Journey): number | null {
  const candidatePrice = candidate.cost_parts?.price?.median ?? null;
  const chosenPrice = chosen.cost_parts?.price?.median ?? null;
  if (candidatePrice === null || chosenPrice === null) {
    return null;
  }
  return candidatePrice - chosenPrice;
}

/**
 * Describes how a candidate differs from the chosen journey in time and price, such as
 * "40 min faster, €15 more" or "same time, €15 cheaper". A price that is not known is left out.
 */
export function describeDifference(candidate: Journey, chosen: Journey): string {
  const minutes = minutesLonger(candidate, chosen);
  const time =
    Math.abs(minutes) < SAME_TIME_MINUTES
      ? "same time"
      : minutes < 0
        ? `${formatDuration(-minutes)} faster`
        : `${formatDuration(minutes)} longer`;
  const cents = centsMore(candidate, chosen);
  if (cents === null) {
    return time;
  }
  if (Math.abs(cents) < SAME_PRICE_CENTS) {
    return `${time}, same price`;
  }
  return cents < 0 ? `${time}, ${formatEuro(-cents)} cheaper` : `${time}, ${formatEuro(cents)} more`;
}

/** Returns the alternatives that a search at another weight found, of a kind. */
export function alternativesOfKind(legPlan: LegPlan, kind: AlternativeKind): Journey[] {
  return legPlan.alternative_journeys.filter((journey) => journey.alternative_kind === kind);
}

/** Describes which kinds of alternatives a plan has, such as "cheaper and faster", or `null`. */
export function describeKinds(legPlan: LegPlan): string | null {
  const kinds: string[] = [];
  if (alternativesOfKind(legPlan, "Cheaper").length > 0) {
    kinds.push("cheaper");
  }
  if (alternativesOfKind(legPlan, "Faster").length > 0) {
    kinds.push("faster");
  }
  return kinds.length === 0 ? null : kinds.join(" and ");
}
