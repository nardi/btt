import { describe, expect, it } from "vitest";

import type { Journey, JourneyCostParts, LegPlan } from "./bindings";
import {
  compareJourneys,
  explainAlternative,
  explainChoice,
  explainNearbyDay,
  explainSearch,
  explainUnits,
  partCosts,
} from "./explanation";

function parts(changes: Partial<JourneyCostParts>): JourneyCostParts {
  const base: JourneyCostParts = {
    door_to_door_minutes: 300,
    way_to_station_minutes: 20,
    way_to_station_extra_cost: 10,
    transfer_count: 1,
    transfer_cost_each: 65,
    minutes_outside_arrival_window: 0,
    arrival_window_cost: 0,
    sleeping_minutes: 0,
    night_minutes_awake: 0,
    night_awake_cost: 0,
    rest_of_chain_cost: 0,
    total_cost: 0,
  };
  const merged = { ...base, ...changes };
  const total = Object.values(partCosts(merged)).reduce((sum, cost) => sum + cost, 0);
  return { ...merged, total_cost: total };
}

/** A journey on train `number`, leaving at a minute of 1 October 2026 in UTC. */
function journey(number: string, departureMinute: number, costParts: JourneyCostParts): Journey {
  const day = 29_846_880;
  return {
    access: { duration: 0, distance_meters: 0, mode: "Walk" },
    first_ride: {
      boarding_station: 0,
      departure_time: day + departureMinute,
      alighting_station: 1,
      arrival_time: day + departureMinute + 300,
      train_label: { category: "ICE", number },
      offers_berths: false,
      intermediate_station_coordinates: [],
    },
    later_rides: [],
    arrival_footpath: null,
    egress: { duration: 0, distance_meters: 0, mode: "Walk" },
    sleeping_ride: null,
    cost_parts: costParts,
  };
}

function plan(chosen: Journey, alternatives: Journey[], followsChoice = false): LegPlan {
  return {
    chosen_journey: chosen,
    alternative_journeys: alternatives,
    station_table: [
      { name: "Amsterdam-Centraal", coordinates: { latitude: 52.37, longitude: 4.9 }, time_zone: "UTC" },
      { name: "Berlin Hbf", coordinates: { latitude: 52.52, longitude: 13.37 }, time_zone: "UTC" },
    ],
    follows_traveller_choice: followsChoice,
    best_journey_cost: chosen.cost_parts?.total_cost ?? 0,
    departure_window: { start: 29_846_880 + 420, end: 29_846_880 + 1200 },
  };
}

describe("compareJourneys", () => {
  it("splits the difference into parts that add up to it", () => {
    const fast = parts({ door_to_door_minutes: 300, transfer_count: 1 });
    const slow = parts({ door_to_door_minutes: 400, transfer_count: 0 });
    const comparison = compareJourneys(fast, slow);
    expect(comparison.costDifference).toBe(100 - 65);
    expect(comparison.advantages.map((part) => part.phrase)).toEqual(["1 h 40 min shorter door to door"]);
    expect(comparison.drawbacks.map((part) => part.phrase)).toEqual(["1 more change"]);
    const sum = [...comparison.advantages, ...comparison.drawbacks].reduce((total, part) => total + part.cost, 0);
    expect(sum).toBe(comparison.costDifference);
  });

  it("leaves out small differences unless the journeys are close", () => {
    const first = parts({ door_to_door_minutes: 300, way_to_station_minutes: 20, way_to_station_extra_cost: 10 });
    const second = parts({ door_to_door_minutes: 400, way_to_station_minutes: 30, way_to_station_extra_cost: 15 });
    expect(compareJourneys(first, second).advantages.map((part) => part.part)).toEqual(["travel"]);
    const close = parts({ door_to_door_minutes: 303, way_to_station_minutes: 30, way_to_station_extra_cost: 15 });
    expect(compareJourneys(first, close).advantages.map((part) => part.part)).toEqual(["ways", "travel"]);
  });
});

describe("explanations", () => {
  const fast = journey("145", 468, parts({ door_to_door_minutes: 300, transfer_count: 1 }));
  const slow = journey("147", 588, parts({ door_to_door_minutes: 400, transfer_count: 0 }));

  it("explains the choice by what outweighs what", () => {
    expect(explainChoice(plan(fast, [slow]))).toBe(
      "Picked over ICE 147 at 09:48: 1 h 40 min shorter door to door. That outweighs 1 more change worth 1 h 5 min.",
    );
  });

  it("says when the traveller picked the journey", () => {
    expect(explainChoice(plan(slow, [fast], true))).toBe(
      "You picked this journey. The planner would take ICE 145 at 07:48, which counts 35 min less: 1 h 40 min shorter door to door.",
    );
  });

  it("explains an alternative by what makes it cost more", () => {
    expect(explainAlternative(plan(fast, [slow]), slow)).toBe(
      "counts 35 min more, because of 1 h 40 min longer door to door.",
    );
  });

  it("compares a nearby day on the journey alone", () => {
    const withChain = journey("145", 468, parts({ door_to_door_minutes: 600, rest_of_chain_cost: 200 }));
    const nightTrain = journey("402", 1200, parts({ door_to_door_minutes: 700, sleeping_minutes: 450 }));
    const suggestion = { day_offset: 1, departure_date: "2026-10-02", plan: plan(nightTrain, []) };
    expect(explainNearbyDay(plan(withChain, []), suggestion)).toBe(
      "A day later, ICE 402 at 20:00 counts 5 h 50 min less: 7 h 30 min more asleep in a berth.",
    );
  });

  it("names the units and the search limits", () => {
    expect(explainUnits(plan(fast, []))).toBe(
      "Each part counts as minutes on a train. A change counts as 1 h 5 min at your transfer aversion.",
    );
    expect(explainSearch(plan(fast, []))).toBe(
      "Only journeys leaving between 07:00 and 20:00 were compared, and a journey was left out when another leaves no earlier, arrives no later, has no more changes and costs no more.",
    );
  });
});
