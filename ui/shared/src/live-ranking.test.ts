import { describe, expect, it } from "vitest";

import type { EuroRange, Journey, LegPlan } from "./bindings";
import { journeyKey, legPlanWithLivePrices, liveCandidates } from "./live-ranking";

/** A journey on train `category` at a price, ranked at a weight of 3 minutes per euro. */
function journey(category: string, departureMinute: number, minutes: number, priceCents: number): Journey {
  const priceCost = Math.floor((300 * priceCents) / 100);
  return {
    access: { duration: 0, distance_meters: 0, mode: "Walk" },
    first_ride: {
      boarding_station: 0,
      departure_time: 29_846_880 + departureMinute,
      alighting_station: 1,
      arrival_time: 29_846_880 + departureMinute + minutes,
      train_label: { category, number: "1" },
      offers_berths: false,
      intermediate_station_coordinates: [],
    },
    later_rides: [],
    arrival_footpath: null,
    egress: { duration: 0, distance_meters: 0, mode: "Walk" },
    sleeping_ride: null,
    cost_parts: {
      door_to_door_minutes: minutes,
      way_to_station_minutes: 0,
      way_to_station_extra_cost: 0,
      transfer_count: 0,
      transfer_cost_each: 65,
      minutes_outside_arrival_window: 0,
      arrival_window_cost: 0,
      sleeping_minutes: 0,
      night_minutes_awake: 0,
      night_awake_cost: 0,
      price: { low: priceCents, median: priceCents, high: priceCents },
      price_cost: priceCost,
      rest_of_chain_cost: 0,
      total_cost: minutes + Math.floor(priceCost / 100),
    },
  };
}

/** The regional train at 300 minutes and 35 euro first, the ICE at 120 minutes and 120 euro next. */
function legPlan(followsChoice = false): LegPlan {
  return {
    chosen_journey: journey("RE", 480, 300, 3_500),
    alternative_journeys: [journey("ICE", 540, 120, 12_000), journey("IC", 600, 200, 9_000), journey("EC", 660, 220, 9_000)],
    station_table: [],
    follows_traveller_choice: followsChoice,
  };
}

const live = (cents: number): EuroRange => ({ low: cents, median: cents, high: cents });

describe("liveCandidates", () => {
  it("takes the chosen journey and the next two", () => {
    expect(liveCandidates(legPlan()).map((candidate) => candidate.first_ride.train_label.category)).toEqual([
      "RE",
      "ICE",
      "IC",
    ]);
  });
});

describe("legPlanWithLivePrices", () => {
  it("puts a journey first when its live price makes it the cheapest by cost", () => {
    const plan = legPlan();
    const ice = plan.alternative_journeys[0]!;
    // At 30 euro the ICE counts 120 + 90 minutes, less than the regional train's 405.
    const prices = new Map([[journeyKey(ice), live(3_000)]]);
    const { legPlan: ranked, overtaken } = legPlanWithLivePrices(plan, prices, 300);
    expect(ranked.chosen_journey.first_ride.train_label.category).toBe("ICE");
    expect(ranked.chosen_journey.cost_parts?.total_cost).toBe(210);
    expect(ranked.chosen_journey.cost_parts?.price).toEqual(live(3_000));
    expect(ranked.alternative_journeys.map((journey) => journey.first_ride.train_label.category)).toEqual([
      "RE",
      "IC",
      "EC",
    ]);
    expect(overtaken?.first_ride.train_label.category).toBe("RE");
  });

  it("keeps the order when the live price changes nothing", () => {
    const plan = legPlan();
    const prices = new Map([[journeyKey(plan.chosen_journey), live(4_000)]]);
    const { legPlan: ranked, overtaken } = legPlanWithLivePrices(plan, prices, 300);
    expect(ranked.chosen_journey.first_ride.train_label.category).toBe("RE");
    expect(ranked.chosen_journey.cost_parts?.total_cost).toBe(300 + 120);
    expect(overtaken).toBeNull();
  });

  it("leaves a journey the traveller picked first", () => {
    const plan = legPlan(true);
    const prices = new Map([[journeyKey(plan.alternative_journeys[0]!), live(1_000)]]);
    const { legPlan: ranked, overtaken } = legPlanWithLivePrices(plan, prices, 300);
    expect(ranked.chosen_journey.first_ride.train_label.category).toBe("RE");
    expect(ranked.alternative_journeys[0]!.cost_parts?.price).toEqual(live(1_000));
    expect(overtaken).toBeNull();
  });
});
