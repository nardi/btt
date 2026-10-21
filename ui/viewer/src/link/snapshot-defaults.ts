// What a snapshot leaves out, which the viewer fills in with the values the app uses by default.

import type { JourneyCostParts, TravelClass, TravelDayWindows, TripPreferences } from "@btt/shared/bindings";

/** The ID of a trip in a snapshot. A snapshot has no trip ID, so every snapshot gets the nil UUID. */
export const snapshotTripId = "00000000-0000-0000-0000-000000000000";

/** The time windows a trip has by default, which a snapshot does not store. */
const defaultWindows: TravelDayWindows = {
  departure: { start: 420, length: 780 },
  arrival: { start: 420, length: 960 },
};

/**
 * Returns the preferences of a snapshot's trip, which are the default preferences of the app.
 * A snapshot of schema version 1 also keeps the time windows and the travel class, and passes
 * them.
 */
export function snapshotPreferences(
  windows: TravelDayWindows = defaultWindows,
  travelClass: TravelClass = "Second",
): TripPreferences {
  return {
    default_windows: windows,
    transfer_aversion: 50,
    default_same_day_stay: { minimum: 120, maximum_beyond_minimum: null },
    pass_policy: "PreferPass",
    minimum_transfer_time: 10,
    traveller_ids: [],
    travel_class: travelClass,
    price_weight: 0,
    ticket_preferences: {
      consider_passes: true,
      consider_families: true,
      fare_type: "Flexible",
      reserve_optional_seats: false,
      live_prices: true,
      assumed_purchase_date: null,
    },
  };
}

/**
 * Returns the cost parts of a journey in a snapshot, which hold the time door to door and the
 * price and zero for the parts that explain the planner's ranking.
 */
export function snapshotCostParts(
  doorToDoorMinutes: number,
  price: JourneyCostParts["price"],
): JourneyCostParts {
  return {
    door_to_door_minutes: doorToDoorMinutes,
    way_to_station_minutes: 0,
    way_to_station_extra_cost: 0,
    transfer_count: 0,
    transfer_cost_each: 0,
    minutes_outside_arrival_window: 0,
    arrival_window_cost: 0,
    sleeping_minutes: 0,
    night_minutes_awake: 0,
    night_awake_cost: 0,
    price,
    price_cost: 0,
    rest_of_chain_cost: 0,
    total_cost: 0,
  };
}
