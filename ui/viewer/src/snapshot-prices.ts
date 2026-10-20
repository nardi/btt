// Tells whether a snapshot holds prices, which a link can leave out.

import type { Journey, TripSnapshot } from "@btt/shared/bindings";

/** Returns whether any journey of the snapshot has a price estimate. */
export function snapshotHasPrices(snapshot: TripSnapshot): boolean {
  return snapshot.legs.some(({ result }) => {
    const plan = result.Found;
    if (plan === undefined) {
      return false;
    }
    const journeys: Journey[] = [plan.chosen_journey, ...plan.alternative_journeys];
    return journeys.some((journey) => journey.cost_parts?.price != null);
  });
}
