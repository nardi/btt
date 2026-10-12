// Live prices of the candidate journeys of a leg, which replace the estimates the planner
// ranked by and can change which journey comes first.

import type { EuroRange, Journey, LegPlan } from "./bindings";

/** Returns a key that names a journey by its first departure and its trains. */
export function journeyKey(journey: Journey): string {
  const rides = [journey.first_ride, ...journey.later_rides.map((later) => later.ride)];
  const trains = rides.map((ride) => `${ride.train_label.category} ${ride.train_label.number}`);
  return `${journey.first_ride.departure_time}|${trains.join("|")}`;
}

/** The candidates of a leg whose live prices the app looks up: the chosen journey and the next two. */
export function liveCandidates(legPlan: LegPlan): Journey[] {
  return [legPlan.chosen_journey, ...legPlan.alternative_journeys.slice(0, 2)];
}

/**
 * Returns a journey with a live price in place of its estimate. The weighted price in the cost
 * becomes the weight times the live median, in hundredths of a minute like the planner counts.
 */
function withLivePrice(journey: Journey, live: EuroRange, priceWeight: number): Journey {
  const parts = journey.cost_parts ?? null;
  if (parts === null) {
    return journey;
  }
  const priceCost = Math.floor((priceWeight * live.median) / 100);
  const totalCost = parts.total_cost - Math.floor((parts.price_cost ?? 0) / 100) + Math.floor(priceCost / 100);
  return { ...journey, cost_parts: { ...parts, price: live, price_cost: priceCost, total_cost: totalCost } };
}

/** A leg plan with live prices, and the journey that ranked first before them when they changed the order. */
export interface LivePricedLegPlan {
  legPlan: LegPlan;
  /** The journey the estimates put first, when a live price moved another ahead of it. */
  overtaken: Journey | null;
}

/**
 * Puts the live prices the app knows into the candidates of a leg and ranks them again. A
 * journey the traveller picked stays chosen, and only gets its live price. The dead band of the
 * planner's ranking does not apply to live prices, which carry no noise of the model.
 */
export function legPlanWithLivePrices(
  legPlan: LegPlan,
  livePrices: ReadonlyMap<string, EuroRange>,
  priceWeight: number,
): LivePricedLegPlan {
  const reprice = (journey: Journey) => {
    const live = livePrices.get(journeyKey(journey));
    return live === undefined ? journey : withLivePrice(journey, live, priceWeight);
  };
  const candidates = [legPlan.chosen_journey, ...legPlan.alternative_journeys].map(reprice);
  const [chosen, ...alternatives] = candidates;
  const repriced = { ...legPlan, chosen_journey: chosen!, alternative_journeys: alternatives };
  if (legPlan.follows_traveller_choice || priceWeight === 0) {
    return { legPlan: repriced, overtaken: null };
  }
  const cost = (journey: Journey) => journey.cost_parts?.total_cost ?? Number.POSITIVE_INFINITY;
  const best = alternatives.reduce(
    (found, journey, index) => (cost(journey) < cost(found.journey) ? { journey, index } : found),
    { journey: chosen!, index: -1 },
  );
  if (best.index < 0) {
    return { legPlan: repriced, overtaken: null };
  }
  // The new first takes the place of the old one among the alternatives, so the rest keep their order.
  const reordered = [...alternatives];
  reordered[best.index] = chosen!;
  return {
    legPlan: { ...repriced, chosen_journey: best.journey, alternative_journeys: reordered },
    overtaken: legPlan.chosen_journey,
  };
}
