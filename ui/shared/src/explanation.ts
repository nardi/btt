// Explains why the planner picked a leg's journey, from the parts of each journey's cost.
//
// Every part counts in minutes of train travel and the parts add up to the cost the planner
// ranked by, so the difference between two journeys splits exactly into its parts.

import type { Journey, JourneyCostParts, LegPlan, NearbyDaySuggestion } from "./bindings";
import { formatDuration, formatTimeInZone } from "./format";
import { formatEuro } from "./tickets";

/** The parts of a cost, as the planner adds them up. */
export type CostPartName =
  | "travel"
  | "sleep"
  | "ways"
  | "changes"
  | "arrivalWindow"
  | "nightAwake"
  | "price"
  | "restOfChain";

/** Parts in the order the details table lists them. */
export const COST_PART_NAMES: readonly CostPartName[] = [
  "travel",
  "sleep",
  "ways",
  "changes",
  "arrivalWindow",
  "nightAwake",
  "price",
  "restOfChain",
];

/** Row labels of the details table. */
export const COST_PART_LABELS: Record<CostPartName, string> = {
  travel: "Door to door",
  sleep: "Asleep in a berth",
  ways: "Extra for ways to and from stations",
  changes: "Changes",
  arrivalWindow: "Outside the arrival window",
  nightAwake: "Awake at night",
  price: "Price",
  restOfChain: "Rest of the day",
};

/**
 * Differences smaller than this many minutes aren't mentioned, unless the two journeys are this
 * close in total.
 */
const SMALLEST_MENTIONED_DIFFERENCE = 10;

/** Most parts a sentence names in favour or against, the largest first. */
const MENTIONED_PARTS = 2;

/** One part in which two journeys differ, with its cost difference and a phrase for it. */
export interface PartDifference {
  part: CostPartName;
  /** Minutes the part costs more in the other journey than in this one. */
  cost: number;
  /** How this journey differs from the other in the part, such as "1 fewer change". */
  phrase: string;
}

/** How one journey compares with another. */
export interface JourneyComparison {
  /** Minutes the other journey costs more in total, negative when it costs less. */
  costDifference: number;
  /** Parts in which this journey is better, the largest first. */
  advantages: PartDifference[];
  /** Parts in which this journey is worse, the largest first. */
  drawbacks: PartDifference[];
}

/** Returns what each part adds to the cost. */
export function partCosts(parts: JourneyCostParts): Record<CostPartName, number> {
  return {
    travel: parts.door_to_door_minutes,
    sleep: -parts.sleeping_minutes,
    ways: parts.way_to_station_extra_cost,
    changes: parts.transfer_count * parts.transfer_cost_each,
    arrivalWindow: parts.arrival_window_cost,
    nightAwake: parts.night_awake_cost,
    price: Math.floor((parts.price_cost ?? 0) / 100),
    restOfChain: parts.rest_of_chain_cost,
  };
}

/** Describes how journey `own` differs from `other` in one part, from `own`'s side. */
function partPhrase(part: CostPartName, own: JourneyCostParts, other: JourneyCostParts): string {
  const more = (difference: number, word: string, lessWord: string) =>
    difference < 0 ? lessWord : word;
  switch (part) {
    case "travel": {
      const difference = own.door_to_door_minutes - other.door_to_door_minutes;
      return `${formatDuration(Math.abs(difference))} ${more(difference, "longer", "shorter")} door to door`;
    }
    case "sleep": {
      const difference = own.sleeping_minutes - other.sleeping_minutes;
      return `${formatDuration(Math.abs(difference))} ${more(difference, "more", "less")} asleep in a berth`;
    }
    case "ways": {
      const difference = own.way_to_station_minutes - other.way_to_station_minutes;
      return `${formatDuration(Math.abs(difference))} ${more(difference, "more", "less")} on the way to and from stations`;
    }
    case "changes": {
      const difference = own.transfer_count - other.transfer_count;
      const amount = Math.abs(difference);
      return `${amount} ${more(difference, "more", "fewer")} ${amount === 1 ? "change" : "changes"}`;
    }
    case "arrivalWindow": {
      const difference = own.minutes_outside_arrival_window - other.minutes_outside_arrival_window;
      return `${formatDuration(Math.abs(difference))} ${more(difference, "more", "less")} outside your arrival window`;
    }
    case "nightAwake": {
      const difference = own.night_minutes_awake - other.night_minutes_awake;
      return `${formatDuration(Math.abs(difference))} ${more(difference, "more", "less")} of the night awake`;
    }
    case "price": {
      const ownPrice = own.price?.median ?? null;
      const otherPrice = other.price?.median ?? null;
      if (ownPrice === null || otherPrice === null) {
        return "a price the planner cannot estimate";
      }
      const difference = ownPrice - otherPrice;
      return `${formatEuro(Math.abs(difference))} ${more(difference, "more expensive", "cheaper")}`;
    }
    case "restOfChain": {
      const difference = own.rest_of_chain_cost - other.rest_of_chain_cost;
      return `${more(difference, "a worse", "a better")} fit with the stays and legs around it`;
    }
  }
}

/** Compares journey `own` with `other` part by part. */
export function compareJourneys(own: JourneyCostParts, other: JourneyCostParts): JourneyComparison {
  const ownCosts = partCosts(own);
  const otherCosts = partCosts(other);
  const costDifference = other.total_cost - own.total_cost;
  const mentionAll = Math.abs(costDifference) < SMALLEST_MENTIONED_DIFFERENCE;
  const differences = COST_PART_NAMES.map((part) => ({
    part,
    cost: otherCosts[part] - ownCosts[part],
    phrase: partPhrase(part, own, other),
  })).filter(
    (difference) =>
      difference.cost !== 0 &&
      (mentionAll || Math.abs(difference.cost) >= SMALLEST_MENTIONED_DIFFERENCE),
  );
  return {
    costDifference,
    advantages: differences
      .filter((difference) => difference.cost > 0)
      .sort((first, second) => second.cost - first.cost),
    drawbacks: differences
      .filter((difference) => difference.cost < 0)
      .sort((first, second) => first.cost - second.cost),
  };
}

/** Joins phrases as "a, b and c". */
function joinPhrases(phrases: string[]): string {
  if (phrases.length <= 1) {
    return phrases.join("");
  }
  return `${phrases.slice(0, -1).join(", ")} and ${phrases.at(-1)}`;
}

/** Parts whose phrase already names the minutes they count. */
const PARTS_COUNTED_AS_STATED: readonly CostPartName[] = ["travel", "sleep"];

/**
 * Joins the phrases of differences with what each counts, such as "1 more change worth 1 h
 * 5 min". Door-to-door time and sleep count as the minutes their phrase names.
 */
function describeDifferences(differences: PartDifference[]): string {
  return joinPhrases(
    differences.map((difference) =>
      PARTS_COUNTED_AS_STATED.includes(difference.part)
        ? difference.phrase
        : `${difference.phrase} worth ${formatDuration(Math.abs(difference.cost))}`,
    ),
  );
}

/** Names a journey by its trains and first departure, such as "ICE 145 at 09:48". */
export function describeJourney(plan: LegPlan, journey: Journey): string {
  const rides = [journey.first_ride, ...journey.later_rides.map((later) => later.ride)];
  const trains = rides.map((ride) => `${ride.train_label.category} ${ride.train_label.number}`);
  const timeZone = plan.station_table[journey.first_ride.boarding_station]?.time_zone ?? "UTC";
  return `${trains.join(", ")} at ${formatTimeInZone(journey.first_ride.departure_time, timeZone)}`;
}

/** Explains why the chosen journey is shown, compared with the best alternative. */
export function explainChoice(plan: LegPlan): string {
  const chosen = plan.chosen_journey.cost_parts;
  const alternative = plan.alternative_journeys[0];
  if (chosen === null || chosen === undefined) {
    return "";
  }
  if (alternative?.cost_parts === null || alternative?.cost_parts === undefined) {
    return "It's the only journey that fits your times.";
  }
  const alternativeName = describeJourney(plan, alternative);
  if (plan.follows_traveller_choice) {
    const planners = compareJourneys(alternative.cost_parts, chosen);
    const reasons = planners.advantages.length > 0 ? `: ${describeDifferences(planners.advantages.slice(0, MENTIONED_PARTS))}` : "";
    return `You picked this journey. The planner would take ${alternativeName}, which counts ${formatDuration(Math.max(planners.costDifference, 0))} less${reasons}.`;
  }
  const comparison = compareJourneys(chosen, alternative.cost_parts);
  const reasons =
    comparison.advantages.length > 0
      ? `: ${describeDifferences(comparison.advantages.slice(0, MENTIONED_PARTS))}`
      : "";
  const tradeOff =
    comparison.drawbacks.length > 0
      ? ` That outweighs ${describeDifferences(comparison.drawbacks.slice(0, MENTIONED_PARTS))}.`
      : "";
  return `Picked over ${alternativeName}${reasons}.${tradeOff}`;
}

/**
 * Explains why an alternative wasn't chosen, following the journey's name, such as "counts 1 h
 * 5 min more, because of 1 more change worth 1 h 5 min".
 */
export function explainAlternative(plan: LegPlan, alternative: Journey): string {
  const chosen = plan.chosen_journey.cost_parts;
  if (chosen === null || chosen === undefined || alternative.cost_parts === null || alternative.cost_parts === undefined) {
    return "";
  }
  const comparison = compareJourneys(alternative.cost_parts, chosen);
  const amount = -comparison.costDifference;
  const direction = amount >= 0 ? "more" : "less";
  const reasons = comparison.drawbacks.slice(0, MENTIONED_PARTS);
  const detail = reasons.length > 0 ? `, because of ${describeDifferences(reasons)}` : "";
  return `counts ${formatDuration(Math.abs(amount))} ${direction}${detail}.`;
}

/** Explains how the best journey on a nearby day compares with the chosen one. */
export function explainNearbyDay(plan: LegPlan, nearbyDay: NearbyDaySuggestion): string {
  const chosen = plan.chosen_journey.cost_parts;
  const other = nearbyDay.plan.chosen_journey.cost_parts;
  if (chosen === null || chosen === undefined || other === null || other === undefined) {
    return "";
  }
  // Only the journey itself counts, because the other day has other legs and stays around it.
  const own = { ...other, rest_of_chain_cost: 0, total_cost: other.total_cost - other.rest_of_chain_cost };
  const current = {
    ...chosen,
    rest_of_chain_cost: 0,
    total_cost: chosen.total_cost - chosen.rest_of_chain_cost,
  };
  const comparison = compareJourneys(own, current);
  const day = nearbyDay.day_offset < 0 ? "A day earlier" : "A day later";
  const reasons =
    comparison.advantages.length > 0 ? `: ${describeDifferences(comparison.advantages.slice(0, MENTIONED_PARTS))}` : "";
  return `${day}, ${describeJourney(nearbyDay.plan, nearbyDay.plan.chosen_journey)} counts ${formatDuration(Math.max(comparison.costDifference, 0))} less${reasons}.`;
}

/** Says what the minutes in the explanation mean. */
export function explainUnits(plan: LegPlan): string {
  const each = plan.chosen_journey.cost_parts?.transfer_cost_each;
  const change = each === undefined ? "" : ` A change counts as ${formatDuration(each)} at your transfer aversion.`;
  return `Each part counts as minutes on a train.${change}`;
}

/** Says which journeys the planner compared at all. */
export function explainSearch(plan: LegPlan): string {
  const window = plan.departure_window;
  const timeZone = plan.station_table[plan.chosen_journey.first_ride.boarding_station]?.time_zone ?? "UTC";
  const leaving =
    window === null || window === undefined
      ? "Only journeys in your departure window were compared"
      : `Only journeys leaving between ${formatTimeInZone(window.start, timeZone)} and ${formatTimeInZone(window.end, timeZone)} were compared`;
  return `${leaving}, and a journey was left out when another leaves no earlier, arrives no later, has no more changes and costs no more.`;
}
