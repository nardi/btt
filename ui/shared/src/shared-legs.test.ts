import { describe, expect, it } from "vitest";

import type { LegPlan } from "./bindings";
import { sharedLegs } from "./shared-legs";
import { amsterdamViennaRome } from "./test-fixtures";
import type { LegViewState, TripViewState } from "./trip-source";

/** A plan with nothing but a marker, because these tests only follow the plan through. */
const plan = { best_journey_cost: 7 } as unknown as LegPlan;

function viewState(legStates: [number, LegViewState][], extrasPending: number[] = []): TripViewState {
  return {
    trip: amsterdamViennaRome(),
    legStatesByArrivalPlace: new Map(legStates),
    nearbyDaysByArrivalPlace: new Map(),
    notices: [],
    errorMessage: null,
    passScenarios: null,
    livePricesByJourney: new Map(),
    extrasPendingArrivalPlaces: new Set(extrasPending),
  };
}

describe("sharedLegs", () => {
  it("takes the planned legs and the legs without a journey in itinerary order", () => {
    const shared = sharedLegs(
      viewState([
        [3, { status: "none", reason: "NoConnectionInWindow" }],
        [2, { status: "found", legPlan: plan }],
      ]),
    );
    expect(shared.legs).toEqual([
      { arrival_place_id: 2, result: { Found: plan } },
      { arrival_place_id: 3, result: { NoJourneyFound: "NoConnectionInWindow" } },
    ]);
    expect(shared.unfinishedLegs).toBe(0);
  });

  it("counts a leg that waits for its plan and leaves it out", () => {
    const shared = sharedLegs(viewState([[2, { status: "found", legPlan: plan }]]));
    expect(shared.legs).toHaveLength(1);
    expect(shared.unfinishedLegs).toBe(1);
  });

  it("counts a planned leg that still waits for its cheaper and faster journeys", () => {
    const shared = sharedLegs(
      viewState(
        [
          [2, { status: "found", legPlan: plan }],
          [3, { status: "found", legPlan: plan }],
        ],
        [3],
      ),
    );
    expect(shared.legs).toHaveLength(2);
    expect(shared.unfinishedLegs).toBe(1);
  });
});
