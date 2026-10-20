import { describe, expect, it } from "vitest";
import type { Journey, TripSnapshot } from "@btt/shared/bindings";
import demo from "./fixtures/demo.json";
import { snapshotHasPrices } from "./snapshot-prices";

/** Returns a journey without its price, as a link without prices holds it. */
function unpriced(journey: Journey): Journey {
  return {
    ...journey,
    cost_parts: journey.cost_parts == null ? journey.cost_parts : { ...journey.cost_parts, price: null },
  };
}

describe("finding the prices of a snapshot", () => {
  const snapshot = demo.document.snapshot as unknown as TripSnapshot;

  it("finds the prices of the demo trip", () => {
    expect(snapshotHasPrices(snapshot)).toBe(true);
  });

  it("finds none once every journey is without its price", () => {
    const stripped: TripSnapshot = {
      ...snapshot,
      legs: snapshot.legs.map((leg) => {
        const plan = leg.result.Found;
        if (plan === undefined) {
          return leg;
        }
        return {
          ...leg,
          result: {
            Found: {
              ...plan,
              chosen_journey: unpriced(plan.chosen_journey),
              alternative_journeys: plan.alternative_journeys.map(unpriced),
            },
          },
        };
      }),
    };
    expect(snapshotHasPrices(stripped)).toBe(false);
  });
});
