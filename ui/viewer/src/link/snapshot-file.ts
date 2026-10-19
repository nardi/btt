// Reads the JSON of a trip file, the `.btt` file the desktop app exports.
//
// A file is input from anyone, like a link. The reader checks every field against the type it
// has, drops fields it does not know, and checks that the journeys only point at stations their
// leg has.

import type {
  AcceptedBerths,
  DailyTimeWindow,
  EuroRange,
  GeoCoordinates,
  Itinerary,
  Journey,
  LayoutLink,
  LegPlan,
  LegPlanStation,
  LegPlanningResult,
  LegTravelMode,
  NoJourneyReason,
  PlaceStay,
  PlaceWindowOverride,
  SnapshotLeg,
  StationWay,
  TrainLabel,
  TrainRide,
  TransferAndRide,
  TravelDayWindows,
  TripPlace,
  TripSnapshot,
} from "@btt/shared/bindings";
import { SnapshotLinkError, malformed } from "./errors";
import { snapshotCostParts, snapshotPreferences, snapshotTripId } from "./snapshot-defaults";
import { supportedSchemaVersion } from "./snapshot-reader";
import {
  type Parser,
  boolean,
  date,
  decimal,
  integer,
  list,
  nullable,
  object,
  oneOf,
  text,
  variants,
} from "./validate";

/** Largest trip file the viewer reads, in characters. */
export const maximumFileCharacters = 8_000_000;

const unsigned32 = integer(0, 2 ** 32 - 1);
const minutes = integer(0, 65_535);
const timeOfDay = integer(0, 1439);

const coordinates: Parser<GeoCoordinates> = object<GeoCoordinates>({
  latitude: decimal(-90, 90),
  longitude: decimal(-180, 180),
});

const window: Parser<DailyTimeWindow> = object<DailyTimeWindow>({ start: timeOfDay, length: integer(1, 1440) });

const windows: Parser<TravelDayWindows> = object<TravelDayWindows>({ departure: window, arrival: window });

const place: Parser<TripPlace> = object<TripPlace>({
  id: integer(1, 2 ** 32 - 1),
  name: text,
  coordinates,
  station_search_radius: integer(100, 50_000),
  window_override: nullable(
    variants<PlaceWindowOverride>([], { Arrival: window, Departure: window, Both: windows }),
  ),
});

const berthType = oneOf("Couchette", "Sleeper");

const acceptedBerths = variants<AcceptedBerths>([], {
  Only: berthType,
  Either: object<{ preferred: "Couchette" | "Sleeper" | null }>({ preferred: nullable(berthType) }),
});

const travelMode = variants<LegTravelMode>(["Daytime"], {
  Overnight: object({ bedtime: timeOfDay, wake_up_time: timeOfDay, accepted_berths: acceptedBerths }),
});

const stay = variants<PlaceStay>([], {
  Nights: integer(1, 255),
  SameDay: object({
    stay_range_override: nullable(object({ minimum: minutes, maximum_beyond_minimum: nullable(minutes) })),
    prefers_longer_stay: boolean,
  }),
});

const itinerary = variants<Itinerary>(["Empty"], {
  OriginOnly: place,
  Route: object({
    origin: place,
    via_places: list(object({ arriving_leg_travel_mode: travelMode, place, stay })),
    final_leg_travel_mode: travelMode,
    destination: place,
  }),
});

const station: Parser<LegPlanStation> = object<LegPlanStation>({ name: text, coordinates, time_zone: text });

const stationIndex = integer(0, 255);

const way: Parser<StationWay> = object<StationWay>({
  duration: minutes,
  distance_meters: unsigned32,
  mode: oneOf("Walk", "LocalTransport"),
});

/** A text that a file may leave out, which the app names with an empty text. */
const optionalText: Parser<string> = (value, path) => (value === undefined ? "" : text(value, path));

/** Leaves out the agency of a train, which a snapshot does not carry. */
const noText: Parser<string> = () => "";

const trainLabel: Parser<TrainLabel> = object<TrainLabel>({
  category: text,
  number: text,
  operator: optionalText,
  agency: noText,
});

/** Leaves out the geometry of a train, which a snapshot does not carry. */
const noCoordinates: Parser<GeoCoordinates[]> = () => [];

/** Leaves out the layout links of a train, which a snapshot does not carry. */
const noLayoutLinks: Parser<LayoutLink[]> = () => [];

const ride: Parser<TrainRide> = object<TrainRide>({
  boarding_station: stationIndex,
  departure_time: unsigned32,
  alighting_station: stationIndex,
  arrival_time: unsigned32,
  train_label: trainLabel,
  offers_berths: boolean,
  intermediate_station_coordinates: noCoordinates,
  path_coordinates: noCoordinates,
  layout_links: noLayoutLinks,
});

const laterRide: Parser<TransferAndRide> = object<TransferAndRide>({ transfer_duration: minutes, ride });

const euroRange: Parser<EuroRange> = (value, path) => {
  const range = object<EuroRange>({ low: unsigned32, median: unsigned32, high: unsigned32 })(value, path);
  if (range.low > range.median || range.median > range.high) {
    throw malformed(`${path} has its ends out of order`);
  }
  return range;
};

/** Reads the cost parts of a journey, of which a snapshot keeps the time and the price. */
const costParts: Parser<Journey["cost_parts"]> = (value, path) => {
  if (value === null || value === undefined) {
    return null;
  }
  const kept = object<{ door_to_door_minutes: number; price: EuroRange | null }>({
    door_to_door_minutes: unsigned32,
    price: nullable(euroRange),
  })(value, path);
  return snapshotCostParts(kept.door_to_door_minutes, kept.price);
};

const journey: Parser<Journey> = object<Journey>({
  access: way,
  first_ride: ride,
  later_rides: list(laterRide),
  arrival_footpath: nullable(object({ to_station: stationIndex, way })),
  egress: way,
  sleeping_ride: nullable(integer(0, 255)),
  cost_parts: costParts,
  alternative_kind: nullable(oneOf("Cheaper", "Faster")),
});

const noJourneyReason = oneOf<NoJourneyReason>(
  "NoStationNearDeparturePlace",
  "NoStationNearArrivalPlace",
  "NoConnectionInWindow",
  "BackendUnavailable",
  "TravelDateOutsideTimetable",
);

/** Reads the plan of a leg, of which a snapshot keeps the journeys and their stations. */
const legPlan: Parser<LegPlan> = (value, path) => {
  const kept = object({
    chosen_journey: journey,
    alternative_journeys: list(journey),
    station_table: list(station),
    follows_traveller_choice: boolean,
  })(value, path);
  const stationCount = kept.station_table.length;
  if (stationCount > 256) {
    throw malformed(`${path} has too many stations`);
  }
  for (const candidate of [kept.chosen_journey, ...kept.alternative_journeys]) {
    checkStations(candidate, stationCount, path);
  }
  return {
    ...kept,
    best_journey_cost: 0,
    pass_options: [],
    departure_window: null,
  };
};

/** Checks that a journey only points at stations of its table. */
function checkStations(candidate: Journey, stationCount: number, path: string): void {
  const rides = [candidate.first_ride, ...candidate.later_rides.map((later) => later.ride)];
  const indices = [
    ...rides.flatMap((ride) => [ride.boarding_station, ride.alighting_station]),
    ...(candidate.arrival_footpath === null ? [] : [candidate.arrival_footpath.to_station]),
  ];
  if (indices.some((index) => index >= stationCount)) {
    throw malformed(`${path} points at a station outside its table`);
  }
  if ((candidate.sleeping_ride ?? 0) >= rides.length) {
    throw malformed(`${path} has a sleeping ride that does not exist`);
  }
}

const legResult = variants<LegPlanningResult>([], { Found: legPlan, NoJourneyFound: noJourneyReason });

const snapshotLeg: Parser<SnapshotLeg> = object<SnapshotLeg>({
  arrival_place_id: integer(1, 2 ** 32 - 1),
  result: legResult,
});

/** Reads the trip of a snapshot, which has no ID and keeps only a few preferences. */
const trip: Parser<TripSnapshot["trip"]> = (value, path) => {
  const kept = object({
    name: text,
    start_date: date,
    itinerary,
    preferences: object({ default_windows: windows, travel_class: oneOf("First", "Second") }),
    next_place_id: integer(1, 2 ** 32 - 1),
  })(value, path);
  return {
    id: snapshotTripId,
    name: kept.name,
    start_date: kept.start_date,
    itinerary: kept.itinerary,
    preferences: snapshotPreferences(kept.preferences.default_windows, kept.preferences.travel_class),
    next_place_id: kept.next_place_id,
    chosen_journeys: [],
  };
};

const snapshot: Parser<TripSnapshot> = object<TripSnapshot>({
  trip,
  legs: list(snapshotLeg),
  planned_at: date,
  bundle_version: unsigned32,
});

/**
 * Reads the contents of a trip file. Throws a `SnapshotLinkError` when the file is not JSON,
 * comes from a newer version of the app or does not hold a trip.
 */
export function parseSnapshotFile(contents: string): TripSnapshot {
  if (contents.length > maximumFileCharacters) {
    throw new SnapshotLinkError("too-large", "the file is larger than the viewer reads");
  }
  let document: unknown;
  try {
    document = JSON.parse(contents);
  } catch {
    throw malformed("the file is not JSON");
  }
  const version = object({ schema_version: integer(0, 65_535) })(document, "the file");
  if (version.schema_version > supportedSchemaVersion) {
    throw new SnapshotLinkError(
      "newer-schema",
      `the trip was written with schema version ${version.schema_version}`,
      version.schema_version,
    );
  }
  return object({ snapshot })(document, "the file").snapshot;
}
