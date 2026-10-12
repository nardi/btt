import { describe, expect, it } from "vitest";

import type { Journey, LegPlan } from "./bindings";
import { alternativesOfKind, centsMore, describeDifference, describeKinds, minutesLonger } from "./alternatives";

/** A journey with a door-to-door time and a median price, and the kind it was offered as. */
function journey(minutes: number, priceCents: number | null, kind: Journey["alternative_kind"] = null): Journey {
  return {
    access: { duration: 0, distance_meters: 0, mode: "Walk" },
    first_ride: {
      boarding_station: 0,
      departure_time: 29_846_880,
      alighting_station: 1,
      arrival_time: 29_846_880 + minutes,
      train_label: { category: "ICE", number: "1" },
      offers_berths: false,
      intermediate_station_coordinates: [],
    },
    later_rides: [],
    arrival_footpath: null,
    egress: { duration: 0, distance_meters: 0, mode: "Walk" },
    sleeping_ride: null,
    alternative_kind: kind,
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
      price: priceCents === null ? null : { low: priceCents, median: priceCents, high: priceCents },
      price_cost: 0,
      rest_of_chain_cost: 0,
      total_cost: minutes,
    },
  };
}

const chosen = journey(300, 12_000);

describe("describeDifference", () => {
  it("says how much faster and cheaper or longer and more expensive", () => {
    expect(describeDifference(journey(260, 13_500), chosen)).toBe("40 min faster, €15 more");
    expect(describeDifference(journey(380, 3_500), chosen)).toBe("1 h 20 min longer, €85 cheaper");
  });

  it("says same for a difference that is too small to matter", () => {
    expect(describeDifference(journey(302, 11_950), chosen)).toBe("same time, same price");
    expect(describeDifference(journey(300, 10_500), chosen)).toBe("same time, €15 cheaper");
  });

  it("leaves a price out that is not known", () => {
    expect(describeDifference(journey(260, null), chosen)).toBe("40 min faster");
    expect(centsMore(journey(260, null), chosen)).toBeNull();
    expect(minutesLonger(journey(260, null), chosen)).toBe(-40);
  });
});

describe("alternativesOfKind", () => {
  const plan = {
    chosen_journey: chosen,
    alternative_journeys: [journey(310, 12_500), journey(300, 10_500, "Cheaper"), journey(250, 14_000, "Faster")],
    station_table: [],
    follows_traveller_choice: false,
  } as unknown as LegPlan;

  it("finds the alternatives that a search at another weight found", () => {
    expect(alternativesOfKind(plan, "Cheaper")).toHaveLength(1);
    expect(alternativesOfKind(plan, "Faster")).toHaveLength(1);
  });

  it("names the kinds of a plan, or none", () => {
    expect(describeKinds(plan)).toBe("cheaper and faster");
    const plain = { ...plan, alternative_journeys: [journey(310, 12_500)] } as unknown as LegPlan;
    expect(describeKinds(plain)).toBeNull();
  });
});
