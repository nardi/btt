//! The bit layout of a snapshot.
//!
//! The writer and the reader walk the snapshot in the same order. The web viewer has a reader
//! that follows the reader here, and a test checks that both read the same link the same way.
//!
//! Times travel as the difference to the previous time, in minutes, because the times of a trip
//! lie close together. The first time of a trip counts from the midnight (UTC) of its start
//! date. Coordinates travel as the difference in microdegrees to the previous point. A text that
//! repeats across trips travels as an index into a vocabulary, and every other text as bytes.

use std::collections::HashMap;
use std::num::{NonZeroU8, NonZeroU32};

use chrono::{Datelike, NaiveDate};
use smallvec::SmallVec;
use smol_str::SmolStr;
use uuid::Uuid;

use super::SnapshotLinkError;
use super::bits::{BitReader, BitWriter};
use super::vocabulary::{self, CATEGORIES, OPERATORS, TIME_ZONES};
use super::{cities, stations};
use crate::coordinates::{GeoCoordinates, Latitude, Longitude};
use crate::itinerary::{
    AcceptedBerths, BerthType, Itinerary, ItineraryRoute, LegTravelMode, OvernightLegOptions,
    PlaceStay, PlaceWindowOverride, SameDayStayRange, StationSearchRadius, TripPlace, TripPlaceId,
    ViaPlace,
};
use crate::plan::{
    AlternativeKind, ArrivalFootpath, Journey, JourneyCostParts, LegPlan, LegPlanStation,
    LegPlanStationIndex, LegPlanningResult, NoJourneyReason, StationWay, TrainLabel, TrainRide,
    TransferAndRide, WayToStationMode,
};
use crate::snapshot::{SNAPSHOT_SCHEMA_VERSION, SnapshotLeg, TripSnapshot};
use crate::tickets::{EuroCents, EuroRange};
use crate::time::{
    DailyTimeWindow, DailyWindowLength, DurationMinutes, LocalTimeOfDay, TravelDayWindows,
    UnixTimeMinutes,
};
use crate::trip::{TravelClass, Trip, TripId, TripPreferences};

/// Days from 0001-01-01 (day 1) to 1970-01-01, which `NaiveDate` counts from.
const DAYS_FROM_COMMON_ERA_TO_UNIX_EPOCH: i32 = 719_163;

/// Most stations one leg plan may have, because journeys point at them with a byte.
const MAXIMUM_LEG_STATIONS: usize = 256;

/// The reasons for no journey in the order of their codes. Reasons are only appended.
const NO_JOURNEY_REASONS: [NoJourneyReason; 5] = [
    NoJourneyReason::NoStationNearDeparturePlace,
    NoJourneyReason::NoStationNearArrivalPlace,
    NoJourneyReason::NoConnectionInWindow,
    NoJourneyReason::BackendUnavailable,
    NoJourneyReason::TravelDateOutsideTimetable,
];

/// How a leg appears in the data.
const LEG_MISSING: u32 = 0;
/// A leg with journeys.
const LEG_FOUND: u32 = 1;
/// A leg without journeys, followed by the reason.
const LEG_NONE: u32 = 2;

/// Writes a snapshot as bits.
pub(super) fn write_snapshot(snapshot: &TripSnapshot) -> Result<Vec<u8>, SnapshotLinkError> {
    let mut writer = SnapshotWriter {
        bits: BitWriter::default(),
        clock: 0,
        coordinates: (0, 0),
        place_steps: (0, 0),
    };
    writer.bits.number(u64::from(SNAPSHOT_SCHEMA_VERSION));
    writer.bits.number(days_since_epoch(snapshot.planned_at)?);
    writer.bits.number(u64::from(snapshot.bundle_version));
    writer.trip(&snapshot.trip)?;

    // The stations of all legs share one table, because neighbouring legs share stations.
    let mut table: Vec<&LegPlanStation> = Vec::new();
    let mut table_index: HashMap<&LegPlanStation, usize> = HashMap::new();
    let mut leg_stations: Vec<Vec<usize>> = Vec::new();
    for leg in &snapshot.legs {
        let mut stations = Vec::new();
        if let LegPlanningResult::Found(plan) = &leg.result {
            if plan.station_table.len() > MAXIMUM_LEG_STATIONS {
                return Err(SnapshotLinkError::TooLarge);
            }
            for station in &plan.station_table {
                let index = *table_index.entry(station).or_insert_with(|| {
                    table.push(station);
                    table.len() - 1
                });
                stations.push(index);
            }
        }
        leg_stations.push(stations);
    }
    writer.bits.number(table.len() as u64);
    writer.coordinates = (0, 0);
    for station in &table {
        writer.station(station);
    }

    // Every leg of the itinerary has a status, in itinerary order.
    let mut anchor = writer.trip_clock_base(&snapshot.trip)?;
    for itinerary_leg in snapshot.trip.itinerary.legs() {
        let position = snapshot
            .legs
            .iter()
            .position(|leg| leg.arrival_place_id == itinerary_leg.arrival_place.id);
        let Some(position) = position else {
            writer.bits.bits(LEG_MISSING, 2);
            continue;
        };
        match &snapshot.legs[position].result {
            LegPlanningResult::NoJourneyFound(reason) => {
                writer.bits.bits(LEG_NONE, 2);
                let code = NO_JOURNEY_REASONS
                    .iter()
                    .position(|candidate| candidate == reason)
                    .unwrap_or(0);
                writer.bits.bits(u32::try_from(code).unwrap_or(0), 3);
            }
            LegPlanningResult::Found(plan) => {
                writer.bits.bits(LEG_FOUND, 2);
                anchor = writer.leg_plan(plan, &leg_stations[position], anchor);
            }
        }
    }
    Ok(writer.bits.finish())
}

/// Reads a snapshot from bits.
pub(super) fn read_snapshot(bytes: &[u8]) -> Result<TripSnapshot, SnapshotLinkError> {
    let mut reader = SnapshotReader {
        bits: BitReader::new(bytes),
        clock: 0,
        coordinates: (0, 0),
        place_steps: (0, 0),
    };
    let schema_version = reader.bits.number_u16()?;
    if schema_version > SNAPSHOT_SCHEMA_VERSION {
        return Err(SnapshotLinkError::NewerSchema(schema_version));
    }
    let planned_at = date_from_days(reader.bits.number()?)?;
    let bundle_version = reader.bits.number_u32()?;
    let trip = reader.trip()?;

    let station_count = reader.bits.count()?;
    reader.coordinates = (0, 0);
    let mut table = Vec::with_capacity(station_count);
    for _ in 0..station_count {
        table.push(reader.station()?);
    }

    let mut anchor = reader.trip_clock_base(&trip)?;
    let mut legs = Vec::new();
    for itinerary_leg in trip.itinerary.legs() {
        let status = reader.bits.bits(2)?;
        let arrival_place_id = itinerary_leg.arrival_place.id;
        match status {
            LEG_MISSING => {}
            LEG_NONE => {
                let code = reader.bits.bits(3)? as usize;
                let reason = *NO_JOURNEY_REASONS
                    .get(code)
                    .ok_or(SnapshotLinkError::Malformed(
                        "an unknown reason for no journey",
                    ))?;
                legs.push(SnapshotLeg {
                    arrival_place_id,
                    result: LegPlanningResult::NoJourneyFound(reason),
                });
            }
            LEG_FOUND => {
                let (plan, next_anchor) = reader.leg_plan(&table, anchor)?;
                anchor = next_anchor;
                legs.push(SnapshotLeg {
                    arrival_place_id,
                    result: LegPlanningResult::Found(Box::new(plan)),
                });
            }
            _ => return Err(SnapshotLinkError::Malformed("an unknown leg status")),
        }
    }
    Ok(TripSnapshot {
        trip,
        legs,
        planned_at,
        bundle_version,
    })
}

/// Returns the number of days from 1970-01-01 to a date.
fn days_since_epoch(date: NaiveDate) -> Result<u64, SnapshotLinkError> {
    u64::try_from(date.num_days_from_ce() - DAYS_FROM_COMMON_ERA_TO_UNIX_EPOCH)
        .map_err(|_| SnapshotLinkError::Malformed("a date lies before 1970"))
}

/// Returns the date a number of days after 1970-01-01.
fn date_from_days(days: u64) -> Result<NaiveDate, SnapshotLinkError> {
    i32::try_from(days)
        .ok()
        .and_then(|days| days.checked_add(DAYS_FROM_COMMON_ERA_TO_UNIX_EPOCH))
        .and_then(NaiveDate::from_num_days_from_ce_opt)
        .ok_or(SnapshotLinkError::Malformed("a date is out of range"))
}

/// Returns the minute since the Unix epoch at which the start date of a trip begins in UTC.
fn clock_base_minutes(trip: &Trip) -> Result<i64, SnapshotLinkError> {
    Ok(i64::try_from(days_since_epoch(trip.start_date)?)
        .map_err(|_| SnapshotLinkError::Malformed("a date is out of range"))?
        * 1440)
}

/// Returns the minutes from leaving the departure place to reaching the arrival place, which
/// follow from the times of the rides and the ways before the first and after the last. The
/// planner computes the same number as the time door to door of the cost parts. Returns `None`
/// when the times run backwards.
fn derived_door_to_door_minutes(journey: &Journey) -> Option<u32> {
    let departure =
        i64::from(journey.first_ride.departure_time.0) - i64::from(journey.access.duration.0);
    let footpath = journey
        .arrival_footpath
        .map_or(0, |footpath| i64::from(footpath.way.duration.0));
    let arrival = i64::from(journey.last_ride().arrival_time.0)
        + footpath
        + i64::from(journey.egress.duration.0);
    u32::try_from(arrival - departure).ok()
}

/// Returns whether a text is a number without leading zeros that fits in 9 digits.
fn is_plain_number(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 9
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'))
}

/// Writes the parts of a snapshot.
struct SnapshotWriter {
    /// The bits written so far.
    bits: BitWriter,
    /// The last time written, in minutes since the Unix epoch.
    clock: i64,
    /// The last point written, in microdegrees as latitude and longitude.
    coordinates: (i32, i32),
    /// The last place written by its name, in steps of `PLACE_STEP_MICRODEGREES`.
    place_steps: (i64, i64),
}

impl SnapshotWriter {
    /// Returns the time from which the first leg's times count, and starts the clock there.
    fn trip_clock_base(&mut self, trip: &Trip) -> Result<i64, SnapshotLinkError> {
        let base = clock_base_minutes(trip)?;
        self.clock = base;
        Ok(base)
    }

    /// Writes a time as the difference to the clock, and moves the clock to it.
    fn time(&mut self, time: UnixTimeMinutes) {
        let minutes = i64::from(time.0);
        self.bits.signed_number(minutes - self.clock);
        self.clock = minutes;
    }

    /// Writes a point as the difference to the last point.
    fn coordinates(&mut self, coordinates: GeoCoordinates) {
        let latitude = coordinates.latitude.microdegrees();
        let longitude = coordinates.longitude.microdegrees();
        self.bits
            .signed_number(i64::from(latitude) - i64::from(self.coordinates.0));
        self.bits
            .signed_number(i64::from(longitude) - i64::from(self.coordinates.1));
        self.coordinates = (latitude, longitude);
    }

    /// Writes a text that is empty, in a vocabulary or a literal.
    fn vocabulary_text(&mut self, vocabulary: &[&str], text: &str) {
        if text.is_empty() {
            self.bits.number(0);
        } else if let Some(index) = vocabulary::index_of(vocabulary, text) {
            self.bits.number(index as u64 + 2);
        } else {
            self.bits.number(1);
            self.bits.text(text);
        }
    }

    /// Writes a daily time window.
    fn window(&mut self, window: DailyTimeWindow) {
        self.bits
            .number(u64::from(window.start.minutes_since_midnight()));
        self.bits.number(u64::from(window.length.minutes()));
    }

    /// Writes how a leg travels.
    fn travel_mode(&mut self, travel_mode: LegTravelMode) {
        match travel_mode {
            LegTravelMode::Daytime => self.bits.bit(false),
            LegTravelMode::Overnight(options) => {
                self.bits.bit(true);
                self.bits
                    .number(u64::from(options.bedtime.minutes_since_midnight()));
                self.bits
                    .number(u64::from(options.wake_up_time.minutes_since_midnight()));
                self.bits.bits(
                    match options.accepted_berths {
                        AcceptedBerths::Either { preferred: None } => 0,
                        AcceptedBerths::Either {
                            preferred: Some(BerthType::Couchette),
                        } => 1,
                        AcceptedBerths::Either {
                            preferred: Some(BerthType::Sleeper),
                        } => 2,
                        AcceptedBerths::Only(BerthType::Couchette) => 3,
                        AcceptedBerths::Only(BerthType::Sleeper) => 4,
                    },
                    3,
                );
            }
        }
    }

    /// Writes how long the traveller stays at a via place.
    fn stay(&mut self, stay: PlaceStay) {
        match stay {
            PlaceStay::Nights(nights) => {
                self.bits.bit(false);
                self.bits.number(u64::from(nights.get()));
            }
            PlaceStay::SameDay {
                stay_range_override,
                prefers_longer_stay,
            } => {
                self.bits.bit(true);
                self.bits.bit(prefers_longer_stay);
                self.bits.bit(stay_range_override.is_some());
                if let Some(range) = stay_range_override {
                    self.bits.number(u64::from(range.minimum.0));
                    self.bits.bit(range.maximum_beyond_minimum.is_some());
                    if let Some(beyond) = range.maximum_beyond_minimum {
                        self.bits.number(u64::from(beyond.0));
                    }
                }
            }
        }
    }

    /// Writes a place.
    fn place(&mut self, place: &TripPlace) {
        self.bits.number(u64::from(place.id.0.get()));
        if let Some(found) = cities::dictionary().find(place) {
            // The name is the city's, and the position is the difference to the city's.
            self.bits.number(found.index as u64 + 1);
            self.bits.signed_number(found.latitude_offset);
            self.bits.signed_number(found.longitude_offset);
        } else {
            let latitude = cities::steps_of(i64::from(place.coordinates.latitude.microdegrees()));
            let longitude = cities::steps_of(i64::from(place.coordinates.longitude.microdegrees()));
            self.bits.number(0);
            self.bits.text(&place.name);
            self.bits.signed_number(latitude - self.place_steps.0);
            self.bits.signed_number(longitude - self.place_steps.1);
            self.place_steps = (latitude, longitude);
        }
        self.bits
            .number(u64::from(place.station_search_radius.meters()));
        match place.window_override {
            None => self.bits.bits(0, 2),
            Some(PlaceWindowOverride::Arrival(window)) => {
                self.bits.bits(1, 2);
                self.window(window);
            }
            Some(PlaceWindowOverride::Departure(window)) => {
                self.bits.bits(2, 2);
                self.window(window);
            }
            Some(PlaceWindowOverride::Both(windows)) => {
                self.bits.bits(3, 2);
                self.window(windows.departure);
                self.window(windows.arrival);
            }
        }
    }

    /// Writes the trip without the parts a snapshot leaves out.
    fn trip(&mut self, trip: &Trip) -> Result<(), SnapshotLinkError> {
        self.bits.text(&trip.name);
        self.bits.number(days_since_epoch(trip.start_date)?);
        self.bits
            .bit(trip.preferences.travel_class == TravelClass::First);
        self.window(trip.preferences.default_windows.departure);
        self.window(trip.preferences.default_windows.arrival);
        self.bits.number(u64::from(trip.next_place_id.get()));
        self.place_steps = (0, 0);
        match &trip.itinerary {
            Itinerary::Empty => self.bits.bits(0, 2),
            Itinerary::OriginOnly(origin) => {
                self.bits.bits(1, 2);
                self.place(origin);
            }
            Itinerary::Route(route) => {
                self.bits.bits(2, 2);
                self.place(&route.origin);
                self.bits.number(route.via_places.len() as u64);
                for via_place in &route.via_places {
                    self.travel_mode(via_place.arriving_leg_travel_mode);
                    self.place(&via_place.place);
                    self.stay(via_place.stay);
                }
                self.travel_mode(route.final_leg_travel_mode);
                self.place(&route.destination);
            }
        }
        Ok(())
    }

    /// Writes a station of the shared table: the number of its line in the dictionary and how
    /// far it lies from the entry, or else its name, point and time zone.
    fn station(&mut self, station: &LegPlanStation) {
        if let Some(found) = stations::dictionary().find(station) {
            self.bits.number(found.index as u64 + 1);
            let exact = found.latitude_offset == 0 && found.longitude_offset == 0;
            self.bits.bit(exact);
            if !exact {
                self.bits.signed_number(i64::from(found.latitude_offset));
                self.bits.signed_number(i64::from(found.longitude_offset));
            }
            return;
        }
        self.bits.number(0);
        self.bits.text(&station.name);
        self.coordinates(station.coordinates);
        self.vocabulary_text(TIME_ZONES, &station.time_zone);
    }

    /// Writes a way to or from a station.
    fn way(&mut self, way: StationWay) {
        self.bits.number(u64::from(way.duration.0));
        self.bits.number(u64::from(way.distance_meters));
        self.bits.bit(way.mode == WayToStationMode::LocalTransport);
    }

    /// Writes one ride and the times of it.
    fn ride(&mut self, ride: &TrainRide) {
        self.bits.number(u64::from(ride.boarding_station.0));
        self.bits.number(u64::from(ride.alighting_station.0));
        self.time(ride.departure_time);
        self.time(ride.arrival_time);
        self.vocabulary_text(CATEGORIES, &ride.train_label.category);
        if is_plain_number(&ride.train_label.number) {
            self.bits.bit(true);
            // The check above leaves at most nine digits.
            self.bits
                .number(ride.train_label.number.parse::<u64>().unwrap_or(0));
        } else {
            self.bits.bit(false);
            self.bits.text(&ride.train_label.number);
        }
        self.vocabulary_text(OPERATORS, &ride.train_label.operator);
        self.bits.bit(ride.offers_berths);
    }

    /// Writes a journey. The clock runs on from its first time.
    fn journey(&mut self, journey: &Journey) {
        self.way(journey.access);
        self.bits.number(journey.later_rides.len() as u64);
        self.ride(&journey.first_ride);
        let mut previous_arrival = i64::from(journey.first_ride.arrival_time.0);
        for later in &journey.later_rides {
            let transfer = i64::from(later.ride.departure_time.0) - previous_arrival;
            let matches = transfer == i64::from(later.transfer_duration.0);
            self.bits.bit(matches);
            if !matches {
                self.bits.number(u64::from(later.transfer_duration.0));
            }
            self.ride(&later.ride);
            previous_arrival = i64::from(later.ride.arrival_time.0);
        }
        self.bits.bit(journey.arrival_footpath.is_some());
        if let Some(footpath) = &journey.arrival_footpath {
            self.bits.number(u64::from(footpath.to_station.0));
            self.way(footpath.way);
        }
        self.way(journey.egress);
        self.bits.bit(journey.sleeping_ride.is_some());
        if let Some(position) = journey.sleeping_ride {
            self.bits.number(u64::from(position));
        }
        self.bits.bits(
            match journey.alternative_kind {
                None => 0,
                Some(AlternativeKind::Cheaper) => 1,
                Some(AlternativeKind::Faster) => 2,
            },
            2,
        );
        self.bits.bit(journey.cost_parts.is_some());
        if let Some(parts) = &journey.cost_parts {
            // The time door to door follows from the times and the ways in nearly every journey,
            // so a bit stands in for it.
            let derived = derived_door_to_door_minutes(journey) == Some(parts.door_to_door_minutes);
            self.bits.bit(derived);
            if !derived {
                self.bits.number(u64::from(parts.door_to_door_minutes));
            }
            self.bits.bit(parts.price.is_some());
            if let Some(price) = parts.price {
                self.bits.number(u64::from(price.median().0));
                self.bits
                    .number(u64::from(price.median().0 - price.low().0));
                self.bits
                    .number(u64::from(price.high().0 - price.median().0));
            }
        }
    }

    /// Writes the plan of a leg. The times of the leg count from `anchor`, the time the
    /// previous leg ended at. Returns the time this leg ends at.
    fn leg_plan(&mut self, plan: &LegPlan, stations: &[usize], anchor: i64) -> i64 {
        self.bits.bit(plan.follows_traveller_choice);
        self.bits.number(stations.len() as u64);
        for station in stations {
            self.bits.number(*station as u64);
        }
        self.bits.number(plan.alternative_journeys.len() as u64);
        self.clock = anchor;
        self.journey(&plan.chosen_journey);
        let end = self.clock;
        for alternative in &plan.alternative_journeys {
            self.clock = i64::from(plan.chosen_journey.first_ride.departure_time.0);
            self.journey(alternative);
        }
        end
    }
}

/// Reads the parts of a snapshot.
struct SnapshotReader<'a> {
    /// The bits left to read.
    bits: BitReader<'a>,
    /// The last time read, in minutes since the Unix epoch.
    clock: i64,
    /// The last point read, in microdegrees as latitude and longitude.
    coordinates: (i32, i32),
    /// The last place read by its name, in steps of `PLACE_STEP_MICRODEGREES`.
    place_steps: (i64, i64),
}

impl SnapshotReader<'_> {
    /// Returns the time from which the first leg's times count, and starts the clock there.
    fn trip_clock_base(&mut self, trip: &Trip) -> Result<i64, SnapshotLinkError> {
        let base = clock_base_minutes(trip)?;
        self.clock = base;
        Ok(base)
    }

    /// Reads a time, which is a difference to the clock, and moves the clock to it.
    fn time(&mut self) -> Result<UnixTimeMinutes, SnapshotLinkError> {
        let minutes = self.clock + self.bits.signed_number()?;
        let time = u32::try_from(minutes)
            .map_err(|_| SnapshotLinkError::Malformed("a time is out of range"))?;
        self.clock = minutes;
        Ok(UnixTimeMinutes(time))
    }

    /// Reads a point, which is a difference to the last point.
    fn coordinates(&mut self) -> Result<GeoCoordinates, SnapshotLinkError> {
        let out_of_range = |_| SnapshotLinkError::Malformed("a coordinate is out of range");
        let latitude = i64::from(self.coordinates.0) + self.bits.signed_number()?;
        let longitude = i64::from(self.coordinates.1) + self.bits.signed_number()?;
        let latitude = i32::try_from(latitude).map_err(out_of_range)?;
        let longitude = i32::try_from(longitude).map_err(out_of_range)?;
        let coordinates = GeoCoordinates {
            latitude: Latitude::from_microdegrees(latitude)
                .map_err(|_| SnapshotLinkError::Malformed("a coordinate is out of range"))?,
            longitude: Longitude::from_microdegrees(longitude)
                .map_err(|_| SnapshotLinkError::Malformed("a coordinate is out of range"))?,
        };
        self.coordinates = (latitude, longitude);
        Ok(coordinates)
    }

    /// Reads a text that is empty, in a vocabulary or a literal.
    fn vocabulary_text(&mut self, vocabulary: &[&str]) -> Result<SmolStr, SnapshotLinkError> {
        match self.bits.number()? {
            0 => Ok(SmolStr::default()),
            1 => Ok(SmolStr::from(self.bits.text()?)),
            index => usize::try_from(index - 2)
                .ok()
                .and_then(|index| vocabulary.get(index))
                .map(|entry| SmolStr::from(*entry))
                .ok_or(SnapshotLinkError::Malformed(
                    "a vocabulary entry is unknown",
                )),
        }
    }

    /// Reads a daily time window.
    fn window(&mut self) -> Result<DailyTimeWindow, SnapshotLinkError> {
        let invalid = |_| SnapshotLinkError::Malformed("a time window is out of range");
        let start = LocalTimeOfDay::new(self.bits.number_u16()?).map_err(invalid)?;
        let length = DailyWindowLength::new(self.bits.number_u16()?).map_err(invalid)?;
        Ok(DailyTimeWindow { start, length })
    }

    /// Reads how a leg travels.
    fn travel_mode(&mut self) -> Result<LegTravelMode, SnapshotLinkError> {
        if !self.bits.bit()? {
            return Ok(LegTravelMode::Daytime);
        }
        let invalid = |_| SnapshotLinkError::Malformed("a time of day is out of range");
        let bedtime = LocalTimeOfDay::new(self.bits.number_u16()?).map_err(invalid)?;
        let wake_up_time = LocalTimeOfDay::new(self.bits.number_u16()?).map_err(invalid)?;
        let accepted_berths = match self.bits.bits(3)? {
            0 => AcceptedBerths::Either { preferred: None },
            1 => AcceptedBerths::Either {
                preferred: Some(BerthType::Couchette),
            },
            2 => AcceptedBerths::Either {
                preferred: Some(BerthType::Sleeper),
            },
            3 => AcceptedBerths::Only(BerthType::Couchette),
            4 => AcceptedBerths::Only(BerthType::Sleeper),
            _ => return Err(SnapshotLinkError::Malformed("an unknown berth choice")),
        };
        Ok(LegTravelMode::Overnight(OvernightLegOptions {
            bedtime,
            wake_up_time,
            accepted_berths,
        }))
    }

    /// Reads how long the traveller stays at a via place.
    fn stay(&mut self) -> Result<PlaceStay, SnapshotLinkError> {
        if !self.bits.bit()? {
            let nights = u8::try_from(self.bits.number()?)
                .ok()
                .and_then(NonZeroU8::new)
                .ok_or(SnapshotLinkError::Malformed("a stay has no nights"))?;
            return Ok(PlaceStay::Nights(nights));
        }
        let prefers_longer_stay = self.bits.bit()?;
        let stay_range_override = if self.bits.bit()? {
            let minimum = DurationMinutes(self.bits.number_u16()?);
            let maximum_beyond_minimum = if self.bits.bit()? {
                Some(DurationMinutes(self.bits.number_u16()?))
            } else {
                None
            };
            Some(SameDayStayRange {
                minimum,
                maximum_beyond_minimum,
            })
        } else {
            None
        };
        Ok(PlaceStay::SameDay {
            stay_range_override,
            prefers_longer_stay,
        })
    }

    /// Reads a place.
    fn place(&mut self) -> Result<TripPlace, SnapshotLinkError> {
        let id = NonZeroU32::new(self.bits.number_u32()?)
            .map(TripPlaceId)
            .ok_or(SnapshotLinkError::Malformed("a place has no ID"))?;
        let line = self.bits.number()?;
        let (name, latitude_steps, longitude_steps) = if line > 0 {
            let city = usize::try_from(line - 1)
                .ok()
                .and_then(|index| cities::dictionary().get(index))
                .ok_or(SnapshotLinkError::Malformed(
                    "a place names a city that is not in the dictionary",
                ))?;
            (
                SmolStr::from(city.name),
                city.latitude_steps() + self.bits.signed_number()?,
                city.longitude_steps() + self.bits.signed_number()?,
            )
        } else {
            let name = SmolStr::from(self.bits.text()?);
            let latitude = self.place_steps.0 + self.bits.signed_number()?;
            let longitude = self.place_steps.1 + self.bits.signed_number()?;
            self.place_steps = (latitude, longitude);
            (name, latitude, longitude)
        };
        let out_of_range = || SnapshotLinkError::Malformed("a coordinate is out of range");
        let step = crate::snapshot::PLACE_STEP_MICRODEGREES;
        let coordinates = GeoCoordinates {
            latitude: i32::try_from(latitude_steps * step)
                .ok()
                .and_then(|microdegrees| Latitude::from_microdegrees(microdegrees).ok())
                .ok_or_else(out_of_range)?,
            longitude: i32::try_from(longitude_steps * step)
                .ok()
                .and_then(|microdegrees| Longitude::from_microdegrees(microdegrees).ok())
                .ok_or_else(out_of_range)?,
        };
        let station_search_radius = StationSearchRadius::new(self.bits.number_u16()?)
            .map_err(|_| SnapshotLinkError::Malformed("a search radius is out of range"))?;
        let window_override = match self.bits.bits(2)? {
            0 => None,
            1 => Some(PlaceWindowOverride::Arrival(self.window()?)),
            2 => Some(PlaceWindowOverride::Departure(self.window()?)),
            _ => {
                let departure = self.window()?;
                let arrival = self.window()?;
                Some(PlaceWindowOverride::Both(TravelDayWindows {
                    departure,
                    arrival,
                }))
            }
        };
        Ok(TripPlace {
            id,
            name,
            coordinates,
            station_search_radius,
            window_override,
        })
    }

    /// Reads the trip.
    fn trip(&mut self) -> Result<Trip, SnapshotLinkError> {
        let name = SmolStr::from(self.bits.text()?);
        let start_date = date_from_days(self.bits.number()?)?;
        let travel_class = if self.bits.bit()? {
            TravelClass::First
        } else {
            TravelClass::Second
        };
        let departure = self.window()?;
        let arrival = self.window()?;
        let next_place_id = NonZeroU32::new(self.bits.number_u32()?)
            .ok_or(SnapshotLinkError::Malformed("the next place ID is zero"))?;
        self.place_steps = (0, 0);
        let itinerary = match self.bits.bits(2)? {
            0 => Itinerary::Empty,
            1 => Itinerary::OriginOnly(self.place()?),
            2 => {
                let origin = self.place()?;
                let via_count = self.bits.count()?;
                let mut via_places = Vec::with_capacity(via_count);
                for _ in 0..via_count {
                    let arriving_leg_travel_mode = self.travel_mode()?;
                    let place = self.place()?;
                    let stay = self.stay()?;
                    via_places.push(ViaPlace {
                        arriving_leg_travel_mode,
                        place,
                        stay,
                    });
                }
                let final_leg_travel_mode = self.travel_mode()?;
                let destination = self.place()?;
                Itinerary::Route(ItineraryRoute {
                    origin,
                    via_places,
                    final_leg_travel_mode,
                    destination,
                })
            }
            _ => return Err(SnapshotLinkError::Malformed("an unknown itinerary")),
        };
        Ok(Trip {
            id: TripId(Uuid::nil()),
            name,
            start_date,
            itinerary,
            preferences: TripPreferences {
                default_windows: TravelDayWindows { departure, arrival },
                travel_class,
                ..TripPreferences::default()
            },
            next_place_id,
            chosen_journeys: Vec::new(),
            leg_transfer_overrides: Vec::new(),
        })
    }

    /// Reads a station of the shared table.
    fn station(&mut self) -> Result<LegPlanStation, SnapshotLinkError> {
        let line = self.bits.number()?;
        if line > 0 {
            let entry = usize::try_from(line - 1)
                .ok()
                .and_then(|index| stations::dictionary().get(index))
                .ok_or(SnapshotLinkError::Malformed(
                    "a station is not in the dictionary",
                ))?;
            let (latitude_offset, longitude_offset) = if self.bits.bit()? {
                (0, 0)
            } else {
                (self.bits.signed_number()?, self.bits.signed_number()?)
            };
            let out_of_range = || SnapshotLinkError::Malformed("a coordinate is out of range");
            let latitude = i32::try_from(i64::from(entry.latitude) + latitude_offset)
                .map_err(|_| out_of_range())?;
            let longitude = i32::try_from(i64::from(entry.longitude) + longitude_offset)
                .map_err(|_| out_of_range())?;
            return Ok(LegPlanStation {
                name: SmolStr::from(entry.name),
                coordinates: GeoCoordinates {
                    latitude: Latitude::from_microdegrees(latitude).map_err(|_| out_of_range())?,
                    longitude: Longitude::from_microdegrees(longitude)
                        .map_err(|_| out_of_range())?,
                },
                time_zone: SmolStr::from(entry.time_zone),
            });
        }
        let name = SmolStr::from(self.bits.text()?);
        let coordinates = self.coordinates()?;
        let time_zone = self.vocabulary_text(TIME_ZONES)?;
        Ok(LegPlanStation {
            name,
            coordinates,
            time_zone,
        })
    }

    /// Reads a way to or from a station.
    fn way(&mut self) -> Result<StationWay, SnapshotLinkError> {
        let duration = DurationMinutes(self.bits.number_u16()?);
        let distance_meters = self.bits.number_u32()?;
        let mode = if self.bits.bit()? {
            WayToStationMode::LocalTransport
        } else {
            WayToStationMode::Walk
        };
        Ok(StationWay {
            duration,
            distance_meters,
            mode,
        })
    }

    /// Reads the index of a station of the leg, which has to point into its table.
    fn station_index(
        &mut self,
        station_count: usize,
    ) -> Result<LegPlanStationIndex, SnapshotLinkError> {
        let index = usize::try_from(self.bits.number()?)
            .map_err(|_| SnapshotLinkError::Malformed("a station index is too large"))?;
        if index >= station_count {
            return Err(SnapshotLinkError::Malformed(
                "a station index is outside the table",
            ));
        }
        // The table holds at most 256 stations, so the index fits in a byte.
        #[allow(clippy::cast_possible_truncation)]
        Ok(LegPlanStationIndex(index as u8))
    }

    /// Reads one ride and the times of it.
    fn ride(&mut self, station_count: usize) -> Result<TrainRide, SnapshotLinkError> {
        let boarding_station = self.station_index(station_count)?;
        let alighting_station = self.station_index(station_count)?;
        let departure_time = self.time()?;
        let arrival_time = self.time()?;
        let category = self.vocabulary_text(CATEGORIES)?;
        let number = if self.bits.bit()? {
            SmolStr::from(self.bits.number()?.to_string())
        } else {
            SmolStr::from(self.bits.text()?)
        };
        let operator = self.vocabulary_text(OPERATORS)?;
        let offers_berths = self.bits.bit()?;
        Ok(TrainRide {
            boarding_station,
            departure_time,
            alighting_station,
            arrival_time,
            train_label: TrainLabel {
                category,
                number,
                operator,
                agency: SmolStr::default(),
            },
            offers_berths,
            intermediate_station_coordinates: SmallVec::new(),
            path_coordinates: Vec::new(),
            layout_links: Vec::new(),
        })
    }

    /// Reads a price range, which is its median and the distances from the median to both ends.
    fn price(&mut self) -> Result<EuroRange, SnapshotLinkError> {
        let out_of_range = || SnapshotLinkError::Malformed("a price is out of range");
        let median = self.bits.number_u32()?;
        let below = self.bits.number_u32()?;
        let above = self.bits.number_u32()?;
        let low = median.checked_sub(below).ok_or_else(out_of_range)?;
        let high = median.checked_add(above).ok_or_else(out_of_range)?;
        EuroRange::new(EuroCents(low), EuroCents(median), EuroCents(high))
            .map_err(|_| out_of_range())
    }

    /// Reads a journey. The clock runs on from its first time.
    fn journey(&mut self, station_count: usize) -> Result<Journey, SnapshotLinkError> {
        let access = self.way()?;
        let later_count = self.bits.count()?;
        let first_ride = self.ride(station_count)?;
        let mut later_rides: SmallVec<[TransferAndRide; 3]> = SmallVec::new();
        let mut previous_arrival = i64::from(first_ride.arrival_time.0);
        for _ in 0..later_count {
            let matches = self.bits.bit()?;
            let explicit_transfer = if matches {
                None
            } else {
                Some(self.bits.number_u16()?)
            };
            let ride = self.ride(station_count)?;
            let transfer = match explicit_transfer {
                Some(minutes) => minutes,
                None => u16::try_from(i64::from(ride.departure_time.0) - previous_arrival)
                    .map_err(|_| SnapshotLinkError::Malformed("a transfer is out of range"))?,
            };
            previous_arrival = i64::from(ride.arrival_time.0);
            later_rides.push(TransferAndRide {
                transfer_duration: DurationMinutes(transfer),
                ride,
            });
        }
        let arrival_footpath = if self.bits.bit()? {
            let to_station = self.station_index(station_count)?;
            let way = self.way()?;
            Some(ArrivalFootpath { to_station, way })
        } else {
            None
        };
        let egress = self.way()?;
        let sleeping_ride = if self.bits.bit()? {
            let position = u8::try_from(self.bits.number()?)
                .map_err(|_| SnapshotLinkError::Malformed("a sleeping ride is out of range"))?;
            if usize::from(position) > later_count {
                return Err(SnapshotLinkError::Malformed(
                    "a sleeping ride is out of range",
                ));
            }
            Some(position)
        } else {
            None
        };
        let alternative_kind = match self.bits.bits(2)? {
            0 => None,
            1 => Some(AlternativeKind::Cheaper),
            2 => Some(AlternativeKind::Faster),
            _ => return Err(SnapshotLinkError::Malformed("an unknown alternative kind")),
        };
        let cost_parts = if self.bits.bit()? {
            let explicit_minutes = if self.bits.bit()? {
                None
            } else {
                Some(self.bits.number_u32()?)
            };
            let price = if self.bits.bit()? {
                Some(self.price()?)
            } else {
                None
            };
            Some((explicit_minutes, price))
        } else {
            None
        };
        let mut journey = Journey {
            access,
            first_ride,
            later_rides,
            arrival_footpath,
            egress,
            sleeping_ride,
            cost_parts: None,
            alternative_kind,
        };
        if let Some((explicit_minutes, price)) = cost_parts {
            let door_to_door_minutes = match explicit_minutes {
                Some(minutes) => minutes,
                None => derived_door_to_door_minutes(&journey).ok_or(
                    SnapshotLinkError::Malformed("the time door to door cannot be derived"),
                )?,
            };
            journey.cost_parts = Some(JourneyCostParts {
                door_to_door_minutes,
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
            });
        }
        Ok(journey)
    }

    /// Reads the plan of a leg. The times of the leg count from `anchor`, the time the previous
    /// leg ended at. Returns the plan and the time this leg ends at.
    fn leg_plan(
        &mut self,
        table: &[LegPlanStation],
        anchor: i64,
    ) -> Result<(LegPlan, i64), SnapshotLinkError> {
        let follows_traveller_choice = self.bits.bit()?;
        let station_count = self.bits.count()?;
        if station_count > MAXIMUM_LEG_STATIONS {
            return Err(SnapshotLinkError::Malformed("a leg has too many stations"));
        }
        let mut station_table = SmallVec::new();
        for _ in 0..station_count {
            let index = usize::try_from(self.bits.number()?)
                .map_err(|_| SnapshotLinkError::Malformed("a station index is too large"))?;
            station_table.push(
                table
                    .get(index)
                    .ok_or(SnapshotLinkError::Malformed(
                        "a station is outside the table",
                    ))?
                    .clone(),
            );
        }
        let alternative_count = self.bits.count()?;
        self.clock = anchor;
        let chosen_journey = self.journey(station_count)?;
        let end = self.clock;
        let mut alternative_journeys = SmallVec::new();
        for _ in 0..alternative_count {
            self.clock = i64::from(chosen_journey.first_ride.departure_time.0);
            alternative_journeys.push(self.journey(station_count)?);
        }
        Ok((
            LegPlan {
                chosen_journey,
                alternative_journeys,
                station_table,
                follows_traveller_choice,
                best_journey_cost: 0,
                pass_options: SmallVec::new(),
                departure_window: None,
            },
            end,
        ))
    }
}
