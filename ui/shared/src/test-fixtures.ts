// Trips shared by the unit tests.

import type { LegTravelMode, Trip, TripPlace } from "./bindings";

export function place(id: number, name: string, latitude = 50, longitude = 10): TripPlace {
  return {
    id,
    name,
    coordinates: { latitude, longitude },
    station_search_radius: 5000,
    window_override: null,
  };
}

export const overnight: LegTravelMode = {
  Overnight: {
    boarding_window: { start: 1140, length: 240 },
    alighting_window: { start: 360, length: 240 },
    accepted_berths: { Either: { preferred: null } },
    minimum_time_on_night_train: null,
  },
};

/** Amsterdam, two nights in Vienna and a night train to Rome. */
export function amsterdamViennaRome(): Trip {
  return {
    id: "019a8f3c-6b2e-7c41-9d1a-3f5e2b7c8a90",
    name: "Test",
    start_date: "2026-10-01",
    itinerary: {
      Route: {
        origin: place(1, "Amsterdam", 52.3789, 4.9003),
        via_places: [
          {
            arriving_leg_travel_mode: "Daytime",
            place: place(2, "Vienna", 48.1852, 16.3761),
            stay: { Nights: 2 },
          },
        ],
        final_leg_travel_mode: overnight,
        destination: place(3, "Rome", 41.901, 12.5007),
      },
    },
    preferences: {
      default_windows: {
        departure: { start: 420, length: 780 },
        arrival: { start: 420, length: 960 },
      },
      transfer_aversion: 50,
      default_same_day_stay: { minimum: 120, maximum_beyond_minimum: null },
      pass_policy: "PreferPass",
      minimum_transfer_time: 10,
      traveller_ids: [],
      travel_class: "Second",
      price_weight: 0,
    },
    next_place_id: 4,
    chosen_journeys: [],
  };
}
