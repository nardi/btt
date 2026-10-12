import { describe, expect, it } from "vitest";

import type { Journey, LegPlan, PassScenario } from "./bindings";
import {
  legPlanOnPass,
  passOptionOfLeg,
  scenarioLeg,
  scenarioTitle,
  selectedScenario,
  type PassScenarioState,
} from "./scenarios";

/** A journey on train `category` with a total cost and a weighted price of `priceCost`. */
function journey(category: string, totalCost: number, priceCost: number, restOfChain = 0): Journey {
  return {
    access: { duration: 0, distance_meters: 0, mode: "Walk" },
    first_ride: {
      boarding_station: 0,
      departure_time: 29_846_880 + 480,
      alighting_station: 1,
      arrival_time: 29_846_880 + 600,
      train_label: { category, number: "1" },
      offers_berths: false,
      intermediate_station_coordinates: [],
    },
    later_rides: [],
    arrival_footpath: null,
    egress: { duration: 0, distance_meters: 0, mode: "Walk" },
    sleeping_ride: null,
    cost_parts: {
      door_to_door_minutes: 120,
      way_to_station_minutes: 0,
      way_to_station_extra_cost: 0,
      transfer_count: 0,
      transfer_cost_each: 65,
      minutes_outside_arrival_window: 0,
      arrival_window_cost: 0,
      sleeping_minutes: 0,
      night_minutes_awake: 0,
      night_awake_cost: 0,
      price: { low: 3000, median: 3500, high: 4000 },
      price_cost: priceCost,
      rest_of_chain_cost: restOfChain,
      total_cost: totalCost,
    },
  };
}

/** A leg on the regional train with one pass option on the ICE. */
function legPlan(): LegPlan {
  return {
    chosen_journey: journey("RE", 300 + 105 + 20, 10_500, 20),
    alternative_journeys: [],
    station_table: [],
    follows_traveller_choice: false,
    best_journey_cost: 425,
    cheapest: journey("RB", 500, 0),
    fastest: null,
    pass_options: [
      {
        journey: journey("ICE", 150, 3_000),
        cost_minutes: 120,
        price_cents: 1_000,
        days: 1,
        first_date: "2026-10-01",
      },
    ],
  };
}

const singles: PassScenario = { pass: null, pass_option_of_leg: [null, null], minutes: 600, price: 7_000 };
const onPass: PassScenario = {
  pass: { name: "Interrail Global Pass, 4 travel days in 1 month", first_valid_date: "2026-10-01", last_valid_date: "2026-10-31", price: 20_000 },
  pass_option_of_leg: [0, null],
  minutes: 420,
  price: 23_500,
};

describe("scenarioLeg", () => {
  it("leaves the weighted price and the rest of the chain out of the cost on single tickets", () => {
    const leg = scenarioLeg(legPlan(), "2026-10-01");
    expect(leg.single_cost_minutes).toBe(300);
    expect(leg.single_price).toBe(3_500);
    expect(leg.pass_options).toEqual([{ cost_minutes: 120, price_cents: 1_000, days: 1, first_date: "2026-10-01" }]);
  });
});

describe("passOptionOfLeg", () => {
  const state: PassScenarioState = { scenarios: [onPass, singles], arrivalPlaceIds: [2, 3], selectedIndex: 0 };

  it("finds the pass option of a leg in the picked scenario", () => {
    expect(passOptionOfLeg(state, 2)).toBe(0);
    expect(passOptionOfLeg(state, 3)).toBeNull();
    expect(passOptionOfLeg({ ...state, selectedIndex: 1 }, 2)).toBeNull();
  });

  it("gives single tickets to a leg the scenarios don't cover", () => {
    expect(passOptionOfLeg(state, 9)).toBeNull();
    expect(passOptionOfLeg(null, 2)).toBeNull();
  });

  it("keeps the pick within the scenarios there are", () => {
    expect(selectedScenario({ ...state, selectedIndex: 5 })).toBe(singles);
  });
});

describe("legPlanOnPass", () => {
  it("shows the pass journey and drops the comparisons with single tickets", () => {
    const plan = legPlanOnPass(legPlan(), 0);
    expect(plan.chosen_journey.first_ride.train_label.category).toBe("ICE");
    expect(plan.cheapest).toBeNull();
    expect(legPlanOnPass(legPlan(), 3).chosen_journey.first_ride.train_label.category).toBe("RE");
  });
});

describe("scenarioTitle", () => {
  it("names the pass and its window", () => {
    expect(scenarioTitle(singles)).toBe("Single tickets");
    expect(scenarioTitle(onPass)).toMatch(/^Interrail Global Pass, 4 travel days in 1 month, valid /);
  });
});
