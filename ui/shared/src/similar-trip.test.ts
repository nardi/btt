import { describe, expect, it } from "vitest";

import type { Journey, LegPlan, SimilarTrip, TicketItem, TicketPlan, TicketRide } from "./bindings";
import { arrivalPlaceOfTicket, journeyOfSimilarTrip } from "./similar-trip";

const START = 29_846_880;

/** A journey that leaves at a minute and rides the trains by name, such as `ICE 848`. */
function journey(departureMinute: number, trains: string[]): Journey {
  const ride = (name: string, index: number) => {
    const [category = "", number = ""] = name.split(" ");
    return {
      boarding_station: index,
      departure_time: START + departureMinute + index * 20,
      alighting_station: index + 1,
      arrival_time: START + departureMinute + index * 20 + 15,
      train_label: { category, number },
      offers_berths: false,
      intermediate_station_coordinates: [],
    };
  };
  const [first, ...later] = trains.map(ride);
  return {
    access: { duration: 0, distance_meters: 0, mode: "Walk" },
    first_ride: first!,
    later_rides: later.map((next) => ({ ride: next, transfer: { duration: 10, distance_meters: 0, mode: "Walk" } })),
    arrival_footpath: null,
    egress: { duration: 0, distance_meters: 0, mode: "Walk" },
  } as unknown as Journey;
}

function legPlan(chosen: Journey, alternatives: Journey[]): LegPlan {
  return { chosen_journey: chosen, alternative_journeys: alternatives, station_table: [], follows_traveller_choice: false };
}

function similarTrip(departureMinute: number, trains: string[]): SimilarTrip {
  return {
    price: { low: 13_799, high: 13_799, kind: "Live", source: { name: "DB", note: "", as_of: null, page: null } },
    trains,
    first_departure_time: START + departureMinute,
  };
}

describe("journeyOfSimilarTrip", () => {
  const chosen = journey(480, ["S 7", "ICE 848", "ICE 999", "Intercity 28470"]);
  const sold = journey(482, ["S 9", "ICE 848", "ICE 142", "Intercity 28470"]);

  it("finds the alternative with the middle trains of the site's trip", () => {
    const similar = similarTrip(482, ["S9", "ICE 848", "ICE 142", "IC 28470"]);
    expect(journeyOfSimilarTrip(legPlan(chosen, [sold]), similar)).toBe(sold);
  });

  it("finds nothing when no alternative rides the same middle trains", () => {
    const similar = similarTrip(482, ["S9", "ICE 848", "ICE 142", "IC 28470"]);
    const other = journey(482, ["S 9", "ICE 848", "ICE 5", "Intercity 28470"]);
    expect(journeyOfSimilarTrip(legPlan(chosen, [other]), similar)).toBeNull();
  });

  it("finds nothing when the alternative leaves long before the site's trip", () => {
    const similar = similarTrip(482 + 90, ["S9", "ICE 848", "ICE 142", "IC 28470"]);
    expect(journeyOfSimilarTrip(legPlan(chosen, [sold]), similar)).toBeNull();
  });

  it("finds nothing when the chosen journey already is the site's trip", () => {
    const similar = similarTrip(482, ["S9", "ICE 848", "ICE 142", "IC 28470"]);
    expect(journeyOfSimilarTrip(legPlan(sold, [chosen]), similar)).toBeNull();
  });

  it("takes the alternative that leaves closest to the site's trip", () => {
    const similar = similarTrip(482, ["S9", "ICE 848", "ICE 142", "IC 28470"]);
    const later = journey(500, ["S 9", "ICE 848", "ICE 142", "Intercity 28470"]);
    expect(journeyOfSimilarTrip(legPlan(chosen, [later, sold]), similar)).toBe(sold);
  });
});

describe("arrivalPlaceOfTicket", () => {
  const ride = (placeId: number) => ({ leg_arrival_place_id: placeId }) as TicketRide;
  const plan = { rides: [ride(2), ride(2), ride(3)] } as unknown as TicketPlan;
  const item = (rides: number[]) => ({ ride_indices: rides }) as TicketItem;

  it("names the place when every ride of the ticket belongs to one leg", () => {
    expect(arrivalPlaceOfTicket(plan, item([0, 1]))).toBe(2);
  });

  it("names none when the ticket spans legs", () => {
    expect(arrivalPlaceOfTicket(plan, item([1, 2]))).toBeNull();
  });
});
