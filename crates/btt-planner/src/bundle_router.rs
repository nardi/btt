//! A backend that plans legs with the RAPTOR router on a timetable bundle.

use std::num::NonZeroU8;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use btt_geo::{TimeZoneFinder, local_time_to_unix_minutes};
use btt_model::{
    ArrivalFootpath, DailyTimeWindow, DurationMinutes, Journey, LegPlan, LegPlanStation,
    LegPlanStationIndex, LegPlanningResult, LegTravelMode, NoJourneyReason, TrainLabel, TrainRide,
    TransferAndRide, UnixTimeMinutes,
};
use btt_router::{
    BoardAnyTrip, CandidateStation, MinutesSinceBundleStart, ParetoJourneySet, RaptorJourney,
    RaptorLegQuery, RaptorSearchBuffers, SearchIndex, search_leg_journeys,
};
use btt_timetable::{BundleLoadError, BundleStationIndex, LoadedTimetableBundle, StationIndex};
use chrono::NaiveDate;
use smallvec::SmallVec;

use crate::input::{LegEndpoint, LegPlanningInput};
use crate::scoring::{JourneyCostInputs, ScoringWeights, journey_cost};

/// Largest number of candidate stations per place, closest first.
const MAXIMUM_CANDIDATE_STATIONS: usize = 16;

/// Largest number of trains in one journey.
const MAXIMUM_TRAIN_RIDES: NonZeroU8 = NonZeroU8::new(5).unwrap();

/// Largest number of alternatives a leg plan shows next to the chosen journey.
const MAXIMUM_ALTERNATIVES: usize = 4;

/// Minutes in one day.
const MINUTES_PER_DAY: u32 = 1440;

/// Number of parts the departure window splits into, each searched on its own thread.
const PARALLEL_WINDOW_SLICES: u32 = 4;

/// Shortest part of the departure window worth a thread of its own, in minutes.
const SHORTEST_WINDOW_SLICE_MINUTES: u32 = 60;

/// Plans legs on a loaded bundle.
pub struct TimetableBundleRouter {
    /// The loaded bundle.
    bundle: LoadedTimetableBundle,
    /// Finds stations near a place.
    station_index: StationIndex,
    /// Lower bounds on travel times, which let the search skip hopeless stations.
    search_index: SearchIndex,
    /// Resolves the time zone of places for their local time windows.
    time_zone_finder: Arc<TimeZoneFinder>,
    /// The bundle's first day, in minutes since the Unix epoch.
    bundle_start_unix_minutes: u32,
    /// Search buffers of finished searches, reused by later ones to avoid allocating.
    spare_search_buffers: Mutex<Vec<RaptorSearchBuffers>>,
}

impl TimetableBundleRouter {
    /// Maps a bundle file and builds its station index and lower bound graph.
    ///
    /// # Errors
    ///
    /// Returns an error when the bundle cannot be loaded.
    pub fn open(
        path: &Path,
        time_zone_finder: Arc<TimeZoneFinder>,
    ) -> Result<Self, BundleLoadError> {
        Ok(Self::from_loaded(
            LoadedTimetableBundle::open(path)?,
            time_zone_finder,
        ))
    }

    /// Wraps an already loaded bundle, for tests.
    pub fn from_loaded(
        bundle: LoadedTimetableBundle,
        time_zone_finder: Arc<TimeZoneFinder>,
    ) -> Self {
        let station_index = StationIndex::new(&bundle);
        let search_index = SearchIndex::new(&bundle);
        let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or_default();
        let start_days = (bundle.validity_start() - epoch).num_days();
        let bundle_start_unix_minutes =
            u32::try_from(start_days * i64::from(MINUTES_PER_DAY)).unwrap_or(0);
        Self {
            bundle,
            station_index,
            search_index,
            time_zone_finder,
            bundle_start_unix_minutes,
            spare_search_buffers: Mutex::new(Vec::new()),
        }
    }

    /// Returns the version of the bundle's data.
    pub fn bundle_version(&self) -> u32 {
        self.bundle.bundle_version()
    }

    /// Plans one leg. `should_stop` lets a newer planning run cancel the search.
    pub fn plan_leg(
        &self,
        input: &LegPlanningInput,
        should_stop: impl Fn() -> bool + Sync,
    ) -> LegPlanningResult {
        let Some(departure_window) = self.departure_window(input) else {
            return LegPlanningResult::NoJourneyFound(NoJourneyReason::TravelDateOutsideTimetable);
        };
        let first_day = departure_window.start.0 / MINUTES_PER_DAY;
        let departure_candidates = self.candidates(&input.departure, first_day..=first_day + 1);
        if departure_candidates.is_empty() {
            return LegPlanningResult::NoJourneyFound(NoJourneyReason::NoStationNearDeparturePlace);
        }
        let arrival_candidates = self.candidates(&input.arrival, first_day..=first_day + 2);
        if arrival_candidates.is_empty() {
            return LegPlanningResult::NoJourneyFound(NoJourneyReason::NoStationNearArrivalPlace);
        }
        let found = self.search_in_parallel(
            &departure_candidates,
            &arrival_candidates,
            departure_window,
            input,
            &should_stop,
        );
        // Lowest cost first. Ties go to the earlier arrival.
        let mut journeys: Vec<&RaptorJourney> = found.journeys.iter().collect();
        journeys.sort_by_key(|journey| {
            (
                journey_cost(
                    journey_cost_inputs(journey),
                    input.transfer_aversion,
                    ScoringWeights::DEFAULT,
                ),
                journey.arrival,
            )
        });
        let Some((chosen, alternatives)) = journeys.split_first() else {
            return LegPlanningResult::NoJourneyFound(NoJourneyReason::NoConnectionInWindow);
        };
        let mut stations = LegPlanStations::default();
        let Some(chosen_journey) = self.journey(chosen, &mut stations) else {
            return LegPlanningResult::NoJourneyFound(NoJourneyReason::NoConnectionInWindow);
        };
        let alternative_journeys = alternatives
            .iter()
            .take(MAXIMUM_ALTERNATIVES)
            .filter_map(|journey| self.journey(journey, &mut stations))
            .collect();
        LegPlanningResult::Found(Box::new(LegPlan {
            chosen_journey,
            alternative_journeys,
            station_table: stations.table,
        }))
    }

    /// Searches the departure window in parts on scoped threads and merges their journeys.
    ///
    /// Range search reuses labels from later departures within one part only, so the parts do
    /// more work in total than one search over the whole window. They finish sooner because
    /// they run at the same time, and the merged journeys are the same.
    fn search_in_parallel(
        &self,
        departure_candidates: &[CandidateStation],
        arrival_candidates: &[CandidateStation],
        departure_window: std::ops::Range<MinutesSinceBundleStart>,
        input: &LegPlanningInput,
        should_stop: &(impl Fn() -> bool + Sync),
    ) -> ParetoJourneySet {
        let start = departure_window.start.0;
        let length = departure_window.end.0.saturating_sub(start);
        let slice_count = PARALLEL_WINDOW_SLICES
            .min(length / SHORTEST_WINDOW_SLICE_MINUTES)
            .max(1);
        let slices = (0..slice_count).map(|slice| {
            let slice_start = start + length * slice / slice_count;
            let slice_end = start + length * (slice + 1) / slice_count;
            MinutesSinceBundleStart(slice_start)..MinutesSinceBundleStart(slice_end)
        });
        let search_slice = |slice_window| {
            let query = RaptorLegQuery {
                departure_candidates,
                arrival_candidates,
                departure_window: slice_window,
                minimum_transfer_minutes: input.minimum_transfer_time.minutes(),
                maximum_train_rides: MAXIMUM_TRAIN_RIDES,
            };
            let mut buffers = self
                .spare_search_buffers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop()
                .unwrap_or_default();
            let found = search_leg_journeys(
                &self.bundle,
                &self.search_index,
                &query,
                &BoardAnyTrip,
                &mut buffers,
                should_stop,
            );
            self.spare_search_buffers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(buffers);
            found
        };
        std::thread::scope(|scope| {
            let searches: Vec<_> = slices
                .map(|slice_window| scope.spawn(|| search_slice(slice_window)))
                .collect();
            ParetoJourneySet::merge(searches.into_iter().filter_map(|search| search.join().ok()))
        })
    }

    /// Turns the local departure window of a leg into bundle minutes. A night train leaves in
    /// its boarding window. Returns `None` when the window lies outside the bundle.
    fn departure_window(
        &self,
        input: &LegPlanningInput,
    ) -> Option<std::ops::Range<MinutesSinceBundleStart>> {
        let window: DailyTimeWindow = match input.travel_mode {
            LegTravelMode::Daytime => input.departure.time_window,
            LegTravelMode::Overnight(options) => options.boarding_window,
        };
        let time_zone = self
            .time_zone_finder
            .time_zone_at(input.departure.coordinates);
        let start_minute = u32::from(window.start.minutes_since_midnight());
        let start = local_time_to_unix_minutes(input.departure_date, start_minute, time_zone);
        let end = local_time_to_unix_minutes(
            input.departure_date,
            start_minute + u32::from(window.length.minutes()),
            time_zone,
        );
        let bundle_minutes =
            |moment: UnixTimeMinutes| moment.0.checked_sub(self.bundle_start_unix_minutes);
        let start = bundle_minutes(start)?;
        let end = bundle_minutes(end)?;
        let bundle_end = u32::from(self.bundle.validity_days()) * MINUTES_PER_DAY;
        (start < bundle_end)
            .then_some(MinutesSinceBundleStart(start)..MinutesSinceBundleStart(end.min(bundle_end)))
    }

    /// Returns the stations within a place's search radius that have service on any of the
    /// given days, with the time to get there, closest first.
    fn candidates(
        &self,
        endpoint: &LegEndpoint,
        days: std::ops::RangeInclusive<u32>,
    ) -> SmallVec<[CandidateStation; 16]> {
        let radius_meters = f64::from(endpoint.station_search_radius.meters());
        self.station_index
            .stations_within(&self.bundle, endpoint.coordinates, radius_meters)
            .into_iter()
            .filter(|nearby| {
                let service_days = self.bundle.station_service_days(nearby.station);
                days.clone()
                    .any(|day| u16::try_from(day).is_ok_and(|day| service_days.runs_on(day)))
            })
            .take(MAXIMUM_CANDIDATE_STATIONS)
            .map(|nearby| CandidateStation {
                station: nearby.station,
                time_to_place_minutes: access_minutes(nearby.distance_meters),
            })
            .collect()
    }

    /// Turns a router journey into a model journey, adding its stations to the table.
    fn journey(&self, journey: &RaptorJourney, stations: &mut LegPlanStations) -> Option<Journey> {
        let mut rides = journey.rides.iter();
        let first = rides.next()?;
        let first_ride = self.train_ride(first, stations)?;
        let mut last = first;
        let mut later_rides = SmallVec::new();
        for ride in rides {
            let transfer = ride.departure.0.saturating_sub(last.arrival.0);
            later_rides.push(TransferAndRide {
                transfer_duration: DurationMinutes(u16::try_from(transfer).unwrap_or(u16::MAX)),
                ride: self.train_ride(ride, stations)?,
            });
            last = ride;
        }
        let arrival_footpath = match arrival_footpath_minutes(journey) {
            None => None,
            Some(footpath_minutes) => Some(ArrivalFootpath {
                to_station: stations.index_of(&self.bundle, journey.arrival_candidate.station)?,
                duration: DurationMinutes(u16::try_from(footpath_minutes).unwrap_or(u16::MAX)),
            }),
        };
        Some(Journey {
            access_duration: DurationMinutes(journey.departure_candidate.time_to_place_minutes),
            first_ride,
            later_rides,
            arrival_footpath,
            egress_duration: DurationMinutes(journey.arrival_candidate.time_to_place_minutes),
        })
    }

    /// Turns a router ride into a model ride.
    fn train_ride(
        &self,
        ride: &btt_router::RaptorRide,
        stations: &mut LegPlanStations,
    ) -> Option<TrainRide> {
        let (category, number, _) = self.bundle.trip_train_label(ride.trip);
        let route = self.bundle.route(ride.raptor_route);
        let intermediate_station_coordinates = (ride.boarding_position + 1
            ..ride.alighting_position)
            .map(|position| {
                let station = self.bundle.pattern_stop(&route, position).station();
                self.bundle.station_coordinates(station)
            })
            .collect();
        Some(TrainRide {
            boarding_station: stations.index_of(&self.bundle, ride.boarding_station)?,
            departure_time: UnixTimeMinutes(self.bundle_start_unix_minutes + ride.departure.0),
            alighting_station: stations.index_of(&self.bundle, ride.alighting_station)?,
            arrival_time: UnixTimeMinutes(self.bundle_start_unix_minutes + ride.arrival.0),
            train_label: TrainLabel {
                category: category.into(),
                number: number.into(),
            },
            intermediate_station_coordinates,
        })
    }
}

/// The station table of a leg plan while it is being filled.
#[derive(Default)]
struct LegPlanStations {
    /// Bundle stations in table order.
    bundle_stations: SmallVec<[BundleStationIndex; 8]>,
    /// The table itself.
    table: SmallVec<[LegPlanStation; 8]>,
}

impl LegPlanStations {
    /// Returns the table index of a bundle station, adding it on first use. Returns `None` once
    /// the table is full.
    fn index_of(
        &mut self,
        bundle: &LoadedTimetableBundle,
        station: BundleStationIndex,
    ) -> Option<LegPlanStationIndex> {
        if let Some(position) = self
            .bundle_stations
            .iter()
            .position(|&known| known == station)
        {
            return Some(LegPlanStationIndex(u8::try_from(position).ok()?));
        }
        let index = LegPlanStationIndex(u8::try_from(self.table.len()).ok()?);
        self.bundle_stations.push(station);
        self.table.push(LegPlanStation {
            name: bundle.station_name(station).into(),
            coordinates: bundle.station_coordinates(station),
            time_zone: bundle.station_time_zone(station).into(),
        });
        Some(index)
    }
}

/// Returns the minutes of the footpath that ends a journey, or `None` when the last train
/// stops at the arrival station itself.
fn arrival_footpath_minutes(journey: &RaptorJourney) -> Option<u32> {
    let last = journey.rides.last()?;
    if last.alighting_station == journey.arrival_candidate.station {
        return None;
    }
    // The journey arrival counts up to the arrival place, so the footpath takes what remains
    // after the last train and before the egress.
    Some(
        journey
            .arrival
            .0
            .saturating_sub(u32::from(journey.arrival_candidate.time_to_place_minutes))
            .saturating_sub(last.arrival.0),
    )
}

/// Collects what the journey cost depends on.
///
/// A footpath at the end counts as a transfer. Beyond a short walk it stands for local transit,
/// and even a walk to another station is a change the traveller has to make.
fn journey_cost_inputs(journey: &RaptorJourney) -> JourneyCostInputs {
    let footpath_minutes = arrival_footpath_minutes(journey);
    let way_to_station_minutes = u32::from(journey.departure_candidate.time_to_place_minutes)
        + footpath_minutes.unwrap_or(0)
        + u32::from(journey.arrival_candidate.time_to_place_minutes);
    let train_changes = u32::try_from(journey.ride_count().saturating_sub(1)).unwrap_or(u32::MAX);
    JourneyCostInputs {
        door_to_door_minutes: journey.arrival.0.saturating_sub(journey.departure.0),
        way_to_station_minutes,
        transfer_count: train_changes + u32::from(footpath_minutes.is_some()),
    }
}

/// Estimates the minutes between a place and a station at a distance, with the same formula as
/// the builder's footpaths. Short distances are walks, longer ones local transit.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn access_minutes(distance_meters: f64) -> u16 {
    let minutes = if distance_meters < 1000.0 {
        (distance_meters * 1.3 / 83.3).ceil()
    } else {
        (15.0 + distance_meters / 250.0).ceil()
    };
    // The distance is at most the 50 km search radius, so the minutes fit easily.
    minutes as u16
}
