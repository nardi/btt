//! A backend that plans legs with the RAPTOR router on a timetable bundle.

use std::num::NonZeroU8;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use btt_geo::{TimeZoneFinder, local_time_to_unix_minutes};
use btt_model::{
    ArrivalFootpath, DurationMinutes, Journey, JourneyChoice, LegPlan, LegPlanStation,
    LegPlanStationIndex, LegPlanningResult, LegTravelMode, NoJourneyReason, OvernightLegOptions,
    StationWay, TrainLabel, TrainRide, TransferAndRide, UnixTimeMinutes, WayToStationMode,
};
use btt_router::{
    AllowEveryRide, CandidateStation, MinutesSinceBundleStart, ParetoJourneySet, RaptorJourney,
    RaptorLegQuery, RaptorSearchBuffers, RideFilter, SearchIndex, search_leg_journeys,
};
use btt_timetable::{
    BundleLoadError, BundleStationIndex, BundleTripIndex, LoadedTimetableBundle, RaptorRouteIndex,
    StationIndex,
};
use chrono::NaiveDate;
use smallvec::SmallVec;

use crate::chain::{ChainJourney, ChainStay, chain_through_costs};
use crate::explanation::{CandidateExplanation, LegExplanation, RankedJourneyExplanation};
use crate::input::{LegChainInput, LegEndpoint, LegPlanningInput, SameDayStop};
use crate::ride_path::StationHopGraph;
use crate::scoring::{JourneyCostInputs, ScoringWeights, journey_cost};

/// Largest number of candidate stations per place, closest first.
const MAXIMUM_CANDIDATE_STATIONS: usize = 16;

/// Largest number of trains in one journey.
const MAXIMUM_TRAIN_RIDES: NonZeroU8 = NonZeroU8::new(5).unwrap();

/// Largest number of alternatives a leg plan shows next to the chosen journey.
const MAXIMUM_ALTERNATIVES: usize = 4;

/// Alternatives cost at most this many times the cost of the best journey.
const MAXIMUM_ALTERNATIVE_COST_FACTOR: u32 = 2;

/// Minutes in one day.
const MINUTES_PER_DAY: u32 = 1440;

/// Number of parts the departure window splits into, each searched on its own thread.
const PARALLEL_WINDOW_SLICES: u32 = 4;

/// Shortest part of the departure window worth a thread of its own, in minutes.
const SHORTEST_WINDOW_SLICE_MINUTES: u32 = 60;

/// Length of the departure window of an overnight leg, which ends at bedtime. Night trains
/// often leave in the early evening, and the way to them may start with a daytime train.
const OVERNIGHT_DEPARTURE_WINDOW_MINUTES: u32 = 8 * 60;

/// Returns the local bedtime of an overnight leg in minutes since midnight of the departure
/// date. A bedtime before noon falls after midnight, so it counts on the next day.
fn bedtime_minute(options: OvernightLegOptions) -> u32 {
    let bedtime = u32::from(options.bedtime.minutes_since_midnight());
    if bedtime < MINUTES_PER_DAY / 2 {
        bedtime + MINUTES_PER_DAY
    } else {
        bedtime
    }
}

/// Minutes before the wake-up time from which trains without berths may be boarded again in
/// the night search. Seated trains through the night depart in the evening, while the trains
/// that follow a night train depart in the early morning.
const MORNING_BOARDING_MINUTES: u32 = 3 * 60;

/// Keeps night trains in the Pareto set, where a seated train that leaves later or arrives
/// earlier would otherwise beat them in time.
///
/// Trains with berths may be boarded at any time, other trains not in the core of the night,
/// from bedtime to `MORNING_BOARDING_MINUTES` before wake-up. A train with berths may not be
/// left before wake-up, except where it ends, so the search also finds the journeys that
/// stay on it as long as the traveller wants to sleep.
struct NightRideFilter<'a> {
    /// The bundle that says which trips have berths.
    bundle: &'a LoadedTimetableBundle,
    /// The night in bundle minutes.
    night: std::ops::Range<u32>,
}

impl RideFilter for NightRideFilter<'_> {
    fn may_board(
        &self,
        _: RaptorRouteIndex,
        trip: BundleTripIndex,
        departure: MinutesSinceBundleStart,
    ) -> bool {
        let core_end = self
            .night
            .end
            .saturating_sub(MORNING_BOARDING_MINUTES)
            .max(self.night.start);
        !(self.night.start..core_end).contains(&departure.0) || self.bundle.trip_offers_berths(trip)
    }

    fn may_alight(
        &self,
        _: RaptorRouteIndex,
        trip: BundleTripIndex,
        arrival: MinutesSinceBundleStart,
        is_last_stop: bool,
    ) -> bool {
        is_last_stop || !self.night.contains(&arrival.0) || !self.bundle.trip_offers_berths(trip)
    }
}

/// Plans legs on a loaded bundle.
pub struct TimetableBundleRouter {
    /// The loaded bundle.
    bundle: LoadedTimetableBundle,
    /// Finds stations near a place.
    station_index: StationIndex,
    /// Lower bounds on travel times, which let the search skip hopeless stations.
    search_index: SearchIndex,
    /// Consecutive stops of all routes, which show the stations a train passes.
    station_hop_graph: StationHopGraph,
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
        let station_hop_graph = StationHopGraph::new(&bundle);
        let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or_default();
        let start_days = (bundle.validity_start() - epoch).num_days();
        let bundle_start_unix_minutes =
            u32::try_from(start_days * i64::from(MINUTES_PER_DAY)).unwrap_or(0);
        Self {
            bundle,
            station_index,
            search_index,
            station_hop_graph,
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
        match self.search_leg(input, &should_stop) {
            Err(reason) => LegPlanningResult::NoJourneyFound(reason),
            Ok(search) => {
                let ranking = self.rank_journeys(&search, input);
                self.leg_plan(&ranking, input)
            }
        }
    }

    /// Plans the legs of a chain joined by same-day stops, one result per leg in order.
    ///
    /// Each leg after the first leaves within the stay range after the arrivals of the leg
    /// before it. Every journey then gets the cost of the best chain through it, which picks
    /// the chosen journeys and orders the alternatives. A leg without journeys splits the
    /// chain, and the legs on both sides are paired on their own.
    pub fn plan_chain(
        &self,
        chain: &LegChainInput,
        should_stop: impl Fn() -> bool + Sync,
    ) -> Vec<LegPlanningResult> {
        if let [leg] = chain.legs.as_slice() {
            return vec![self.plan_leg(&leg.input, should_stop)];
        }
        let mut searches: Vec<Result<LegSearch, NoJourneyReason>> =
            Vec::with_capacity(chain.legs.len());
        for (index, leg) in chain.legs.iter().enumerate() {
            let window_after_stay = index.checked_sub(1).and_then(|previous| {
                let previous_search = searches[previous].as_ref().ok()?;
                self.window_after_stay(previous_search, chain.stops[previous], &leg.input)
            });
            searches.push(match window_after_stay {
                Some(window) => self.search_leg_in_window(&leg.input, window, &should_stop),
                None => self.search_leg(&leg.input, &should_stop),
            });
        }

        let mut results: Vec<Option<LegPlanningResult>> = vec![None; chain.legs.len()];
        let mut segment_start = 0;
        for index in 0..=chain.legs.len() {
            let segment_ends = searches.get(index).is_none_or(Result::is_err);
            if !segment_ends {
                continue;
            }
            if segment_start < index {
                let segment = segment_start..index;
                let segment_searches: Vec<&LegSearch> = searches[segment.clone()]
                    .iter()
                    .filter_map(|search| search.as_ref().ok())
                    .collect();
                let segment_inputs: Vec<&LegPlanningInput> = chain.legs[segment.clone()]
                    .iter()
                    .map(|leg| &leg.input)
                    .collect();
                let stays = &chain.stops[segment.start..segment.end - 1];
                for (offset, result) in self
                    .plan_segment(&segment_searches, &segment_inputs, stays)
                    .into_iter()
                    .enumerate()
                {
                    results[segment.start + offset] = Some(result);
                }
            }
            if let Some(Err(reason)) = searches.get(index) {
                results[index] = Some(LegPlanningResult::NoJourneyFound(*reason));
            }
            segment_start = index + 1;
        }
        results
            .into_iter()
            .map(|result| {
                result.unwrap_or(LegPlanningResult::NoJourneyFound(
                    NoJourneyReason::NoConnectionInWindow,
                ))
            })
            .collect()
    }

    /// Ranks the journeys of consecutive searched legs by the best chain through them and turns
    /// them into leg plans.
    fn plan_segment(
        &self,
        searches: &[&LegSearch],
        inputs: &[&LegPlanningInput],
        stops: &[SameDayStop],
    ) -> Vec<LegPlanningResult> {
        let costed: Vec<Vec<RankedJourney<'_>>> = searches
            .iter()
            .zip(inputs)
            .map(|(search, input)| self.journey_costs(search, input))
            .collect();
        let chain_journeys: Vec<Vec<ChainJourney>> = costed
            .iter()
            .map(|journeys| {
                journeys
                    .iter()
                    .map(|ranked| ChainJourney {
                        departure: ranked.journey.departure.0,
                        arrival: ranked.journey.arrival.0,
                        cost: ranked.cost,
                    })
                    .collect()
            })
            .collect();
        let stays: Vec<ChainStay> = stops
            .iter()
            .map(|stop| {
                let minimum = u32::from(stop.stay_range.minimum.minutes());
                ChainStay {
                    minimum,
                    maximum: stop
                        .stay_range
                        .maximum_beyond_minimum
                        .map(|beyond| minimum + u32::from(beyond.minutes())),
                    prefers_longer: stop.prefers_longer_stay,
                }
            })
            .collect();
        let picks: Vec<Option<usize>> = costed
            .iter()
            .zip(inputs)
            .map(|(journeys, input)| {
                let choice = input.chosen_journey.as_ref()?;
                journeys
                    .iter()
                    .position(|ranked| self.journey_matches_choice(ranked.journey, choice))
            })
            .collect();

        costed
            .into_iter()
            .enumerate()
            .map(|(leg, journeys)| {
                // The picks of the other legs hold, while this leg's journeys all compete, so
                // the ranking also has alternatives to a picked journey.
                let mut forced = picks.clone();
                forced[leg] = None;
                let through_costs =
                    chain_through_costs(&chain_journeys, &stays, &forced, ScoringWeights::DEFAULT);
                let mut ranked: Vec<RankedJourney<'_>> = journeys
                    .into_iter()
                    .zip(&through_costs[leg])
                    .filter_map(|(ranked, through_cost)| {
                        Some(RankedJourney {
                            cost: u32::try_from((*through_cost)?.max(0)).unwrap_or(u32::MAX),
                            ..ranked
                        })
                    })
                    .collect();
                ranked.sort_by_key(|ranked| (ranked.cost, ranked.journey.arrival));
                let pick_position = inputs[leg].chosen_journey.as_ref().and_then(|choice| {
                    ranked.iter().position(|candidate| {
                        self.journey_matches_choice(candidate.journey, choice)
                    })
                });
                if let Some(position) = pick_position {
                    let picked = ranked.remove(position);
                    ranked.insert(0, picked);
                }
                let ranking = JourneyRanking {
                    journeys: ranked,
                    follows_traveller_choice: pick_position.is_some(),
                    night: searches[leg].night.clone(),
                };
                self.leg_plan(&ranking, inputs[leg])
            })
            .collect()
    }

    /// Returns the departure window of a leg that follows a same-day stop: from the earliest
    /// arrival of the leg before plus the shortest stay, to the latest arrival plus the longest
    /// stay. Without a longest stay it ends with the leg's own departure window. A departure
    /// window the traveller set, or the window of a night train, also bounds it. Returns `None`
    /// when the leg before has no journeys.
    fn window_after_stay(
        &self,
        previous: &LegSearch,
        stop: SameDayStop,
        input: &LegPlanningInput,
    ) -> Option<std::ops::Range<MinutesSinceBundleStart>> {
        let arrivals = previous
            .journeys
            .journeys
            .iter()
            .map(|journey| journey.arrival.0);
        let earliest_arrival = arrivals.clone().min()?;
        let latest_arrival = arrivals.max()?;
        let own_window = self.departure_window(input);
        let minimum = u32::from(stop.stay_range.minimum.minutes());
        let start = earliest_arrival + minimum;
        let end = match stop.stay_range.maximum_beyond_minimum {
            Some(beyond) => latest_arrival + minimum + u32::from(beyond.minutes()),
            None => own_window.as_ref().map_or(start, |window| window.end.0),
        };
        // A window of at least an hour leaves room for a departure when the stay range and the
        // leg's own window don't meet, and the stay then costs by how far it's off.
        let mut window = start..end.max(start + 60);
        let bounded_by_own_window =
            stop.has_departure_window || matches!(input.travel_mode, LegTravelMode::Overnight(_));
        if bounded_by_own_window && let Some(own_window) = own_window {
            let start = window.start.max(own_window.start.0);
            let end = window.end.min(own_window.end.0);
            window = if start < end {
                start..end
            } else {
                own_window.start.0..own_window.end.0
            };
        }
        Some(MinutesSinceBundleStart(window.start)..MinutesSinceBundleStart(window.end))
    }

    /// Plans one leg like `plan_leg` and also returns what the plan came from: the candidate
    /// stations and every journey of the Pareto set with its cost, for problem reports.
    pub fn explain_leg(&self, input: &LegPlanningInput) -> LegExplanation {
        let search = match self.search_leg(input, &|| false) {
            Err(reason) => {
                return LegExplanation::without_search(LegPlanningResult::NoJourneyFound(reason));
            }
            Ok(search) => search,
        };
        let ranking = self.rank_journeys(&search, input);
        let result = self.leg_plan(&ranking, input);
        let candidate = |endpoint: &LegEndpoint, candidate: &CandidateStation| {
            let coordinates = self.bundle.station_coordinates(candidate.station);
            CandidateExplanation {
                station_name: self.bundle.station_name(candidate.station).into(),
                coordinates,
                distance_meters: rounded_meters(
                    endpoint.coordinates.haversine_distance_meters(coordinates),
                ),
                minutes_to_place: candidate.time_to_place_minutes,
            }
        };
        let mut stations = LegPlanStations::default();
        let ranked_journeys = ranking
            .journeys
            .iter()
            .filter_map(|ranked| {
                Some(RankedJourneyExplanation {
                    journey: self.journey(
                        ranked.journey,
                        input,
                        ranking.night.as_ref(),
                        &mut stations,
                    )?,
                    cost: ranked.cost,
                    cost_inputs: ranked.cost_inputs,
                })
            })
            .collect();
        LegExplanation {
            result,
            departure_candidates: search
                .departure_candidates
                .iter()
                .map(|station| candidate(&input.departure, station))
                .collect(),
            arrival_candidates: search
                .arrival_candidates
                .iter()
                .map(|station| candidate(&input.arrival, station))
                .collect(),
            ranked_journeys,
            station_table: stations.table.into_vec(),
            follows_traveller_choice: ranking.follows_traveller_choice,
        }
    }

    /// Finds the candidate stations of a leg and searches its Pareto set of journeys.
    fn search_leg(
        &self,
        input: &LegPlanningInput,
        should_stop: &(impl Fn() -> bool + Sync),
    ) -> Result<LegSearch, NoJourneyReason> {
        let departure_window = self
            .departure_window(input)
            .ok_or(NoJourneyReason::TravelDateOutsideTimetable)?;
        self.search_leg_in_window(input, departure_window, should_stop)
    }

    /// Searches a leg like `search_leg`, leaving the departure place within a given window
    /// instead of the leg's own.
    fn search_leg_in_window(
        &self,
        input: &LegPlanningInput,
        departure_window: std::ops::Range<MinutesSinceBundleStart>,
        should_stop: &(impl Fn() -> bool + Sync),
    ) -> Result<LegSearch, NoJourneyReason> {
        let bundle_end = u32::from(self.bundle.validity_days()) * MINUTES_PER_DAY;
        if departure_window.start.0 >= bundle_end {
            return Err(NoJourneyReason::TravelDateOutsideTimetable);
        }
        let first_day = departure_window.start.0 / MINUTES_PER_DAY;
        let departure_candidates = self.candidates(&input.departure, first_day..=first_day + 1);
        if departure_candidates.is_empty() {
            return Err(NoJourneyReason::NoStationNearDeparturePlace);
        }
        let arrival_candidates = self.candidates(&input.arrival, first_day..=first_day + 2);
        if arrival_candidates.is_empty() {
            return Err(NoJourneyReason::NoStationNearArrivalPlace);
        }
        let mut journeys = self.search_in_parallel(
            &departure_candidates,
            &arrival_candidates,
            departure_window.clone(),
            input,
            &AllowEveryRide,
            should_stop,
        );
        let night = self.night_in_bundle_minutes(input);
        if let Some(night) = &night {
            // A second search keeps night trains that the first one prunes, because a seated
            // train that leaves later or arrives earlier beats them in time. The ranking then
            // weighs the sleep against the time.
            let filter = NightRideFilter {
                bundle: &self.bundle,
                night: night.clone(),
            };
            let night_journeys = self.search_in_parallel(
                &departure_candidates,
                &arrival_candidates,
                departure_window,
                input,
                &filter,
                should_stop,
            );
            for journey in night_journeys.journeys {
                if !journeys.journeys.contains(&journey) {
                    journeys.journeys.push(journey);
                }
            }
            journeys.journeys.sort_by_key(|journey| journey.arrival);
        }
        Ok(LegSearch {
            departure_candidates,
            arrival_candidates,
            journeys,
            night,
        })
    }

    /// Orders the journeys of a search by cost, lowest first and the earlier arrival on a tie.
    /// The traveller's pick goes first when it is still among them, and the planner's pick
    /// then becomes the first alternative.
    fn rank_journeys<'search>(
        &self,
        search: &'search LegSearch,
        input: &LegPlanningInput,
    ) -> JourneyRanking<'search> {
        let mut journeys = self.journey_costs(search, input);
        journeys.sort_by_key(|ranked| (ranked.cost, ranked.journey.arrival));
        let chosen_position = input.chosen_journey.as_ref().and_then(|choice| {
            journeys
                .iter()
                .position(|ranked| self.journey_matches_choice(ranked.journey, choice))
        });
        if let Some(position) = chosen_position {
            let picked = journeys.remove(position);
            journeys.insert(0, picked);
        }
        JourneyRanking {
            journeys,
            follows_traveller_choice: chosen_position.is_some(),
            night: search.night.clone(),
        }
    }

    /// Returns every journey of a search with its cost, in the order of the search.
    fn journey_costs<'search>(
        &self,
        search: &'search LegSearch,
        input: &LegPlanningInput,
    ) -> Vec<RankedJourney<'search>> {
        let mut arrival_window = self.arrival_window_in_bundle_minutes(input);
        // Arriving before the wake-up time counts as arriving outside the window.
        if let Some(night) = &search.night {
            arrival_window.start = arrival_window.start.max(night.end);
            arrival_window.end = arrival_window.end.max(arrival_window.start);
        }
        search
            .journeys
            .journeys
            .iter()
            .map(|journey| {
                let cost_inputs =
                    self.journey_cost_inputs(journey, &arrival_window, search.night.as_ref());
                RankedJourney {
                    journey,
                    cost_inputs,
                    cost: journey_cost(
                        cost_inputs,
                        input.transfer_aversion,
                        ScoringWeights::DEFAULT,
                    ),
                }
            })
            .collect()
    }

    /// Turns ranked journeys into a leg plan, with the first as the chosen journey.
    fn leg_plan(
        &self,
        ranking: &JourneyRanking<'_>,
        input: &LegPlanningInput,
    ) -> LegPlanningResult {
        let Some((chosen, alternatives)) = ranking.journeys.split_first() else {
            return LegPlanningResult::NoJourneyFound(NoJourneyReason::NoConnectionInWindow);
        };
        let mut stations = LegPlanStations::default();
        let Some(chosen_journey) =
            self.journey(chosen.journey, input, ranking.night.as_ref(), &mut stations)
        else {
            return LegPlanningResult::NoJourneyFound(NoJourneyReason::NoConnectionInWindow);
        };
        // Alternatives are the next journeys by cost, skipping ones that only repeat a shown
        // journey at another time and ones far more costly than the best.
        let lowest_cost = ranking
            .journeys
            .iter()
            .map(|ranked| ranked.cost)
            .min()
            .unwrap_or(0);
        let mut shown_routes = vec![self.journey_route(chosen.journey)];
        let alternative_journeys = alternatives
            .iter()
            .filter(|ranked| {
                ranked.cost <= lowest_cost.saturating_mul(MAXIMUM_ALTERNATIVE_COST_FACTOR)
            })
            .filter(|ranked| {
                let route = self.journey_route(ranked.journey);
                let is_new = !shown_routes.contains(&route);
                if is_new {
                    shown_routes.push(route);
                }
                is_new
            })
            .take(MAXIMUM_ALTERNATIVES)
            .filter_map(|ranked| {
                self.journey(ranked.journey, input, ranking.night.as_ref(), &mut stations)
            })
            .collect();
        LegPlanningResult::Found(Box::new(LegPlan {
            chosen_journey,
            alternative_journeys,
            station_table: stations.table,
            follows_traveller_choice: ranking.follows_traveller_choice,
        }))
    }

    /// Returns what makes a journey a different option from another: the stations where the
    /// traveller boards and leaves each train, and the train categories. Two journeys with the
    /// same route differ only in time.
    fn journey_route(
        &self,
        journey: &RaptorJourney,
    ) -> Vec<(BundleStationIndex, BundleStationIndex, &str)> {
        journey
            .rides
            .iter()
            .map(|ride| {
                let (category, _, _) = self.bundle.trip_train_label(ride.trip);
                (ride.boarding_station, ride.alighting_station, category)
            })
            .collect()
    }

    /// Returns whether a journey has the first departure and the trains of a choice.
    fn journey_matches_choice(&self, journey: &RaptorJourney, choice: &JourneyChoice) -> bool {
        let Some(first) = journey.rides.first() else {
            return false;
        };
        self.bundle_start_unix_minutes + first.departure.0 == choice.first_departure_time.0
            && journey.rides.len() == choice.trains.len()
            && journey
                .rides
                .iter()
                .zip(&choice.trains)
                .all(|(ride, train)| {
                    let (category, number, _) = self.bundle.trip_train_label(ride.trip);
                    category == train.category && number == train.number
                })
    }

    /// Searches the departure window in parts on scoped threads and merges their journeys.
    ///
    /// Range search reuses labels from later departures within one part only, so the parts do
    /// more work in total than one search over the whole window. They finish sooner because
    /// they run at the same time, and the merged journeys are the same.
    fn search_in_parallel<F: RideFilter + Sync>(
        &self,
        departure_candidates: &[CandidateStation],
        arrival_candidates: &[CandidateStation],
        departure_window: std::ops::Range<MinutesSinceBundleStart>,
        input: &LegPlanningInput,
        boarding_filter: &F,
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
                boarding_filter,
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
    /// the hours before bedtime. Returns `None` when the window lies outside the bundle.
    fn departure_window(
        &self,
        input: &LegPlanningInput,
    ) -> Option<std::ops::Range<MinutesSinceBundleStart>> {
        let (start_minute, end_minute) = match input.travel_mode {
            LegTravelMode::Daytime => {
                let window = input.departure.time_window;
                let start = u32::from(window.start.minutes_since_midnight());
                (start, start + u32::from(window.length.minutes()))
            }
            LegTravelMode::Overnight(options) => {
                let bedtime = bedtime_minute(options);
                (bedtime - OVERNIGHT_DEPARTURE_WINDOW_MINUTES, bedtime)
            }
        };
        let time_zone = self
            .time_zone_finder
            .time_zone_at(input.departure.coordinates);
        let bundle_minutes = |minute: u32| {
            local_time_to_unix_minutes(input.departure_date, minute, time_zone)
                .0
                .checked_sub(self.bundle_start_unix_minutes)
        };
        let start = bundle_minutes(start_minute)?;
        let end = bundle_minutes(end_minute)?;
        let bundle_end = u32::from(self.bundle.validity_days()) * MINUTES_PER_DAY;
        (start < bundle_end)
            .then_some(MinutesSinceBundleStart(start)..MinutesSinceBundleStart(end.min(bundle_end)))
    }

    /// Returns the night of an overnight leg in bundle minutes, from bedtime at the departure
    /// place to the wake-up time at the arrival place on the morning after the departure date.
    /// Returns `None` for a daytime leg. Moments before the bundle start count as its start.
    fn night_in_bundle_minutes(&self, input: &LegPlanningInput) -> Option<std::ops::Range<u32>> {
        let LegTravelMode::Overnight(options) = input.travel_mode else {
            return None;
        };
        let bundle_minutes = |date, minute, coordinates| {
            let time_zone = self.time_zone_finder.time_zone_at(coordinates);
            local_time_to_unix_minutes(date, minute, time_zone)
                .0
                .saturating_sub(self.bundle_start_unix_minutes)
        };
        let bedtime = bundle_minutes(
            input.departure_date,
            bedtime_minute(options),
            input.departure.coordinates,
        );
        let wake_up_time = bundle_minutes(
            input.departure_date + chrono::Days::new(1),
            u32::from(options.wake_up_time.minutes_since_midnight()),
            input.arrival.coordinates,
        );
        Some(bedtime..wake_up_time.max(bedtime))
    }

    /// Returns the arrival window of a leg in bundle minutes. A daytime leg arrives on its
    /// departure date, and an overnight leg on the day after. Moments before the bundle start
    /// count as its start.
    fn arrival_window_in_bundle_minutes(&self, input: &LegPlanningInput) -> std::ops::Range<u32> {
        let window = input.arrival.time_window;
        let date = match input.travel_mode {
            LegTravelMode::Daytime => input.departure_date,
            LegTravelMode::Overnight(_) => input.departure_date + chrono::Days::new(1),
        };
        let time_zone = self
            .time_zone_finder
            .time_zone_at(input.arrival.coordinates);
        let start_minute = u32::from(window.start.minutes_since_midnight());
        let end_minute = start_minute + u32::from(window.length.minutes());
        let bundle_minutes = |minute: u32| {
            local_time_to_unix_minutes(date, minute, time_zone)
                .0
                .saturating_sub(self.bundle_start_unix_minutes)
        };
        bundle_minutes(start_minute)..bundle_minutes(end_minute)
    }

    /// Finds the train with berths that covers most of the night, with the minutes it covers.
    /// Only one train counts, because changing trains wakes the traveller up.
    fn sleeping_ride(
        &self,
        journey: &RaptorJourney,
        night: &std::ops::Range<u32>,
    ) -> Option<(usize, u32)> {
        journey
            .rides
            .iter()
            .enumerate()
            .filter(|(_, ride)| self.bundle.trip_offers_berths(ride.trip))
            .map(|(position, ride)| {
                let covered = ride
                    .arrival
                    .0
                    .min(night.end)
                    .saturating_sub(ride.departure.0.max(night.start));
                (position, covered)
            })
            .filter(|&(_, covered)| covered > 0)
            .max_by_key(|&(_, covered)| covered)
    }

    /// Collects what the journey cost depends on.
    ///
    /// A footpath at the end counts as a transfer. Beyond a short walk it stands for local
    /// transit, and even a walk to another station is a change the traveller has to make.
    fn journey_cost_inputs(
        &self,
        journey: &RaptorJourney,
        arrival_window: &std::ops::Range<u32>,
        night: Option<&std::ops::Range<u32>>,
    ) -> JourneyCostInputs {
        let footpath_minutes = arrival_footpath_minutes(journey);
        let way_to_station_minutes = u32::from(journey.departure_candidate.time_to_place_minutes)
            + footpath_minutes.unwrap_or(0)
            + u32::from(journey.arrival_candidate.time_to_place_minutes);
        let train_changes =
            u32::try_from(journey.ride_count().saturating_sub(1)).unwrap_or(u32::MAX);
        let arrival = journey.arrival.0;
        let (sleeping_minutes, night_minutes_awake) = night.map_or((0, 0), |night| {
            let sleeping = self
                .sleeping_ride(journey, night)
                .map_or(0, |(_, covered)| covered);
            // After arriving, the traveller is at the place, where the arrival window counts
            // instead.
            let travelling_night_end = arrival.min(night.end).max(night.start);
            (
                sleeping,
                (travelling_night_end - night.start).saturating_sub(sleeping),
            )
        });
        JourneyCostInputs {
            door_to_door_minutes: arrival.saturating_sub(journey.departure.0),
            way_to_station_minutes,
            transfer_count: train_changes + u32::from(footpath_minutes.is_some()),
            minutes_outside_arrival_window: arrival_window.start.saturating_sub(arrival)
                + arrival.saturating_sub(arrival_window.end),
            sleeping_minutes,
            night_minutes_awake,
        }
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
    fn journey(
        &self,
        journey: &RaptorJourney,
        input: &LegPlanningInput,
        night: Option<&std::ops::Range<u32>>,
        stations: &mut LegPlanStations,
    ) -> Option<Journey> {
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
        let coordinates = |station| self.bundle.station_coordinates(station);
        let arrival_footpath = match arrival_footpath_minutes(journey) {
            None => None,
            Some(footpath_minutes) => Some(ArrivalFootpath {
                to_station: stations.index_of(&self.bundle, journey.arrival_candidate.station)?,
                way: station_way(
                    coordinates(last.alighting_station)
                        .haversine_distance_meters(coordinates(journey.arrival_candidate.station)),
                    u16::try_from(footpath_minutes).unwrap_or(u16::MAX),
                ),
            }),
        };
        Some(Journey {
            access: station_way(
                input
                    .departure
                    .coordinates
                    .haversine_distance_meters(coordinates(journey.departure_candidate.station)),
                journey.departure_candidate.time_to_place_minutes,
            ),
            first_ride,
            later_rides,
            arrival_footpath,
            egress: station_way(
                input
                    .arrival
                    .coordinates
                    .haversine_distance_meters(coordinates(journey.arrival_candidate.station)),
                journey.arrival_candidate.time_to_place_minutes,
            ),
            sleeping_ride: night.and_then(|night| {
                let (position, covered) = self.sleeping_ride(journey, night)?;
                (covered * 2 >= night.end - night.start)
                    .then(|| u8::try_from(position).ok())
                    .flatten()
            }),
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
        let station_at = |position| self.bundle.pattern_stop(&route, position).station();
        let intermediate_station_coordinates = (ride.boarding_position + 1
            ..ride.alighting_position)
            .map(|position| self.bundle.station_coordinates(station_at(position)))
            .collect();
        // Each hop contributes the stations it passes and then the stop it ends at, except the
        // last hop, whose stop is the alighting station.
        let mut path_coordinates = Vec::new();
        for position in ride.boarding_position + 1..=ride.alighting_position {
            let (from, to) = (station_at(position - 1), station_at(position));
            path_coordinates.extend(self.station_hop_graph.passed_station_coordinates(from, to));
            if position < ride.alighting_position {
                path_coordinates.push(self.bundle.station_coordinates(to));
            }
        }
        Some(TrainRide {
            boarding_station: stations.index_of(&self.bundle, ride.boarding_station)?,
            departure_time: UnixTimeMinutes(self.bundle_start_unix_minutes + ride.departure.0),
            alighting_station: stations.index_of(&self.bundle, ride.alighting_station)?,
            arrival_time: UnixTimeMinutes(self.bundle_start_unix_minutes + ride.arrival.0),
            train_label: TrainLabel {
                category: category.into(),
                number: number.into(),
            },
            offers_berths: self.bundle.trip_offers_berths(ride.trip),
            intermediate_station_coordinates,
            path_coordinates,
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

/// Distance from which the estimates assume local transport instead of walking, matching the
/// builder's footpaths.
const LONGEST_WALK_METERS: f64 = 1000.0;

/// Describes a way off the train with its estimated minutes and the mode the estimate assumes.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn station_way(distance_meters: f64, minutes: u16) -> StationWay {
    StationWay {
        duration: DurationMinutes(minutes),
        // Ways are at most a few tens of kilometers, so the meters fit easily.
        distance_meters: distance_meters.round() as u32,
        mode: if distance_meters < LONGEST_WALK_METERS {
            WayToStationMode::Walk
        } else {
            WayToStationMode::LocalTransport
        },
    }
}

/// Estimates the minutes between a place and a station at a distance, with the same formula as
/// the builder's footpaths. Short distances are walks, longer ones local transit.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn access_minutes(distance_meters: f64) -> u16 {
    let minutes = if distance_meters < LONGEST_WALK_METERS {
        (distance_meters * 1.3 / 83.3).ceil()
    } else {
        (15.0 + distance_meters / 250.0).ceil()
    };
    // The distance is at most the 50 km search radius, so the minutes fit easily.
    minutes as u16
}

/// Everything a search found for a leg, before ranking.
struct LegSearch {
    /// Stations near the departure place with their access times.
    departure_candidates: SmallVec<[CandidateStation; 16]>,
    /// Stations near the arrival place with their egress times.
    arrival_candidates: SmallVec<[CandidateStation; 16]>,
    /// The journeys: the Pareto set, and for an overnight leg also the Pareto set of the
    /// search that keeps night trains.
    journeys: ParetoJourneySet,
    /// The night of an overnight leg in bundle minutes.
    night: Option<std::ops::Range<u32>>,
}

/// A journey of a search with its cost.
struct RankedJourney<'search> {
    /// The journey.
    journey: &'search RaptorJourney,
    /// What the cost came from.
    cost_inputs: JourneyCostInputs,
    /// The cost, lower is better.
    cost: u32,
}

/// The journeys of a search in the order the planner shows them.
struct JourneyRanking<'search> {
    /// Chosen journey first, then the alternatives.
    journeys: Vec<RankedJourney<'search>>,
    /// Whether the first journey is the traveller's pick.
    follows_traveller_choice: bool,
    /// The night of an overnight leg in bundle minutes.
    night: Option<std::ops::Range<u32>>,
}

/// Rounds a distance to whole meters. Distances here stay far below the range of `u32`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn rounded_meters(distance_meters: f64) -> u32 {
    distance_meters.round() as u32
}
