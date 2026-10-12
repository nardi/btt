// The choice between single tickets and a pass for the whole trip, and the journeys each
// choice gives the legs.

import type { LegPlan, PassScenario, PassScenarioLeg, TripPlaceId } from "./bindings";
import { formatDate, formatDuration } from "./format";
import { formatEuro } from "./tickets";

/** The pass scenarios of a trip and the one the traveller looks at. */
export interface PassScenarioState {
  /** The scenarios, the lowest weighted total first. */
  readonly scenarios: readonly PassScenario[];
  /** The legs the scenarios cover, by arrival place, in the order of `pass_option_of_leg`. */
  readonly arrivalPlaceIds: readonly TripPlaceId[];
  /** The scenario the traveller picked, by position. */
  readonly selectedIndex: number;
}

/**
 * Returns a leg as the solve of the pass scenarios takes it, departing on `date`. The cost of
 * the journey on single tickets leaves out the weighted price and the rest of a chain, because
 * the solve adds prices on its own and counts each leg once.
 */
export function scenarioLeg(legPlan: LegPlan, date: string): PassScenarioLeg {
  const parts = legPlan.chosen_journey.cost_parts ?? null;
  const singleCost =
    parts === null
      ? (legPlan.best_journey_cost ?? 0)
      : parts.total_cost - Math.floor((parts.price_cost ?? 0) / 100) - parts.rest_of_chain_cost;
  return {
    date,
    single_cost_minutes: Math.max(singleCost, 0),
    single_price: parts?.price?.median ?? null,
    pass_options: (legPlan.pass_options ?? []).map((option) => ({
      cost_minutes: option.cost_minutes,
      price_cents: option.price_cents,
      days: option.days,
      first_date: option.first_date,
    })),
  };
}

/** Returns the scenario the traveller looks at, or `null` when the trip has none to pick. */
export function selectedScenario(state: PassScenarioState | null): PassScenario | null {
  if (state === null) {
    return null;
  }
  return state.scenarios[Math.min(state.selectedIndex, state.scenarios.length - 1)] ?? null;
}

/**
 * Returns the pass option a leg takes in the scenario the traveller looks at, as an index into
 * its pass options, or `null` when it takes single tickets.
 */
export function passOptionOfLeg(state: PassScenarioState | null, arrivalPlaceId: TripPlaceId): number | null {
  const scenario = selectedScenario(state);
  const position = state?.arrivalPlaceIds.indexOf(arrivalPlaceId) ?? -1;
  if (scenario === null || position < 0) {
    return null;
  }
  return scenario.pass_option_of_leg[position] ?? null;
}

/**
 * Returns the plan of a leg with the journey of one of its pass options as the chosen one. The
 * cheaper and faster journeys compare with single tickets, so they go.
 */
export function legPlanOnPass(legPlan: LegPlan, optionIndex: number): LegPlan {
  const option = legPlan.pass_options?.[optionIndex];
  if (option === undefined) {
    return legPlan;
  }
  return { ...legPlan, chosen_journey: option.journey, cheapest: null, fastest: null };
}

/** Returns the name of a scenario, such as the pass and its window of validity. */
export function scenarioTitle(scenario: PassScenario): string {
  if (scenario.pass === null) {
    return "Single tickets";
  }
  const { name, first_valid_date, last_valid_date } = scenario.pass;
  return `${name}, valid ${formatDate(first_valid_date)} to ${formatDate(last_valid_date)}`;
}

/** Describes what a scenario costs in money and in minutes of train travel. */
export function scenarioSummary(scenario: PassScenario): string {
  const pass = scenario.pass === null ? "" : `, the pass ${formatEuro(scenario.pass.price)} of it`;
  return `about ${formatEuro(scenario.price)}${pass}, counts ${formatDuration(scenario.minutes)}`;
}
