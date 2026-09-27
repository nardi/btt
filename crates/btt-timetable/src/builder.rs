//! Assembles a `TimetableBundle` from stations, footpaths and trips.
//!
//! The timetable builder CLI and the router tests both use it, so tests run on bundles with the
//! same layout the app loads.

use std::num::NonZeroU32;

use btt_model::GeoCoordinates;
use chrono::NaiveDate;
use rustc_hash::FxHashMap;
use thiserror::Error;

use crate::format::{
    BUNDLE_FORMAT_VERSION, BUNDLE_MAGIC, BundleHeader, BundleStation, BundleStationIndex,
    BundleTrainLabel, BundleTrip, CompressedSparseRows, FeedAttribution, Footpath,
    MAXIMUM_VALIDITY_DAYS, MinutesSinceServiceDateUtc, PackedPatternStop, PooledString,
    RaptorRoute, RaptorRouteIndex, RouteStopPosition, ServiceDayBitset, ServiceDayBitsetIndex,
    StopEventTimes, TimeZoneIndex, TimetableBundle, TrainLabelIndex,
};

/// Minutes in one day.
const MINUTES_PER_DAY: u16 = 1440;

/// Errors from assembling a bundle.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BundleBuildError {
    /// The validity window is empty or longer than a service day bitset covers.
    #[error("the validity window must last 1 to {MAXIMUM_VALIDITY_DAYS} days, not {0}")]
    InvalidValidityWindow(u16),
    /// A trip has fewer than two stops.
    #[error("trip {0} has fewer than two stops")]
    TooFewStops(usize),
    /// A trip departs from a stop before it arrives, or arrives at a stop before it left the
    /// previous one.
    #[error("trip {0} goes back in time")]
    TimesGoBackwards(usize),
    /// A trip's first departure is not on its service date in UTC.
    #[error("trip {0} does not depart on its service date")]
    FirstDepartureOutsideServiceDate(usize),
    /// A trip refers to a station that was not added.
    #[error("trip {0} refers to an unknown station")]
    UnknownStation(usize),
    /// A trip has more stops than a route can hold.
    #[error("trip {0} has more stops than a route can hold")]
    TooManyStops(usize),
    /// A trip runs on no day of the validity window.
    #[error("trip {0} runs on no day of the validity window")]
    NoServiceDays(usize),
    /// A string is longer than a pooled string can hold.
    #[error("a string is longer than {} bytes", u16::MAX)]
    StringTooLong,
    /// The bundle has more items than its indices can count.
    #[error("the bundle has more {0} than its indices can count")]
    TooManyItems(&'static str),
}

/// A station to add to a bundle.
#[derive(Debug, Clone, PartialEq)]
pub struct StationInput {
    /// Name shown to the traveller.
    pub name: String,
    /// Location of the station.
    pub coordinates: GeoCoordinates,
    /// IANA time zone, such as "Europe/Vienna".
    pub time_zone: String,
    /// Station ID in the Trainline stations dataset, if matched.
    pub trainline_station_id: Option<NonZeroU32>,
}

/// One stop of a trip to add to a bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TripStopInput {
    /// The station, as returned by `BundleBuilder::add_station`.
    pub station: BundleStationIndex,
    /// Arrival in minutes since 00:00 UTC on the service date.
    pub arrival: u16,
    /// Departure in minutes since 00:00 UTC on the service date.
    pub departure: u16,
    /// Whether passengers may board here.
    pub boarding_allowed: bool,
    /// Whether passengers may alight here.
    pub alighting_allowed: bool,
}

/// How a train is shown to the traveller.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrainLabelInput {
    /// Product or category, such as "ICE".
    pub category: String,
    /// Train number within the category.
    pub number: String,
    /// Name of the operating agency.
    pub agency: String,
}

/// A trip to add to a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripInput {
    /// Stops in travel order.
    pub stops: Vec<TripStopInput>,
    /// Days of the validity window on which the trip departs from its first stop.
    pub service_days: ServiceDayBitset,
    /// How the train is shown to the traveller.
    pub train_label: TrainLabelInput,
}

/// Source and license of a feed to list in the bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedAttributionInput {
    /// Name of the feed.
    pub feed_name: String,
    /// License of the feed.
    pub license: String,
    /// Where the feed or its license is published.
    pub url: String,
}

/// Collects stations, footpaths and trips and assembles them into a bundle.
pub struct BundleBuilder {
    /// First day of the validity window.
    validity_start: NaiveDate,
    /// Length of the validity window.
    validity_days: u16,
    /// Version of the data as `yyyymmddNN`.
    bundle_version: u32,
    /// Stations in the order they were added.
    stations: Vec<StationInput>,
    /// Footpaths as departure station, arrival station and minutes.
    footpaths: Vec<(BundleStationIndex, Footpath)>,
    /// Trips in the order they were added.
    trips: Vec<TripInput>,
    /// Feeds to list in the bundle.
    attributions: Vec<FeedAttributionInput>,
}

impl BundleBuilder {
    /// Starts a bundle for a validity window.
    ///
    /// # Errors
    ///
    /// Returns an error when the window is empty or longer than `MAXIMUM_VALIDITY_DAYS`.
    pub fn new(
        validity_start: NaiveDate,
        validity_days: u16,
        bundle_version: u32,
    ) -> Result<Self, BundleBuildError> {
        if validity_days == 0 || validity_days > MAXIMUM_VALIDITY_DAYS {
            return Err(BundleBuildError::InvalidValidityWindow(validity_days));
        }
        Ok(Self {
            validity_start,
            validity_days,
            bundle_version,
            stations: Vec::new(),
            footpaths: Vec::new(),
            trips: Vec::new(),
            attributions: Vec::new(),
        })
    }

    /// Adds a station and returns its index.
    ///
    /// # Errors
    ///
    /// Returns an error when the bundle already has as many stations as a pattern stop can
    /// address.
    pub fn add_station(
        &mut self,
        station: StationInput,
    ) -> Result<BundleStationIndex, BundleBuildError> {
        let index = u32::try_from(self.stations.len())
            .ok()
            .filter(|index| *index <= PackedPatternStop::MAXIMUM_STATION_INDEX)
            .ok_or(BundleBuildError::TooManyItems("stations"))?;
        self.stations.push(station);
        Ok(BundleStationIndex(index))
    }

    /// Adds a one-way footpath between two stations.
    pub fn add_footpath(
        &mut self,
        from_station: BundleStationIndex,
        to_station: BundleStationIndex,
        duration_minutes: u16,
    ) {
        self.footpaths.push((
            from_station,
            Footpath {
                to_station,
                duration_minutes,
            },
        ));
    }

    /// Adds a trip. The trip is checked when the bundle is built.
    pub fn add_trip(&mut self, trip: TripInput) {
        self.trips.push(trip);
    }

    /// Lists a feed in the bundle's attributions.
    pub fn add_attribution(&mut self, attribution: FeedAttributionInput) {
        self.attributions.push(attribution);
    }

    /// Checks the trips and assembles the bundle.
    ///
    /// Trips with the same stop pattern go into RAPTOR routes, sorted by first departure and
    /// split wherever one trip would overtake another.
    ///
    /// # Errors
    ///
    /// Returns an error when a trip is invalid or the bundle outgrows its indices.
    pub fn build(self) -> Result<TimetableBundle, BundleBuildError> {
        for (trip_number, trip) in self.trips.iter().enumerate() {
            self.check_trip(trip_number, trip)?;
        }
        let mut tables = BundleTables::new(self.stations.len());
        for station in &self.stations {
            tables.add_station(station)?;
        }
        for trip_numbers in split_into_raptor_routes(&self.trips) {
            tables.add_route(&self.trips, &trip_numbers)?;
        }
        let mut footpaths_by_station: Vec<Vec<Footpath>> = vec![Vec::new(); self.stations.len()];
        for (from_station, footpath) in &self.footpaths {
            if let Some(footpaths) = footpaths_by_station.get_mut(from_station.as_usize()) {
                footpaths.push(*footpath);
            }
        }
        let feed_attributions = self
            .attributions
            .iter()
            .map(|attribution| {
                Ok(FeedAttribution {
                    feed_name: tables.strings.intern(&attribution.feed_name)?,
                    license: tables.strings.intern(&attribution.license)?,
                    url: tables.strings.intern(&attribution.url)?,
                })
            })
            .collect::<Result<_, BundleBuildError>>()?;

        let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or_default();
        let validity_start_days_since_epoch =
            i32::try_from((self.validity_start - epoch).num_days())
                .map_err(|_| BundleBuildError::InvalidValidityWindow(self.validity_days))?;
        Ok(TimetableBundle {
            header: BundleHeader {
                magic: BUNDLE_MAGIC,
                format_version: BUNDLE_FORMAT_VERSION,
                bundle_version: self.bundle_version,
                validity_start_days_since_epoch,
                validity_days: self.validity_days,
            },
            stations: tables.stations,
            time_zone_names: tables.time_zone_names,
            route_stops_by_station: compressed_sparse_rows(tables.route_stops_by_station)?,
            footpaths_by_station: compressed_sparse_rows(footpaths_by_station)?,
            service_days_by_station: tables.service_days_by_station,
            raptor_routes: tables.raptor_routes,
            pattern_stops: tables.pattern_stops,
            trips: tables.trips,
            stop_events: tables.stop_events,
            distinct_service_days: tables.distinct_service_days,
            train_labels: tables.train_labels,
            string_pool: tables.strings.text,
            feed_attributions,
        })
    }

    /// Checks that a trip has at least two known stops, runs on at least one day and never
    /// goes back in time.
    fn check_trip(&self, trip_number: usize, trip: &TripInput) -> Result<(), BundleBuildError> {
        let [first, ..] = trip.stops.as_slice() else {
            return Err(BundleBuildError::TooFewStops(trip_number));
        };
        if trip.stops.len() < 2 {
            return Err(BundleBuildError::TooFewStops(trip_number));
        }
        if trip.stops.len() > usize::from(u16::MAX) {
            return Err(BundleBuildError::TooManyStops(trip_number));
        }
        if first.departure >= MINUTES_PER_DAY {
            return Err(BundleBuildError::FirstDepartureOutsideServiceDate(
                trip_number,
            ));
        }
        if trip.service_days.is_empty() {
            return Err(BundleBuildError::NoServiceDays(trip_number));
        }
        let mut previous_departure = 0;
        for stop in &trip.stops {
            if stop.station.as_usize() >= self.stations.len() {
                return Err(BundleBuildError::UnknownStation(trip_number));
            }
            if stop.departure < stop.arrival || stop.arrival < previous_departure {
                return Err(BundleBuildError::TimesGoBackwards(trip_number));
            }
            previous_departure = stop.departure;
        }
        Ok(())
    }
}

/// Returns the service days shifted forward, for a stop a trip reaches after midnight.
fn shifted_service_days(service_days: ServiceDayBitset, days: u16) -> ServiceDayBitset {
    let mut shifted = ServiceDayBitset::default();
    for day in 0..MAXIMUM_VALIDITY_DAYS {
        if service_days.runs_on(day) {
            shifted.insert(day + days);
        }
    }
    shifted
}

/// Packs a trip stop into a pattern stop.
fn pattern_stop(stop: &TripStopInput) -> Option<PackedPatternStop> {
    PackedPatternStop::new(stop.station, stop.boarding_allowed, stop.alighting_allowed)
}

/// Groups trips by stop pattern and splits each group into routes where no trip overtakes
/// another. Returns the trip numbers of each route, sorted by first departure.
fn split_into_raptor_routes(trips: &[TripInput]) -> Vec<Vec<usize>> {
    let mut trips_by_pattern = FxHashMap::<Vec<PackedPatternStop>, Vec<usize>>::default();
    for (trip_number, trip) in trips.iter().enumerate() {
        let pattern = trip.stops.iter().filter_map(pattern_stop).collect();
        trips_by_pattern
            .entry(pattern)
            .or_default()
            .push(trip_number);
    }
    let mut patterns: Vec<_> = trips_by_pattern.into_iter().collect();
    // Sorting makes the bundle the same for the same input, whatever the hash map order.
    patterns.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));

    let mut routes = Vec::new();
    for (_, mut trip_numbers) in patterns {
        trip_numbers.sort_by_key(|&trip_number| {
            let stops = &trips[trip_number].stops;
            (
                stops.iter().map(|stop| stop.departure).collect::<Vec<_>>(),
                trip_number,
            )
        });
        let mut pattern_routes: Vec<Vec<usize>> = Vec::new();
        for trip_number in trip_numbers {
            let fitting_route = pattern_routes.iter_mut().find(|route| {
                route
                    .last()
                    .is_some_and(|&last| never_overtakes(&trips[last], &trips[trip_number]))
            });
            match fitting_route {
                Some(route) => route.push(trip_number),
                None => pattern_routes.push(vec![trip_number]),
            }
        }
        routes.extend(pattern_routes);
    }
    routes
}

/// Returns whether `later` arrives and departs no earlier than `earlier` at every stop.
fn never_overtakes(earlier: &TripInput, later: &TripInput) -> bool {
    earlier
        .stops
        .iter()
        .zip(&later.stops)
        .all(|(earlier_stop, later_stop)| {
            later_stop.arrival >= earlier_stop.arrival
                && later_stop.departure >= earlier_stop.departure
        })
}

/// Converts a list of rows into compressed sparse rows.
fn compressed_sparse_rows<T>(
    rows: Vec<Vec<T>>,
) -> Result<CompressedSparseRows<T>, BundleBuildError> {
    let mut row_offsets = Vec::with_capacity(rows.len() + 1);
    let mut values = Vec::new();
    row_offsets.push(0);
    for row in rows {
        values.extend(row);
        row_offsets.push(to_u32(values.len(), "rows")?);
    }
    Ok(CompressedSparseRows {
        row_offsets,
        values,
    })
}

/// Converts a count to `u32`, naming what overflowed on failure.
fn to_u32(count: usize, what: &'static str) -> Result<u32, BundleBuildError> {
    u32::try_from(count).map_err(|_| BundleBuildError::TooManyItems(what))
}

/// Collects strings into one text and hands out their positions. Equal strings share a
/// position.
#[derive(Default)]
struct StringPool {
    /// All strings, one after another.
    text: String,
    /// Position of each string already in `text`.
    positions: FxHashMap<String, PooledString>,
}

impl StringPool {
    /// Adds a string once and returns its position.
    fn intern(&mut self, value: &str) -> Result<PooledString, BundleBuildError> {
        if let Some(position) = self.positions.get(value) {
            return Ok(*position);
        }
        let position = PooledString {
            byte_offset: to_u32(self.text.len(), "string bytes")?,
            byte_length: u16::try_from(value.len()).map_err(|_| BundleBuildError::StringTooLong)?,
        };
        self.text.push_str(value);
        self.positions.insert(value.to_owned(), position);
        Ok(position)
    }
}

/// The bundle tables while they are being filled, with the lookups that deduplicate values.
struct BundleTables<'input> {
    /// Text of all pooled strings.
    strings: StringPool,
    /// Stations in bundle order.
    stations: Vec<BundleStation>,
    /// Each time zone name once.
    time_zone_names: Vec<PooledString>,
    /// Index of each time zone name in `time_zone_names`.
    time_zone_indices: FxHashMap<&'input str, TimeZoneIndex>,
    /// Routes in bundle order.
    raptor_routes: Vec<RaptorRoute>,
    /// Stop patterns of all routes.
    pattern_stops: Vec<PackedPatternStop>,
    /// Trips, route after route.
    trips: Vec<BundleTrip>,
    /// Times of all trips, trip-major.
    stop_events: Vec<StopEventTimes>,
    /// Each service day bitset once.
    distinct_service_days: Vec<ServiceDayBitset>,
    /// Index of each bitset in `distinct_service_days`.
    service_day_indices: FxHashMap<ServiceDayBitset, ServiceDayBitsetIndex>,
    /// Each train label once.
    train_labels: Vec<BundleTrainLabel>,
    /// Index of each label in `train_labels`.
    train_label_indices: FxHashMap<&'input TrainLabelInput, TrainLabelIndex>,
    /// Days with any stop per station.
    service_days_by_station: Vec<ServiceDayBitset>,
    /// Route stops per station, before they become compressed sparse rows.
    route_stops_by_station: Vec<Vec<RouteStopPosition>>,
}

impl<'input> BundleTables<'input> {
    /// Creates empty tables for a number of stations.
    fn new(station_count: usize) -> Self {
        Self {
            strings: StringPool::default(),
            stations: Vec::with_capacity(station_count),
            time_zone_names: Vec::new(),
            time_zone_indices: FxHashMap::default(),
            raptor_routes: Vec::new(),
            pattern_stops: Vec::new(),
            trips: Vec::new(),
            stop_events: Vec::new(),
            distinct_service_days: Vec::new(),
            service_day_indices: FxHashMap::default(),
            train_labels: Vec::new(),
            train_label_indices: FxHashMap::default(),
            service_days_by_station: vec![ServiceDayBitset::default(); station_count],
            route_stops_by_station: vec![Vec::new(); station_count],
        }
    }

    /// Adds a station, with its time zone name stored once.
    fn add_station(&mut self, station: &'input StationInput) -> Result<(), BundleBuildError> {
        let time_zone = if let Some(index) = self.time_zone_indices.get(station.time_zone.as_str())
        {
            *index
        } else {
            let index = TimeZoneIndex(
                u16::try_from(self.time_zone_names.len())
                    .map_err(|_| BundleBuildError::TooManyItems("time zones"))?,
            );
            self.time_zone_names
                .push(self.strings.intern(&station.time_zone)?);
            self.time_zone_indices.insert(&station.time_zone, index);
            index
        };
        self.stations.push(BundleStation {
            name: self.strings.intern(&station.name)?,
            latitude_microdegrees: station.coordinates.latitude.microdegrees(),
            longitude_microdegrees: station.coordinates.longitude.microdegrees(),
            time_zone,
            trainline_station_id: station.trainline_station_id,
        });
        Ok(())
    }

    /// Adds one RAPTOR route with its stop pattern, trips and stop events.
    fn add_route(
        &mut self,
        all_trips: &'input [TripInput],
        trip_numbers: &[usize],
    ) -> Result<(), BundleBuildError> {
        let raptor_route = RaptorRouteIndex(to_u32(self.raptor_routes.len(), "routes")?);
        let first_trip_number = trip_numbers[0];
        let first_trip = &all_trips[first_trip_number];
        let longest_trip_minutes = trip_numbers
            .iter()
            .filter_map(|&trip_number| all_trips[trip_number].stops.last())
            .map(|stop| stop.arrival)
            .max()
            .unwrap_or(0);
        self.raptor_routes.push(RaptorRoute {
            first_pattern_stop_index: to_u32(self.pattern_stops.len(), "pattern stops")?,
            stop_count: u16::try_from(first_trip.stops.len())
                .map_err(|_| BundleBuildError::TooManyStops(first_trip_number))?,
            first_trip_index: to_u32(self.trips.len(), "trips")?,
            trip_count: to_u32(trip_numbers.len(), "trips")?,
            first_stop_event_index: to_u32(self.stop_events.len(), "stop events")?,
            longest_trip_days: u8::try_from(longest_trip_minutes.div_ceil(MINUTES_PER_DAY))
                .unwrap_or(u8::MAX),
        });
        for (position, stop) in first_trip.stops.iter().enumerate() {
            self.pattern_stops
                .push(pattern_stop(stop).ok_or(BundleBuildError::TooManyItems("stations"))?);
            self.route_stops_by_station[stop.station.as_usize()].push(RouteStopPosition {
                raptor_route,
                stop_position: u16::try_from(position)
                    .map_err(|_| BundleBuildError::TooManyStops(first_trip_number))?,
            });
        }
        for &trip_number in trip_numbers {
            self.add_trip(&all_trips[trip_number])?;
        }
        Ok(())
    }

    /// Adds one trip and its stop events, and marks its days at every station it stops at.
    fn add_trip(&mut self, trip: &'input TripInput) -> Result<(), BundleBuildError> {
        let service_days = self.service_days_index(trip.service_days)?;
        let train_label = self.train_label_index(&trip.train_label)?;
        self.trips.push(BundleTrip {
            service_days,
            train_label,
        });
        for stop in &trip.stops {
            self.stop_events.push(StopEventTimes {
                arrival: MinutesSinceServiceDateUtc(stop.arrival),
                departure: MinutesSinceServiceDateUtc(stop.departure),
            });
            let station_days = &mut self.service_days_by_station[stop.station.as_usize()];
            *station_days = station_days.union(shifted_service_days(
                trip.service_days,
                stop.departure / MINUTES_PER_DAY,
            ));
        }
        Ok(())
    }

    /// Returns the index of a service day bitset, adding it on first use.
    fn service_days_index(
        &mut self,
        service_days: ServiceDayBitset,
    ) -> Result<ServiceDayBitsetIndex, BundleBuildError> {
        if let Some(index) = self.service_day_indices.get(&service_days) {
            return Ok(*index);
        }
        let index = ServiceDayBitsetIndex(to_u32(self.distinct_service_days.len(), "calendars")?);
        self.distinct_service_days.push(service_days);
        self.service_day_indices.insert(service_days, index);
        Ok(index)
    }

    /// Returns the index of a train label, adding it on first use.
    fn train_label_index(
        &mut self,
        label: &'input TrainLabelInput,
    ) -> Result<TrainLabelIndex, BundleBuildError> {
        if let Some(index) = self.train_label_indices.get(label) {
            return Ok(*index);
        }
        let index = TrainLabelIndex(to_u32(self.train_labels.len(), "train labels")?);
        self.train_labels.push(BundleTrainLabel {
            category: self.strings.intern(&label.category)?,
            number: self.strings.intern(&label.number)?,
            agency: self.strings.intern(&label.agency)?,
        });
        self.train_label_indices.insert(label, index);
        Ok(index)
    }
}
