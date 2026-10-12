import { describe, expect, it } from "vitest";

import type { LegPlan } from "./bindings";
import { amsterdamViennaRome } from "./test-fixtures";
import { legViewState, type LegViewState, type TripViewState } from "./trip-source";

/** A plan with nothing but a marker, because these tests only follow the plan through. */
const plan = { best_journey_cost: 7 } as unknown as LegPlan;

function viewState(pending: number[]): TripViewState {
  const legStates: [number, LegViewState][] = [
    [2, { status: "found", legPlan: plan }],
    [3, { status: "none", reason: "NoConnectionInWindow" }],
  ];
  return {
    trip: amsterdamViennaRome(),
    legStatesByArrivalPlace: new Map(legStates),
    nearbyDaysByArrivalPlace: new Map(),
    notices: [],
    errorMessage: null,
    passScenarios: null,
    livePricesByJourney: new Map(),
    extrasPendingArrivalPlaces: new Set(pending),
  };
}

describe("legViewState", () => {
  it("says a found leg waits for its extras while the planner searches for them", () => {
    const state = legViewState(viewState([2]), 2);
    expect(state).toMatchObject({ status: "found", extrasPending: true });
    expect(state.status === "found" && state.legPlan).toBe(plan);
  });

  it("says a found leg waits for nothing once its extras arrived", () => {
    expect(legViewState(viewState([]), 2)).toMatchObject({ status: "found", extrasPending: false });
  });

  it("leaves a leg without journeys and an unplanned leg as they are", () => {
    expect(legViewState(viewState([2]), 3).status).toBe("none");
    expect(legViewState(viewState([2]), 9)).toEqual({ status: "pending" });
  });
});
