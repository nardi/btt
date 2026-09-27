import { describe, expect, it } from "vitest";

import type { LegTravelMode, Trip, TripPlace } from "./bindings";
import { addDays, itineraryRows, tripPlaces } from "./itinerary";

function place(id: number, name: string): TripPlace {
  return {
    id,
    name,
    coordinates: { latitude: 50, longitude: 10 },
    station_search_radius: 5000,
    window_override: null,
  };
}

const overnight: LegTravelMode = {
  Overnight: {
    boarding_window: { start: 1140, length: 240 },
    alighting_window: { start: 360, length: 240 },
    accepted_berths: { Either: { preferred: null } },
    minimum_time_on_night_train: null,
  },
};

function amsterdamViennaRome(): Trip {
  return {
    id: "019a8f3c-6b2e-7c41-9d1a-3f5e2b7c8a90",
    name: "Test",
    start_date: "2026-10-01",
    itinerary: {
      Route: {
        origin: place(1, "Amsterdam"),
        via_places: [
          { arriving_leg_travel_mode: "Daytime", place: place(2, "Vienna"), stay: { Nights: 2 } },
        ],
        final_leg_travel_mode: overnight,
        destination: place(3, "Rome"),
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
  };
}

describe("addDays", () => {
  it("crosses month and year ends", () => {
    expect(addDays("2026-10-31", 1)).toBe("2026-11-01");
    expect(addDays("2026-12-31", 2)).toBe("2027-01-02");
  });

  it("ignores daylight saving changes", () => {
    expect(addDays("2026-10-24", 1)).toBe("2026-10-25");
    expect(addDays("2026-10-25", 1)).toBe("2026-10-26");
  });
});

describe("itineraryRows", () => {
  it("reaches Rome on the third morning after Vienna", () => {
    const rows = itineraryRows(amsterdamViennaRome());
    expect(rows.legs.map((leg) => leg.departureDate)).toEqual(["2026-10-01", "2026-10-03"]);
    expect(rows.places.map((row) => [row.role, row.arrivalDate, row.departureDate])).toEqual([
      ["origin", null, "2026-10-01"],
      ["via", "2026-10-01", "2026-10-03"],
      ["destination", "2026-10-04", null],
    ]);
  });

  it("lists places of every itinerary variant", () => {
    const trip = amsterdamViennaRome();
    expect(tripPlaces(trip.itinerary).map((row) => row.name)).toEqual([
      "Amsterdam",
      "Vienna",
      "Rome",
    ]);
    expect(tripPlaces("Empty")).toEqual([]);
    expect(itineraryRows({ ...trip, itinerary: { OriginOnly: place(1, "Amsterdam") } }).legs).toEqual(
      [],
    );
  });
});
