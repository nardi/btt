import { describe, expect, it } from "vitest";

import type { Journey, LegPlan, TrainRide } from "./bindings";
import {
  addDays,
  itineraryRows,
  journeyMinutes,
  journeyOverview,
  overriddenArrivalWindow,
  overriddenDepartureWindow,
  placeWindowOverride,
  stayLabel,
  transferSummary,
  tripPlaces,
} from "./itinerary";
import { amsterdamViennaRome, place } from "./test-fixtures";

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

  it("shows the arrival date of a plan that takes longer than the calendar assumes", () => {
    const trip = amsterdamViennaRome();
    const rome = tripPlaces(trip.itinerary)[2]!.id;
    const rows = itineraryRows(trip, new Map([[rome, "2026-10-05"]]));
    expect(rows.places[2]!.arrivalDate).toBe("2026-10-05");
    expect(rows.legs.map((leg) => leg.departureDate)).toEqual(["2026-10-01", "2026-10-03"]);
  });

  it("lists places of every itinerary variant", () => {
    const trip = amsterdamViennaRome();
    expect(tripPlaces(trip.itinerary).map((row) => row.name)).toEqual([
      "Amsterdam",
      "Vienna",
      "Rome",
    ]);
    expect(tripPlaces("Empty")).toEqual([]);
    const originOnly = { ...trip, itinerary: { OriginOnly: place(1, "Amsterdam") } };
    expect(itineraryRows(originOnly).legs).toEqual([]);
  });
});

describe("place window overrides", () => {
  const morning = { start: 7 * 60, length: 3 * 60 };
  const evening = { start: 17 * 60, length: 4 * 60 };

  it("keeps only the windows that differ from the defaults", () => {
    expect(placeWindowOverride(null, null)).toBeNull();
    expect(placeWindowOverride(morning, null)).toEqual({ Arrival: morning });
    expect(placeWindowOverride(null, evening)).toEqual({ Departure: evening });
    expect(placeWindowOverride(morning, evening)).toEqual({ Both: { arrival: morning, departure: evening } });
  });

  it("reads each window back out of an override", () => {
    const both = placeWindowOverride(morning, evening);
    expect(overriddenArrivalWindow(both)).toEqual(morning);
    expect(overriddenDepartureWindow(both)).toEqual(evening);
    expect(overriddenArrivalWindow({ Departure: evening })).toBeNull();
    expect(overriddenDepartureWindow(null)).toBeNull();
  });
});

/** Builds a ride between two stations of a leg plan, with times in minutes since the epoch. */
function ride(boarding: number, departure: number, alighting: number, arrival: number, number: string): TrainRide {
  return {
    boarding_station: boarding,
    departure_time: departure,
    alighting_station: alighting,
    arrival_time: arrival,
    train_label: { category: "ICE", number },
    offers_berths: false,
    intermediate_station_coordinates: [],
  };
}

const noWay = { duration: 0, distance_meters: 0, mode: "Walk" } as const;

/** Builds a journey from rides, with the given ways to the first and from the last station. */
function journey(rides: TrainRide[], access = 0, egress = 0): Journey {
  return {
    access: { ...noWay, duration: access },
    first_ride: rides[0]!,
    later_rides: rides.slice(1).map((later) => ({ transfer_duration: 10, ride: later })),
    arrival_footpath: null,
    egress: { ...noWay, duration: egress },
  };
}

const stations: LegPlan["station_table"] = [
  { name: "Amsterdam Centraal", coordinates: { latitude: 52.3789, longitude: 4.9003 }, time_zone: "Europe/Amsterdam" },
  { name: "Köln Hbf", coordinates: { latitude: 50.9432, longitude: 6.9587 }, time_zone: "Europe/Berlin" },
  { name: "Basel SBB", coordinates: { latitude: 47.5476, longitude: 7.5896 }, time_zone: "Europe/Zurich" },
];

function legPlan(chosen: Journey): LegPlan {
  return {
    chosen_journey: chosen,
    alternative_journeys: [],
    station_table: stations,
    follows_traveller_choice: false,
  };
}

/** 08:00 in Amsterdam on 1 October 2026, which is 06:00 UTC, in minutes since the epoch. */
const eight = Date.UTC(2026, 9, 1, 6, 0) / 60_000;

describe("journeyOverview", () => {
  it("summarises a direct journey by its ends and its length", () => {
    const direct = journey([ride(0, eight, 1, eight + 168, "125")], 5, 4);
    const overview = journeyOverview(legPlan(direct), direct);
    expect(overview.boarding).toEqual({ time: "08:00", station: "Amsterdam Centraal" });
    expect(overview.alighting).toEqual({ time: "10:48", station: "Köln Hbf" });
    expect(overview.transfers).toBe(0);
    expect(overview.trains).toEqual(["ICE 125"]);
    // The way to the station and the way from the last station count in the length.
    expect(overview.minutes).toBe(168 + 5 + 4);
  });

  it("takes the last alighting and counts the changes of a journey with two trains", () => {
    const changing = journey([ride(0, eight, 1, eight + 120, "125"), ride(1, eight + 130, 2, eight + 300, "71")]);
    const overview = journeyOverview(legPlan(changing), changing);
    expect(overview.boarding.station).toBe("Amsterdam Centraal");
    expect(overview.alighting).toEqual({ time: "13:00", station: "Basel SBB" });
    expect(overview.transfers).toBe(1);
    expect(overview.trains).toEqual(["ICE 125", "ICE 71"]);
    expect(overview.minutes).toBe(300);
  });

  it("falls back to placeholders for a station the plan lacks", () => {
    const lost = journey([ride(7, eight, 8, eight + 60, "1")]);
    const overview = journeyOverview(legPlan(lost), lost);
    expect(overview.boarding.station).toBe("Unknown station");
    expect(overview.boarding.time).toBe("06:00");
  });
});

describe("transferSummary", () => {
  it("names a direct journey and counts transfers", () => {
    expect(transferSummary(journey([ride(0, eight, 1, eight + 60, "1")]))).toBe("direct");
    const changing = journey([ride(0, eight, 1, eight + 60, "1"), ride(1, eight + 70, 2, eight + 90, "2")]);
    expect(transferSummary(changing)).toBe("1 transfer");
  });
});

describe("journeyMinutes", () => {
  it("adds the arrival footpath of a journey", () => {
    const withFootpath: Journey = {
      ...journey([ride(0, eight, 1, eight + 100, "1")], 2, 3),
      arrival_footpath: { to_station: 2, way: { ...noWay, duration: 6 } },
    };
    expect(journeyMinutes(withFootpath)).toBe(100 + 2 + 6 + 3);
  });
});

describe("stayLabel", () => {
  it("names the ends by their role and the places in between by their stay", () => {
    expect(stayLabel("origin", null)).toBe("origin");
    expect(stayLabel("destination", null)).toBe("destination");
    expect(stayLabel("via", { Nights: 1 })).toBe("1 night");
    expect(stayLabel("via", { Nights: 3 })).toBe("3 nights");
    expect(stayLabel("via", { SameDay: { stay_range_override: null, prefers_longer_stay: false } })).toBe("same day");
  });
});
