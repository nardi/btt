// Reads a snapshot from the bit layout that `crates/btt-model/src/link/codec.rs` writes.
//
// The reader walks the layout in the order the writer wrote it. It checks every value against
// the range of its type, because a link is input from anyone: an index that points outside its
// table, a count the data cannot hold or a time out of range ends the read with an error.

import type {
  AcceptedBerths,
  AlternativeKind,
  DailyTimeWindow,
  Itinerary,
  EuroRange,
  Journey,
  LegPlan,
  LegPlanStation,
  LegPlanningResult,
  LegTravelMode,
  NoJourneyReason,
  PlaceStay,
  PlaceWindowOverride,
  SnapshotLeg,
  StationWay,
  TrainRide,
  TransferAndRide,
  Trip,
  TripPlace,
  TripSnapshot,
} from "@btt/shared/bindings";
import { categories, operators, timeZones } from "../generated/vocabularies";
import { BitReader } from "./bit-reader";
import { SnapshotLinkError, malformed } from "./errors";
import { snapshotCostParts, snapshotPreferences, snapshotTripId } from "./snapshot-defaults";
import { type DictionaryCity, placeStepMicrodegrees, stepsOf } from "./cities";
import type { DictionaryStation } from "./stations";

/** The newest schema version this viewer reads. */
export const supportedSchemaVersion = 1;

/** Most stations one leg plan may have, because journeys point at them with a byte. */
const maximumLegStations = 256;

/** The reasons for no journey, in the order of their codes. */
const noJourneyReasons: readonly NoJourneyReason[] = [
  "NoStationNearDeparturePlace",
  "NoStationNearArrivalPlace",
  "NoConnectionInWindow",
  "BackendUnavailable",
  "TravelDateOutsideTimetable",
];

const minutesPerDay = 1440;
const maximumMicrodegreesOfLatitude = 90_000_000;
const maximumMicrodegreesOfLongitude = 180_000_000;
const maximumUnixMinutes = 2 ** 32 - 1;

/** The largest number of days from 1970-01-01 to a date the viewer shows, in the year 9999. */
const maximumDays = 2_932_896;

/** Returns the date a number of days after 1970-01-01, as the `YYYY-MM-DD` text of a date. */
function dateFromDays(days: number): string {
  if (days > maximumDays) {
    throw malformed("a date is out of range");
  }
  return new Date(days * 86_400_000).toISOString().slice(0, 10);
}

/** Reads the schema version at the start of the layout, before anything else. */
export function readSchemaVersion(bytes: Uint8Array): number {
  return new BitReader(bytes).number();
}

/**
 * Reads a snapshot, or throws a `SnapshotLinkError`. The dictionary is the list of stations
 * that the link refers to by number.
 */
export function readSnapshot(
  bytes: Uint8Array,
  dictionary: readonly DictionaryStation[],
  cities: readonly DictionaryCity[],
): TripSnapshot {
  return new SnapshotReader(bytes, dictionary, cities).snapshot();
}

/**
 * Returns the minutes from leaving the departure place to reaching the arrival place, which
 * follow from the times of the rides and the ways before the first and after the last. The
 * writer in Rust computes the same number. Returns `null` when the times run backwards.
 */
function derivedDoorToDoorMinutes(journey: Journey): number | null {
  const lastRide = journey.later_rides.at(-1)?.ride ?? journey.first_ride;
  const departure = journey.first_ride.departure_time - journey.access.duration;
  const arrival =
    lastRide.arrival_time + (journey.arrival_footpath?.way.duration ?? 0) + journey.egress.duration;
  return arrival >= departure ? arrival - departure : null;
}

class SnapshotReader {
  private readonly bits: BitReader;
  /** The last time read, in minutes since the Unix epoch. */
  private clock = 0;
  /** The last point read, in microdegrees as latitude and longitude. */
  private latitude = 0;
  private longitude = 0;

  /** The last place read by its name, in steps of `placeStepMicrodegrees`. */
  private placeLatitudeSteps = 0;
  private placeLongitudeSteps = 0;

  constructor(
    bytes: Uint8Array,
    private readonly dictionary: readonly DictionaryStation[],
    private readonly cities: readonly DictionaryCity[],
  ) {
    this.bits = new BitReader(bytes);
  }

  snapshot(): TripSnapshot {
    const schemaVersion = this.bits.number();
    if (schemaVersion > supportedSchemaVersion) {
      throw new SnapshotLinkError(
        "newer-schema",
        `the trip was written with schema version ${schemaVersion}`,
        schemaVersion,
      );
    }
    const planned_at = dateFromDays(this.bits.number());
    const bundle_version = this.bits.boundedNumber(2 ** 32 - 1, "the bundle version");
    const { trip, startDays } = this.trip();

    const stationCount = this.bits.count();
    this.resetPoint();
    const table: LegPlanStation[] = [];
    for (let index = 0; index < stationCount; index += 1) {
      table.push(this.station());
    }

    let anchor = startDays * minutesPerDay;
    const legs: SnapshotLeg[] = [];
    for (const arrivalPlaceId of arrivalPlaceIds(trip)) {
      const status = this.bits.bits(2);
      if (status === 0) {
        continue;
      }
      if (status === 2) {
        const reason = noJourneyReasons[this.bits.bits(3)];
        if (reason === undefined) {
          throw malformed("an unknown reason for no journey");
        }
        legs.push({ arrival_place_id: arrivalPlaceId, result: { NoJourneyFound: reason } });
      } else if (status === 1) {
        const { plan, end } = this.legPlan(table, anchor);
        anchor = end;
        const result: LegPlanningResult = { Found: plan };
        legs.push({ arrival_place_id: arrivalPlaceId, result });
      } else {
        throw malformed("an unknown leg status");
      }
    }
    return { trip, legs, planned_at, bundle_version };
  }

  private resetPoint(): void {
    this.latitude = 0;
    this.longitude = 0;
  }

  /** Reads a time, which is a difference to the clock, and moves the clock to it. */
  private time(): number {
    const minutes = this.clock + this.bits.signedNumber();
    if (minutes < 0 || minutes > maximumUnixMinutes) {
      throw malformed("a time is out of range");
    }
    this.clock = minutes;
    return minutes;
  }

  /** Reads a point, which is a difference to the last point. */
  private coordinates(): { latitude: number; longitude: number } {
    const latitude = this.latitude + this.bits.signedNumber();
    const longitude = this.longitude + this.bits.signedNumber();
    if (Math.abs(latitude) > maximumMicrodegreesOfLatitude || Math.abs(longitude) > maximumMicrodegreesOfLongitude) {
      throw malformed("a coordinate is out of range");
    }
    this.latitude = latitude;
    this.longitude = longitude;
    return { latitude: latitude / 1_000_000, longitude: longitude / 1_000_000 };
  }

  /** Reads a text that is empty, in a vocabulary or a literal. */
  private vocabularyText(vocabulary: readonly string[]): string {
    const code = this.bits.number();
    if (code === 0) {
      return "";
    }
    if (code === 1) {
      return this.bits.text();
    }
    const entry = vocabulary[code - 2];
    if (entry === undefined) {
      throw malformed("a vocabulary entry is unknown");
    }
    return entry;
  }

  private window(): DailyTimeWindow {
    const start = this.bits.boundedNumber(minutesPerDay - 1, "a time of day");
    const length = this.bits.number();
    if (length < 1 || length > minutesPerDay) {
      throw malformed("a window length is out of range");
    }
    return { start, length };
  }

  private travelMode(): LegTravelMode {
    if (!this.bits.bit()) {
      return "Daytime";
    }
    const bedtime = this.bits.boundedNumber(minutesPerDay - 1, "a time of day");
    const wake_up_time = this.bits.boundedNumber(minutesPerDay - 1, "a time of day");
    const berths: AcceptedBerths[] = [
      { Either: { preferred: null } },
      { Either: { preferred: "Couchette" } },
      { Either: { preferred: "Sleeper" } },
      { Only: "Couchette" },
      { Only: "Sleeper" },
    ];
    const accepted_berths = berths[this.bits.bits(3)];
    if (accepted_berths === undefined) {
      throw malformed("an unknown berth choice");
    }
    return { Overnight: { bedtime, wake_up_time, accepted_berths } };
  }

  private stay(): PlaceStay {
    if (!this.bits.bit()) {
      const nights = this.bits.number();
      if (nights < 1 || nights > 255) {
        throw malformed("a stay has no nights");
      }
      return { Nights: nights };
    }
    const prefers_longer_stay = this.bits.bit();
    let stay_range_override = null;
    if (this.bits.bit()) {
      const minimum = this.bits.boundedNumber(65_535, "a stay");
      const maximum_beyond_minimum = this.bits.bit() ? this.bits.boundedNumber(65_535, "a stay") : null;
      stay_range_override = { minimum, maximum_beyond_minimum };
    }
    return { SameDay: { stay_range_override, prefers_longer_stay } };
  }

  private place(): TripPlace {
    const id = this.bits.number();
    if (id < 1 || id > 2 ** 32 - 1) {
      throw malformed("a place has no ID");
    }
    let name: string;
    let latitudeSteps: number;
    let longitudeSteps: number;
    const line = this.bits.number();
    if (line > 0) {
      const city = this.cities[line - 1];
      if (city === undefined) {
        // A newer viewer has the city, because the dictionary only grows.
        throw new SnapshotLinkError("newer-schema", "the link names a city that this viewer does not know");
      }
      name = city.name;
      latitudeSteps = stepsOf(city.latitude) + this.bits.signedNumber();
      longitudeSteps = stepsOf(city.longitude) + this.bits.signedNumber();
    } else {
      name = this.bits.text();
      latitudeSteps = this.placeLatitudeSteps + this.bits.signedNumber();
      longitudeSteps = this.placeLongitudeSteps + this.bits.signedNumber();
      this.placeLatitudeSteps = latitudeSteps;
      this.placeLongitudeSteps = longitudeSteps;
    }
    const latitude = latitudeSteps * placeStepMicrodegrees;
    const longitude = longitudeSteps * placeStepMicrodegrees;
    if (Math.abs(latitude) > maximumMicrodegreesOfLatitude || Math.abs(longitude) > maximumMicrodegreesOfLongitude) {
      throw malformed("a coordinate is out of range");
    }
    const coordinates = { latitude: latitude / 1_000_000, longitude: longitude / 1_000_000 };
    const station_search_radius = this.bits.number();
    if (station_search_radius < 100 || station_search_radius > 50_000) {
      throw malformed("a search radius is out of range");
    }
    let window_override: PlaceWindowOverride | null = null;
    switch (this.bits.bits(2)) {
      case 1:
        window_override = { Arrival: this.window() };
        break;
      case 2:
        window_override = { Departure: this.window() };
        break;
      case 3: {
        const departure = this.window();
        const arrival = this.window();
        window_override = { Both: { departure, arrival } };
        break;
      }
      default:
        break;
    }
    return { id, name, coordinates, station_search_radius, window_override };
  }

  /** Reads the trip and the day its start date falls on, which the times of the legs count from. */
  private trip(): { trip: Trip; startDays: number } {
    const name = this.bits.text();
    const startDays = this.bits.number();
    const start_date = dateFromDays(startDays);
    const travel_class = this.bits.bit() ? "First" : "Second";
    const departure = this.window();
    const arrival = this.window();
    const next_place_id = this.bits.number();
    if (next_place_id < 1 || next_place_id > 2 ** 32 - 1) {
      throw malformed("the next place ID is zero");
    }
    this.placeLatitudeSteps = 0;
    this.placeLongitudeSteps = 0;
    let itinerary: Itinerary;
    switch (this.bits.bits(2)) {
      case 0:
        itinerary = "Empty";
        break;
      case 1:
        itinerary = { OriginOnly: this.place() };
        break;
      case 2: {
        const origin = this.place();
        const viaCount = this.bits.count();
        const via_places = [];
        for (let index = 0; index < viaCount; index += 1) {
          const arriving_leg_travel_mode = this.travelMode();
          const place = this.place();
          const stay = this.stay();
          via_places.push({ arriving_leg_travel_mode, place, stay });
        }
        const final_leg_travel_mode = this.travelMode();
        const destination = this.place();
        itinerary = { Route: { origin, via_places, final_leg_travel_mode, destination } };
        break;
      }
      default:
        throw malformed("an unknown itinerary");
    }
    const trip: Trip = {
      id: snapshotTripId,
      name,
      start_date,
      itinerary,
      preferences: snapshotPreferences({ departure, arrival }, travel_class),
      next_place_id,
      chosen_journeys: [],
      leg_transfer_overrides: [],
    };
    return { trip, startDays };
  }

  private station(): LegPlanStation {
    const line = this.bits.number();
    if (line > 0) {
      const entry = this.dictionary[line - 1];
      if (entry === undefined) {
        // A newer viewer has the station, because the dictionary only grows.
        throw new SnapshotLinkError(
          "newer-schema",
          "the link names a station that this viewer does not know",
        );
      }
      const exact = this.bits.bit();
      const latitude = entry.latitude + (exact ? 0 : this.bits.signedNumber());
      const longitude = entry.longitude + (exact ? 0 : this.bits.signedNumber());
      if (Math.abs(latitude) > maximumMicrodegreesOfLatitude || Math.abs(longitude) > maximumMicrodegreesOfLongitude) {
        throw malformed("a coordinate is out of range");
      }
      return {
        name: entry.name,
        coordinates: { latitude: latitude / 1_000_000, longitude: longitude / 1_000_000 },
        time_zone: entry.timeZone,
      };
    }
    const name = this.bits.text();
    const coordinates = this.coordinates();
    const time_zone = this.vocabularyText(timeZones);
    return { name, coordinates, time_zone };
  }

  private way(): StationWay {
    const duration = this.bits.boundedNumber(65_535, "a way");
    const distance_meters = this.bits.boundedNumber(2 ** 32 - 1, "a distance");
    const mode = this.bits.bit() ? "LocalTransport" : "Walk";
    return { duration, distance_meters, mode };
  }

  /** Reads the index of a station of the leg, which has to point into its table. */
  private stationIndex(stationCount: number): number {
    const index = this.bits.number();
    if (index >= stationCount) {
      throw malformed("a station index is outside the table");
    }
    return index;
  }

  private ride(stationCount: number): TrainRide {
    const boarding_station = this.stationIndex(stationCount);
    const alighting_station = this.stationIndex(stationCount);
    const departure_time = this.time();
    const arrival_time = this.time();
    const category = this.vocabularyText(categories);
    const number = this.bits.bit() ? String(this.bits.number()) : this.bits.text();
    const operator = this.vocabularyText(operators);
    const offers_berths = this.bits.bit();
    return {
      boarding_station,
      departure_time,
      alighting_station,
      arrival_time,
      train_label: { category, number, operator, agency: "" },
      offers_berths,
      intermediate_station_coordinates: [],
      path_coordinates: [],
      layout_links: [],
    };
  }

  /** Reads a journey. The clock runs on from its first time. */
  private journey(stationCount: number): Journey {
    const access = this.way();
    const laterCount = this.bits.count();
    const first_ride = this.ride(stationCount);
    const later_rides: TransferAndRide[] = [];
    let previousArrival = first_ride.arrival_time;
    for (let index = 0; index < laterCount; index += 1) {
      const matches = this.bits.bit();
      const explicitTransfer = matches ? null : this.bits.boundedNumber(65_535, "a transfer");
      const ride = this.ride(stationCount);
      const transfer_duration = explicitTransfer ?? ride.departure_time - previousArrival;
      if (transfer_duration < 0 || transfer_duration > 65_535) {
        throw malformed("a transfer is out of range");
      }
      previousArrival = ride.arrival_time;
      later_rides.push({ transfer_duration, ride });
    }
    let arrival_footpath = null;
    if (this.bits.bit()) {
      const to_station = this.stationIndex(stationCount);
      arrival_footpath = { to_station, way: this.way() };
    }
    const egress = this.way();
    let sleeping_ride: number | null = null;
    if (this.bits.bit()) {
      sleeping_ride = this.bits.number();
      if (sleeping_ride > laterCount) {
        throw malformed("a sleeping ride is out of range");
      }
    }
    const kinds: (AlternativeKind | null)[] = [null, "Cheaper", "Faster"];
    const alternative_kind = kinds[this.bits.bits(2)];
    if (alternative_kind === undefined) {
      throw malformed("an unknown alternative kind");
    }
    let costParts: { explicitMinutes: number | null; price: EuroRange | null } | null = null;
    if (this.bits.bit()) {
      const explicitMinutes = this.bits.bit() ? null : this.bits.boundedNumber(2 ** 32 - 1, "a duration");
      let price = null;
      if (this.bits.bit()) {
        const median = this.bits.boundedNumber(2 ** 32 - 1, "a price");
        const below = this.bits.number();
        const above = this.bits.number();
        const low = median - below;
        const high = median + above;
        if (low < 0 || high > 2 ** 32 - 1) {
          throw malformed("a price is out of range");
        }
        price = { low, median, high };
      }
      costParts = { explicitMinutes, price };
    }
    const journey: Journey = {
      access,
      first_ride,
      later_rides,
      arrival_footpath,
      egress,
      sleeping_ride,
      cost_parts: null,
      alternative_kind,
    };
    if (costParts !== null) {
      const doorToDoor = costParts.explicitMinutes ?? derivedDoorToDoorMinutes(journey);
      if (doorToDoor === null) {
        throw malformed("the time door to door cannot be derived");
      }
      journey.cost_parts = snapshotCostParts(doorToDoor, costParts.price);
    }
    return journey;
  }

  /** Reads the plan of a leg, which counts its times from `anchor`, and the time it ends at. */
  private legPlan(table: readonly LegPlanStation[], anchor: number): { plan: LegPlan; end: number } {
    const follows_traveller_choice = this.bits.bit();
    const stationCount = this.bits.count();
    if (stationCount > maximumLegStations) {
      throw malformed("a leg has too many stations");
    }
    const station_table: LegPlanStation[] = [];
    for (let index = 0; index < stationCount; index += 1) {
      const station = table[this.bits.number()];
      if (station === undefined) {
        throw malformed("a station is outside the table");
      }
      station_table.push(station);
    }
    const alternativeCount = this.bits.count();
    this.clock = anchor;
    const chosen_journey = this.journey(stationCount);
    const end = this.clock;
    const alternative_journeys: Journey[] = [];
    for (let index = 0; index < alternativeCount; index += 1) {
      this.clock = chosen_journey.first_ride.departure_time;
      alternative_journeys.push(this.journey(stationCount));
    }
    return {
      plan: {
        chosen_journey,
        alternative_journeys,
        station_table,
        follows_traveller_choice,
        best_journey_cost: 0,
        pass_options: [],
        departure_window: null,
      },
      end,
    };
  }
}

/** Returns the arrival place of every leg of a trip in itinerary order. */
function arrivalPlaceIds(trip: Trip): number[] {
  const itinerary = trip.itinerary;
  if (itinerary === "Empty" || "OriginOnly" in itinerary) {
    return [];
  }
  const route = itinerary.Route;
  return [...route.via_places.map((via) => via.place.id), route.destination.id];
}
