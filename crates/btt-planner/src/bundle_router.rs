//! A backend that plans legs with the RAPTOR router on a timetable bundle.

use std::num::NonZeroU8;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use btt_geo::{TimeZoneFinder, local_time_to_unix_minutes};
use btt_model::{
    AlternativeKind, ArrivalFootpath, DurationMinutes, EuroRange, Journey, JourneyChoice,
    JourneyCostParts, LegExtras, LegPlan, LegPlanStation, LegPlanStationIndex, LegPlanningResult,
    LegTravelMode, NoJourneyReason, OvernightLegOptions, PassDays, PassJourneyOption, PriceWeight,
    SearchedWindow, StationWay, TrainLabel, TrainRide, TransferAndRide, UnixTimeMinutes,
    WayToStationMode,
};
use btt_router::{
    CandidateStation, CostParetoJourneySet, CostSearchBuffers, MinutesSinceBundleStart,
    RaptorJourney, RaptorLegQuery, SearchIndex, search_leg_journeys_by_cost,
};
use btt_timetable::{
    BundleLoadError, BundleStationIndex, BundleTripIndex, LoadedTimetableBundle, RaptorRouteIndex,
    StationIndex,
};
use chrono::NaiveDate;
use smallvec::SmallVec;

mod fare_samples;

pub use fare_samples::FareSampleRide;

use crate::chain::{ChainJourney, ChainStay, chain_through_costs};
use crate::cost_model::LegCostModel;
use crate::explanation::{CandidateExplanation, LegExplanation, RankedJourneyExplanation};
use crate::input::{LegChainInput, LegEndpoint, LegPlanningInput, SameDayStop};
use crate::operators::OperatorTable;
use crate::pricing::{JourneyPricer, NoPrice, PriceMode, RidePrices};
use crate::ride_path::StationHopGraph;
use crate::scoring::{
    JourneyCostInputs, ScoringWeights, journey_cost, journey_cost_parts, transfer_cost_minutes,
};

/// Largest number of candidate stations per place, closest first.
const MAXIMUM_CANDIDATE_STATIONS: usize = 16;

/// Largest number of trains in one journey.
const MAXIMUM_TRAIN_RIDES: NonZeroU8 = NonZeroU8::new(5).unwrap();

/// Largest number of alternatives a leg plan shows next to the chosen journey.
const MAXIMUM_ALTERNATIVES: usize = 4;

/// How far below the shortest stay at a same-day stop the next leg is still searched, in
/// minutes. The stay penalty decides whether such a short stay is worth it.
const SHORT_STAY_SEARCH_MINUTES: u32 = 60;

/// Alternatives cost at most this many times the cost of the best journey.
const MAXIMUM_ALTERNATIVE_COST_FACTOR: u32 = 2;

/// A cheaper journey shows when it saves at least this share of the chosen journey's price.
const CHEAPEST_SAVING_PERCENT: u32 = 15;

/// A cheaper journey shows when it saves at least this many euro cents.
const CHEAPEST_SAVING_CENTS: u32 = 1_000;

/// A cheaper journey shows when it takes at most this many minutes longer than the chosen one.
const CHEAPEST_EXTRA_MINUTES: u32 = 90;

/// A faster journey shows when it takes at least this many minutes less than the chosen one.
const FASTEST_SAVING_MINUTES: u32 = 30;

/// A faster journey shows when its price is at most this share above the chosen one's.
const FASTEST_EXTRA_PRICE_PERCENT: u32 = 25;

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

/// Splits a departure window into the parts that are searched on their own threads.
fn window_slices(
    departure_window: std::ops::Range<MinutesSinceBundleStart>,
) -> impl Iterator<Item = std::ops::Range<MinutesSinceBundleStart>> {
    let start = departure_window.start.0;
    let length = departure_window.end.0.saturating_sub(start);
    let slice_count = PARALLEL_WINDOW_SLICES
        .min(length / SHORTEST_WINDOW_SLICE_MINUTES)
        .max(1);
    (0..slice_count).map(move |slice| {
        let slice_start = start + length * slice / slice_count;
        let slice_end = start + length * (slice + 1) / slice_count;
        MinutesSinceBundleStart(slice_start)..MinutesSinceBundleStart(slice_end)
    })
}

/// Plans legs on a loaded bundle.
pub struct TimetableBundleRouter<P: JourneyPricer = NoPrice> {
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
    /// Whether each route has a trip with berths, whose ride cost depends on the night.
    routes_with_berths: Vec<bool>,
    /// Search buffers of finished searches, reused by later ones to avoid allocating.
    spare_search_buffers: Mutex<Vec<CostSearchBuffers>>,
    /// Tells the operator of a train from its agency and category.
    operators: OperatorTable,
    /// Prices a journey, which the plan shows next to the journey's time.
    pricer: P,
    /// The prices of rides, which the search adds to the cost. `None` when the pricer knows
    /// no price table.
    ride_prices: Option<RidePrices>,
}

impl<P: JourneyPricer> TimetableBundleRouter<P> {
    /// Maps a bundle file and builds its station index and lower bound graph.
    ///
    /// # Errors
    ///
    /// Returns an error when the bundle cannot be loaded.
    pub fn open(
        path: &Path,
        time_zone_finder: Arc<TimeZoneFinder>,
        pricer: P,
    ) -> Result<Self, BundleLoadError> {
        Ok(Self::from_loaded(
            LoadedTimetableBundle::open(path)?,
            time_zone_finder,
            pricer,
        ))
    }

    /// Wraps an already loaded bundle, for tests.
    pub fn from_loaded(
        bundle: LoadedTimetableBundle,
        time_zone_finder: Arc<TimeZoneFinder>,
        pricer: P,
    ) -> Self {
        let station_index = StationIndex::new(&bundle);
        let search_index = SearchIndex::new(&bundle);
        let station_hop_graph = StationHopGraph::new(&bundle);
        let routes_with_berths: Vec<bool> = (0..bundle.route_count())
            .map(|route_index| {
                let route = bundle.route(RaptorRouteIndex(
                    u32::try_from(route_index).unwrap_or(u32::MAX),
                ));
                (0..route.trip_count).any(|trip_offset| {
                    bundle.trip_offers_berths(BundleTripIndex(route.first_trip_index + trip_offset))
                })
            })
            .collect();
        let operators = OperatorTable::bundled();
        // The prices are a snapshot of the model at the moment the bundle loads. What the model
        // learns later changes no plan that is under way.
        let ride_prices = RidePrices::build(&bundle, &operators, &routes_with_berths, &pricer);
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
            routes_with_berths,
            spare_search_buffers: Mutex::new(Vec::new()),
            operators,
            pricer,
            ride_prices,
        }
    }

    /// Returns the version of the bundle's data.
    pub fn bundle_version(&self) -> u32 {
        self.bundle.bundle_version()
    }

    /// Plans one leg, without the searches of `plan_extras`, so the result arrives as soon as
    /// the leg's own search is done. `should_stop` lets a newer planning run cancel the search.
    pub fn plan_leg(
        &self,
        input: &LegPlanningInput,
        should_stop: impl Fn() -> bool + Sync,
    ) -> LegPlanningResult {
        match self.search_leg_from_anchors(input, &should_stop) {
            Err(reason) => LegPlanningResult::NoJourneyFound(reason),
            Ok(mut search) => {
                self.add_pick_from_other_weights(&mut search, input, None, &should_stop);
                let ranking = self.rank_journeys(&search, input);
                self.leg_plan(&ranking, input)
            }
        }
    }

    /// Plans one leg and then runs the searches of `plan_extras` for it, the way the app does
    /// in two steps. For a caller that has no use for the main plan on its own.
    pub fn plan_leg_with_extras(
        &self,
        input: &LegPlanningInput,
        should_stop: impl Fn() -> bool + Sync,
    ) -> LegPlanningResult {
        let mut result = self.plan_leg(input, &should_stop);
        if let LegPlanningResult::Found(plan) = &mut result {
            let extras = self.plan_extras(input, plan, true, &should_stop);
            plan.apply_extras(extras);
        }
        result
    }

    /// Runs the searches that follow the main plan of a leg. One at the cheapest weight finds
    /// the cheaper journey, one without a weight finds the faster journey, and one on the
    /// travel days of a pass finds the journeys that the solve of the whole trip picks among.
    /// Each takes about as long as the main plan or longer, so the main plan is delivered first.
    ///
    /// The journeys of the searches at other weights join the alternatives of the plan, each
    /// marked as the cheaper or the faster journey, and a journey that the same search finds
    /// twice shows once. A cheaper journey that leaves and arrives when the chosen journey does
    /// becomes the chosen journey, because it is the same trip for less, and only a search at
    /// another weight can tell the two apart when the traveller's weight is zero. The traveller's
    /// own pick stays.
    ///
    /// `on_its_own` is false for a leg joined to others by same-day stops. Its departure window
    /// follows the leg before it, so it gets no pass options, and its chosen journey is the one
    /// on the best chain, which no search of the leg alone can replace.
    ///
    /// `should_stop` lets a newer planning run cancel the searches, and the caller drops the
    /// result of a cancelled run.
    pub fn plan_extras(
        &self,
        input: &LegPlanningInput,
        plan: &LegPlan,
        on_its_own: bool,
        should_stop: impl Fn() -> bool + Sync,
    ) -> LegExtras {
        let mut extras = plan.extras_without_changes();
        let weight = input.price_weight.hundredths_of_minute_per_euro();
        let mut found: Vec<(AlternativeKind, ExtraJourney)> = Vec::new();
        // A traveller below the cheapest weight never sees a cheap journey otherwise, and one
        // above zero never sees a fast one. A cheaper journey is only worth showing when it
        // takes at most `CHEAPEST_EXTRA_MINUTES` longer, so the search drops slower journeys.
        let mut replacement: Option<ExtraJourney> = None;
        if weight < PriceWeight::CHEAPEST.hundredths_of_minute_per_euro() {
            let bounds = SearchBounds::for_cheaper_than(&plan.chosen_journey, on_its_own);
            if let Some(cheaper) =
                self.best_journey_at(input, PriceWeight::CHEAPEST, bounds, &should_stop)
            {
                if on_its_own
                    && !plan.follows_traveller_choice
                    && is_the_same_trip_for_less(&cheaper.journey, &plan.chosen_journey)
                {
                    replacement = Some(cheaper);
                } else if cheaper_by_enough(&cheaper.journey, &plan.chosen_journey) {
                    found.push((AlternativeKind::Cheaper, cheaper));
                }
            }
        }
        if weight > PriceWeight::TIME_FIRST.hundredths_of_minute_per_euro()
            && let Some(faster) = self.best_journey_at(
                input,
                PriceWeight::TIME_FIRST,
                SearchBounds::for_faster_than(&plan.chosen_journey, on_its_own),
                &should_stop,
            )
            && faster_by_enough(&faster.journey, &plan.chosen_journey)
        {
            found.push((AlternativeKind::Faster, faster));
        }

        let mut table = plan.station_table.clone();
        if let Some(replacement) = replacement
            && let Some(better) = absorb_journey(&mut table, replacement)
        {
            let old = std::mem::replace(&mut extras.chosen_journey, better);
            if let Some(parts) = extras.chosen_journey.cost_parts {
                extras.best_journey_cost = extras
                    .best_journey_cost
                    .min(u32::try_from(parts.total_cost).unwrap_or(u32::MAX));
            }
            extras.alternative_journeys.push(old);
        }
        for (kind, candidate) in found {
            let Some(mut journey) = absorb_journey(&mut table, candidate) else {
                continue;
            };
            let known = std::iter::once(&extras.chosen_journey)
                .chain(&extras.alternative_journeys)
                .any(|shown| same_trains(shown, &journey));
            if known {
                // The journey is one of the alternatives already, so it only gets the mark.
                if let Some(shown) = extras
                    .alternative_journeys
                    .iter_mut()
                    .find(|shown| same_trains(shown, &journey))
                {
                    shown.alternative_kind = Some(kind);
                }
            } else {
                journey.alternative_kind = Some(kind);
                extras.alternative_journeys.push(journey);
            }
        }
        // The alternatives keep the planner's order, which is by cost at the traveller's weight.
        extras.alternative_journeys.sort_by_key(|journey| {
            journey
                .cost_parts
                .map_or(i32::MAX, |parts| parts.total_cost)
        });
        if on_its_own {
            extras.pass_options = self.pass_options(input, &should_stop, &mut table);
        }
        extras.station_table = table;
        extras
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
            let window_used = window_after_stay.clone();
            searches.push(match window_after_stay {
                Some(window) => self.search_leg_in_window(
                    &leg.input,
                    window,
                    SearchOptions::SINGLE,
                    &should_stop,
                ),
                None => self.search_leg(&leg.input, &should_stop),
            });
            // The traveller's pick can come from a search at another weight, which this one
            // would not find again.
            if let Some(Ok(search)) = searches.last_mut() {
                self.add_pick_from_other_weights(
                    search,
                    &leg.input,
                    window_used.as_ref(),
                    &should_stop,
                );
            }
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
                    departure_window: searches[leg].departure_window.clone(),
                };
                self.leg_plan(&ranking, inputs[leg])
            })
            .collect()
    }

    /// Returns the departure window of a leg that follows a same-day stop: from the earliest
    /// arrival of the leg before plus the shortest stay less `SHORT_STAY_SEARCH_MINUTES`, to the
    /// latest arrival plus the longest stay. Without a longest stay it ends with the leg's own
    /// departure window. A departure window the traveller set, or the window of a night train,
    /// also bounds it. Returns `None` when the leg before has no journeys.
    fn window_after_stay(
        &self,
        previous: &LegSearch,
        stop: SameDayStop,
        input: &LegPlanningInput,
    ) -> Option<std::ops::Range<MinutesSinceBundleStart>> {
        let arrivals = previous.journeys.iter().map(|journey| journey.arrival.0);
        let earliest_arrival = arrivals.clone().min()?;
        let latest_arrival = arrivals.max()?;
        let own_window = self.departure_window(input);
        let minimum = u32::from(stop.stay_range.minimum.minutes());
        // A stay a little short of the minimum is searched too, so the pairing can weigh its
        // penalty against, for example, arriving much later at the next place.
        let start = earliest_arrival + minimum.saturating_sub(SHORT_STAY_SEARCH_MINUTES);
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
        let search = match self.search_leg_from_anchors(input, &|| false) {
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
                    journey: self.journey(ranked, input, ranking.night.as_ref(), &mut stations)?,
                    cost: ranked.cost,
                    cost_inputs: ranked.cost_inputs,
                    price: ranked.price,
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

    /// Searches a leg on single tickets like `search_leg`, with the bounds of Bounded McRAPTOR
    /// when the traveller weighs prices.
    ///
    /// A search without prices runs first. It is several times faster than one with prices,
    /// and its journeys are the anchors: the cheapest of them at the traveller's weight is a
    /// journey the priced search needs to beat. That cost goes to the priced search as its
    /// limit, so it drops every label that can't lead to a cheaper journey from the first train
    /// on, and turns into a deadline for the search backwards from the arrival place. The
    /// anchors then join the journeys of the priced search, so the plan never loses one of
    /// them. See Delling, Dibbelt and Pajor, "Fast and Exact Public Transit Routing with
    /// Restricted Pareto Sets" (ALENEX 2019).
    fn search_leg_from_anchors(
        &self,
        input: &LegPlanningInput,
        should_stop: &(impl Fn() -> bool + Sync),
    ) -> Result<LegSearch, NoJourneyReason> {
        let Some(prices) = self.ride_prices.as_ref() else {
            return self.search_leg(input, should_stop);
        };
        if input.price_weight == PriceWeight::TIME_FIRST {
            return self.search_leg(input, should_stop);
        }
        let mut plain = input.clone();
        plain.price_weight = PriceWeight::TIME_FIRST;
        plain.chosen_journey = None;
        let anchors = self.search_leg(&plain, should_stop)?;
        let mut search = self.search_leg_within_anchors(input, &anchors, prices, should_stop)?;
        for (journey, days) in anchors.journeys.into_iter().zip(anchors.days) {
            if !search.journeys.contains(&journey) {
                search.journeys.push(journey);
                search.days.push(days);
            }
        }
        Ok(search)
    }

    /// Searches a leg on single tickets at the traveller's weight, with the cost limit that the
    /// journeys of a search without prices give, as `search_leg_from_anchors` describes. The
    /// result leaves out the anchors themselves.
    fn search_leg_within_anchors(
        &self,
        input: &LegPlanningInput,
        anchors: &LegSearch,
        prices: &RidePrices,
        should_stop: &(impl Fn() -> bool + Sync),
    ) -> Result<LegSearch, NoJourneyReason> {
        let options = SearchOptions {
            maximum_cost: self.anchor_cost_limit(anchors, input, prices),
            ..SearchOptions::SINGLE
        };
        self.search_leg_in_mode(input, options, should_stop)
    }

    /// Returns the cost limit that the anchors give a priced search, in its unit of hundredths
    /// of a minute, or `None` without an anchor.
    ///
    /// Each anchor costs what the priced search would add up for it: the cost without the
    /// price, and the price of every ride as the search charges it. The limit is the cheapest
    /// anchor, with a margin. The ranking prices a journey from its tickets and with a dead
    /// band of ten percent, which the ride by ride price of the search only estimates, so a
    /// journey that ranks first can cost the search a little more than the anchor does. The
    /// margin is a tenth of the anchor's price cost and two minutes.
    fn anchor_cost_limit(
        &self,
        anchors: &LegSearch,
        input: &LegPlanningInput,
        prices: &RidePrices,
    ) -> Option<i32> {
        let weight = u64::from(input.price_weight.hundredths_of_minute_per_euro());
        let arrival_window = self.ranking_arrival_window(input, anchors.night.as_ref());
        anchors
            .journeys
            .iter()
            .map(|journey| {
                let cost_inputs =
                    self.journey_cost_inputs(journey, &arrival_window, anchors.night.as_ref());
                let minutes = journey_cost(
                    cost_inputs,
                    input.transfer_aversion,
                    ScoringWeights::DEFAULT,
                );
                let mut previous: Option<btt_router::PreviousRide> = None;
                let mut cents = 0_u64;
                for ride in &journey.rides {
                    cents += u64::from(prices.ride_price_cents(
                        ride.raptor_route,
                        ride.boarding_position,
                        ride.alighting_position,
                        previous,
                        PriceMode::Single,
                    ));
                    previous = Some(btt_router::PreviousRide {
                        raptor_route: ride.raptor_route,
                        boarding_position: ride.boarding_position,
                        alighting_position: ride.alighting_position,
                    });
                }
                let price_cost = weight * cents / 100;
                u64::from(minutes) * 100 + price_cost + price_cost / 10 + 200
            })
            .min()
            .map(|limit| i32::try_from(limit).unwrap_or(i32::MAX))
    }

    /// Finds the candidate stations of a leg and searches its Pareto set of journeys on
    /// single tickets.
    fn search_leg(
        &self,
        input: &LegPlanningInput,
        should_stop: &(impl Fn() -> bool + Sync),
    ) -> Result<LegSearch, NoJourneyReason> {
        self.search_leg_in_mode(input, SearchOptions::SINGLE, should_stop)
    }

    /// Finds the candidate stations of a leg and searches its Pareto set of journeys, priced
    /// as the mode says.
    fn search_leg_in_mode(
        &self,
        input: &LegPlanningInput,
        options: SearchOptions,
        should_stop: &(impl Fn() -> bool + Sync),
    ) -> Result<LegSearch, NoJourneyReason> {
        let departure_window = self
            .departure_window(input)
            .ok_or(NoJourneyReason::TravelDateOutsideTimetable)?;
        self.search_leg_in_window(input, departure_window, options, should_stop)
    }

    /// Searches a leg like `search_leg_in_mode`, leaving the departure place within a given
    /// window instead of the leg's own.
    fn search_leg_in_window(
        &self,
        input: &LegPlanningInput,
        departure_window: std::ops::Range<MinutesSinceBundleStart>,
        options: SearchOptions,
        should_stop: &(impl Fn() -> bool + Sync),
    ) -> Result<LegSearch, NoJourneyReason> {
        let searched_window = departure_window.clone();
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
        let night = self.night_in_bundle_minutes(input);
        // The search adds up the same cost the ranking uses, so no journey that would rank
        // first is pruned for leaving earlier or arriving later than another. At a weight
        // above zero the price of every ride enters it, and at weight zero the search skips
        // every price step. On the days of a pass a covered ride costs what the pass charges
        // on top, and the search also counts the travel days the journey uses.
        let price_weight_hundredths = u32::from(input.price_weight.hundredths_of_minute_per_euro());
        let model = LegCostModel {
            bundle: &self.bundle,
            weights: ScoringWeights::DEFAULT,
            transfer_cost_minutes: transfer_cost_minutes(
                input.transfer_aversion,
                ScoringWeights::DEFAULT,
            ),
            arrival_window: self.ranking_arrival_window(input, night.as_ref()),
            night: night.clone(),
            cost_limit_factor: options
                .cost_limit_factor
                .unwrap_or(MAXIMUM_ALTERNATIVE_COST_FACTOR),
            routes_with_berths: &self.routes_with_berths,
            price_weight_hundredths,
            prices: if price_weight_hundredths > 0 {
                self.ride_prices.as_ref()
            } else {
                None
            },
            price_mode: options.mode,
            maximum_duration_minutes: options.maximum_minutes,
            initial_cost_limit: options.maximum_cost.unwrap_or(i32::MAX),
        };
        let found = self.search_in_parallel(
            &departure_candidates,
            &arrival_candidates,
            departure_window,
            input,
            &model,
            should_stop,
        );
        let mut journeys = Vec::with_capacity(found.journeys.len());
        let mut days = Vec::with_capacity(found.journeys.len());
        for costed in found.journeys {
            journeys.push(costed.journey);
            days.push(costed.days);
        }
        Ok(LegSearch {
            departure_candidates,
            arrival_candidates,
            journeys,
            days,
            night,
            departure_window: searched_window,
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
            departure_window: search.departure_window.clone(),
        }
    }

    /// Returns every journey of a search with its cost, in the order of the search. The cost
    /// counts the price at the traveller's weight, so the ranking that orders these journeys
    /// by cost picks the cheapest one when the weight says so.
    fn journey_costs<'search>(
        &self,
        search: &'search LegSearch,
        input: &LegPlanningInput,
    ) -> Vec<RankedJourney<'search>> {
        let arrival_window = self.ranking_arrival_window(input, search.night.as_ref());
        let mut journeys: Vec<RankedJourney<'search>> = search
            .journeys
            .iter()
            .map(|journey| {
                let cost_inputs =
                    self.journey_cost_inputs(journey, &arrival_window, search.night.as_ref());
                let cost = journey_cost(
                    cost_inputs,
                    input.transfer_aversion,
                    ScoringWeights::DEFAULT,
                );
                RankedJourney {
                    journey,
                    cost_inputs,
                    cost,
                    own_cost: cost,
                    price: self.journey_price(journey, input, search.night.as_ref()),
                    price_cost: 0,
                }
            })
            .collect();
        add_price_costs(&mut journeys, input.price_weight);
        journeys
    }

    /// Returns the estimated price of one journey of the search, or `None` when no price is
    /// known for its trains. The price needs the journey as the plan shows it, so a model
    /// journey is built with a throwaway station table.
    fn journey_price(
        &self,
        journey: &RaptorJourney,
        input: &LegPlanningInput,
        night: Option<&std::ops::Range<u32>>,
    ) -> Option<EuroRange> {
        let mut stations = LegPlanStations::default();
        let journey = self.model_journey(journey, input, night, &mut stations)?;
        self.pricer.journey_price(&journey, &stations.table)
    }

    /// Returns the arrival window the cost uses, in bundle minutes. On an overnight leg,
    /// arriving before the wake-up time counts as arriving before the window.
    fn ranking_arrival_window(
        &self,
        input: &LegPlanningInput,
        night: Option<&std::ops::Range<u32>>,
    ) -> std::ops::Range<u32> {
        let mut arrival_window = self.arrival_window_in_bundle_minutes(input);
        if let Some(night) = night {
            arrival_window.start = arrival_window.start.max(night.end);
            arrival_window.end = arrival_window.end.max(arrival_window.start);
        }
        arrival_window
    }

    /// Returns the best journey of a leg searched at another price weight, costed at the
    /// traveller's weight like the plan's own journeys, with the stations it uses. Returns
    /// `None` when the search finds nothing. The traveller's pick does not hold, because the
    /// plan shows the other weight's own best. The bounds say how long and how costly the journeys
    /// the search looks for can be.
    fn best_journey_at(
        &self,
        input: &LegPlanningInput,
        weight: PriceWeight,
        bounds: SearchBounds,
        should_stop: &(impl Fn() -> bool + Sync),
    ) -> Option<ExtraJourney> {
        let mut weighted = input.clone();
        weighted.price_weight = weight;
        weighted.chosen_journey = None;
        let options = SearchOptions {
            mode: PriceMode::Single,
            maximum_minutes: bounds.minutes,
            maximum_cost: bounds.cost,
            cost_limit_factor: Some(1),
        };
        let search = self
            .search_leg_in_mode(&weighted, options, should_stop)
            .ok()?;
        let best = self
            .rank_journeys(&search, &weighted)
            .journeys
            .first()?
            .journey;
        // The cost parts must hold the price at the traveller's weight, so the journey compares
        // with the journeys of the plan.
        let at_travellers_weight = self.journey_costs(&search, input);
        let ranked = at_travellers_weight
            .iter()
            .find(|ranked| std::ptr::eq(ranked.journey, best))?;
        let mut stations = LegPlanStations::default();
        let journey = self.journey(ranked, input, search.night.as_ref(), &mut stations)?;
        Some(ExtraJourney {
            journey,
            stations: stations.table,
        })
    }

    /// Adds the traveller's pick to a search that doesn't have it. The pick can come from a
    /// search at another weight, such as the cheaper journey of the extras, which the search at
    /// the traveller's weight drops when it ties with another journey or loses to it on cost.
    /// The pick is then among the journeys of the search at another weight, and costed at the
    /// traveller's weight with the others. `window` is the departure window of a leg after a
    /// same-day stop, and `None` for the leg's own.
    fn add_pick_from_other_weights(
        &self,
        search: &mut LegSearch,
        input: &LegPlanningInput,
        window: Option<&std::ops::Range<MinutesSinceBundleStart>>,
        should_stop: &(impl Fn() -> bool + Sync),
    ) {
        let Some(choice) = input.chosen_journey.as_ref() else {
            return;
        };
        if search
            .journeys
            .iter()
            .any(|journey| self.journey_matches_choice(journey, choice))
        {
            return;
        }
        let find_pick = |searched: LegSearch| {
            searched
                .journeys
                .into_iter()
                .find(|journey| self.journey_matches_choice(journey, choice))
        };
        // The journeys of the search by time alone, which bound the search at the cheapest
        // weight like the anchors of `search_leg_from_anchors`.
        let mut plain: Option<LegSearch> = None;
        // The search by time alone is the quicker one, so it goes first.
        for weight in [PriceWeight::TIME_FIRST, PriceWeight::CHEAPEST] {
            if weight == input.price_weight {
                continue;
            }
            let mut other = input.clone();
            other.price_weight = weight;
            other.chosen_journey = None;
            let found = match window {
                Some(window) => self
                    .search_leg_in_window(
                        &other,
                        window.start..window.end,
                        SearchOptions::SINGLE,
                        should_stop,
                    )
                    .ok()
                    .and_then(find_pick),
                None if weight == PriceWeight::TIME_FIRST => {
                    let searched = self.search_leg(&other, should_stop).ok();
                    let found = searched.as_ref().and_then(|searched| {
                        searched
                            .journeys
                            .iter()
                            .find(|journey| self.journey_matches_choice(journey, choice))
                            .cloned()
                    });
                    plain = searched;
                    found
                }
                None => {
                    // A cheaper journey of the extras is the best one at the cheapest weight, so
                    // the bounded search finds it in a fraction of the time. A pick that is not
                    // the best there still gets the search without bounds.
                    let anchors = if input.price_weight == PriceWeight::TIME_FIRST {
                        Some(&*search)
                    } else {
                        plain.as_ref()
                    };
                    let bounded = match (anchors, self.ride_prices.as_ref()) {
                        (Some(anchors), Some(prices)) => self
                            .search_leg_within_anchors(&other, anchors, prices, should_stop)
                            .ok()
                            .and_then(find_pick),
                        _ => None,
                    };
                    bounded.or_else(|| {
                        self.search_leg(&other, should_stop)
                            .ok()
                            .and_then(find_pick)
                    })
                }
            };
            if let Some(journey) = found {
                search.journeys.push(journey);
                search.days.push(0);
                return;
            }
        }
    }

    /// Returns the best journeys of the leg on the travel days of a pass, one for each number of
    /// days the journey uses and each first date, which the solve of the whole trip picks
    /// among. The result is empty when the traveller searches by time alone or no pass takes a
    /// train of the bundle.
    fn pass_options(
        &self,
        input: &LegPlanningInput,
        should_stop: &(impl Fn() -> bool + Sync),
        table: &mut SmallVec<[LegPlanStation; 8]>,
    ) -> SmallVec<[PassJourneyOption; 3]> {
        let weight = u32::from(input.price_weight.hundredths_of_minute_per_euro());
        if weight == 0 {
            return SmallVec::new();
        }
        let Some(prices) = self.ride_prices.as_ref() else {
            return SmallVec::new();
        };
        if !prices.any_pass_takes() {
            return SmallVec::new();
        }
        let Ok(search) = self.search_leg_in_mode(input, SearchOptions::PASS, should_stop) else {
            return SmallVec::new();
        };
        // The best journey for a number of days and a first date is the one the solve would
        // pick: the lowest weighted total of its cost in minutes and what the pass does not
        // cover. The dates matter to the solve, because a pass is valid in a window of them.
        let arrival_window = self.ranking_arrival_window(input, search.night.as_ref());
        let mut best: SmallVec<[(u64, PassJourneyOption); 3]> = SmallVec::new();
        for (journey, &journey_days) in search.journeys.iter().zip(&search.days) {
            let Ok(days) = PassDays::new(journey_days) else {
                continue;
            };
            let Some(first_date) = Self::first_pass_day(journey, prices)
                .map(|day| self.bundle.validity_start() + chrono::Days::new(u64::from(day)))
            else {
                continue;
            };
            let cost_inputs =
                self.journey_cost_inputs(journey, &arrival_window, search.night.as_ref());
            let mut parts = journey_cost_parts(
                cost_inputs,
                input.transfer_aversion,
                ScoringWeights::DEFAULT,
            );
            let cost_minutes = u32::try_from(parts.total_cost).unwrap_or(0);
            let mut stations = LegPlanStations::default();
            let Some(mut model) =
                self.model_journey(journey, input, search.night.as_ref(), &mut stations)
            else {
                continue;
            };
            let Some(price) = self.pricer.journey_price_with_pass(&model, &stations.table) else {
                continue;
            };
            let price_cents = price.median();
            let price_cost = u64::from(weight) * u64::from(price_cents.0) / 100;
            let weighted = u64::from(cost_minutes) * 100 + price_cost;
            // The journey carries the pass price and its weight in its cost parts, so the leg
            // view shows and explains it like a journey on single tickets.
            parts.price = Some(price);
            parts.price_cost = u32::try_from(price_cost).unwrap_or(u32::MAX);
            parts.total_cost = parts
                .total_cost
                .saturating_add(i32::try_from(price_cost / 100).unwrap_or(i32::MAX));
            model.cost_parts = Some(parts);
            // The journey indexes the station table of the plan, like the other journeys.
            let Some(model) = absorb_journey(
                table,
                ExtraJourney {
                    journey: model,
                    stations: stations.table,
                },
            ) else {
                continue;
            };
            let option = PassJourneyOption {
                journey: model,
                cost_minutes,
                price_cents,
                days,
                first_date,
            };
            match best
                .iter_mut()
                .find(|(_, known)| known.days == days && known.first_date == first_date)
            {
                Some(known) if weighted < known.0 => *known = (weighted, option),
                Some(_) => {}
                None => best.push((weighted, option)),
            }
        }
        best.sort_by_key(|(_, option)| (option.days, option.first_date));
        best.into_iter().map(|(_, option)| option).collect()
    }

    /// Returns the travel day of the first ride a pass takes, as local days since the bundle
    /// starts, or `None` when the pass takes no ride of the journey.
    fn first_pass_day(journey: &RaptorJourney, prices: &RidePrices) -> Option<u16> {
        journey.rides.iter().find_map(|ride| {
            prices.pass_travel_day(ride.raptor_route, ride.boarding_position, ride.departure.0)
        })
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
            self.journey(chosen, input, ranking.night.as_ref(), &mut stations)
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
        // The planner's best journey, whose own cost lets the session compare the leg with
        // another day. A chain cost would also change with the other legs of the chain.
        let best_own_cost = ranking
            .journeys
            .iter()
            .min_by_key(|ranked| ranked.cost)
            .map_or(0, |ranked| ranked.own_cost);
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
            .filter_map(|ranked| self.journey(ranked, input, ranking.night.as_ref(), &mut stations))
            .collect();
        LegPlanningResult::Found(Box::new(LegPlan {
            chosen_journey,
            alternative_journeys,
            station_table: stations.table,
            follows_traveller_choice: ranking.follows_traveller_choice,
            best_journey_cost: best_own_cost,
            pass_options: SmallVec::new(),
            departure_window: Some(SearchedWindow {
                start: UnixTimeMinutes(
                    self.bundle_start_unix_minutes + ranking.departure_window.start.0,
                ),
                end: UnixTimeMinutes(
                    self.bundle_start_unix_minutes + ranking.departure_window.end.0,
                ),
            }),
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
    fn search_in_parallel(
        &self,
        departure_candidates: &[CandidateStation],
        arrival_candidates: &[CandidateStation],
        departure_window: std::ops::Range<MinutesSinceBundleStart>,
        input: &LegPlanningInput,
        model: &LegCostModel<'_>,
        should_stop: &(impl Fn() -> bool + Sync),
    ) -> CostParetoJourneySet {
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
            let found = search_leg_journeys_by_cost(
                &self.bundle,
                &self.search_index,
                &query,
                model,
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
            let searches: Vec<_> = window_slices(departure_window)
                .map(|slice_window| scope.spawn(|| search_slice(slice_window)))
                .collect();
            CostParetoJourneySet::merge(
                searches.into_iter().filter_map(|search| search.join().ok()),
            )
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

    /// Returns the night minutes covered by all trains with berths on the journey together.
    fn sleeping_minutes(&self, journey: &RaptorJourney, night: &std::ops::Range<u32>) -> u32 {
        journey
            .rides
            .iter()
            .filter(|ride| self.bundle.trip_offers_berths(ride.trip))
            .map(|ride| {
                ride.arrival
                    .0
                    .min(night.end)
                    .saturating_sub(ride.departure.0.max(night.start))
            })
            .sum()
    }

    /// Finds the train with berths that covers most of the night, with the minutes it covers.
    /// The leg row marks this train as the one to sleep on.
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
            let sleeping = self.sleeping_minutes(journey, night);
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
            minutes_before_arrival_window: arrival_window.start.saturating_sub(arrival),
            minutes_after_arrival_window: arrival.saturating_sub(arrival_window.end),
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

    /// Builds the model journey of a router journey, without cost parts, adding its stations
    /// to the table.
    fn model_journey(
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
            cost_parts: None,
            alternative_kind: None,
        })
    }

    /// Turns a router journey into a model journey with its cost parts, which hold the price
    /// the ranking weighed and what it cost.
    fn journey(
        &self,
        ranked: &RankedJourney<'_>,
        input: &LegPlanningInput,
        night: Option<&std::ops::Range<u32>>,
        stations: &mut LegPlanStations,
    ) -> Option<Journey> {
        let mut journey = self.model_journey(ranked.journey, input, night, stations)?;
        journey.cost_parts = Some(cost_parts(ranked, input));
        Some(journey)
    }

    /// Turns a router ride into a model ride.
    fn train_ride(
        &self,
        ride: &btt_router::RaptorRide,
        stations: &mut LegPlanStations,
    ) -> Option<TrainRide> {
        let (category, number, agency) = self.bundle.trip_train_label(ride.trip);
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
                operator: self.operators.operator_of(agency, category),
                agency: agency.into(),
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

/// How a search runs: with which prices, and with which limit on the time of the journeys.
#[derive(Debug, Clone, Copy)]
struct SearchOptions {
    /// Which tickets the rides are priced as.
    mode: PriceMode,
    /// The longest door-to-door time in minutes of the journeys the search is for, or `None`
    /// for no limit.
    maximum_minutes: Option<u32>,
    /// The cost in hundredths of a minute that no journey the search is for exceeds, because a
    /// journey that costs that much is known, or `None` for no limit.
    maximum_cost: Option<i32>,
    /// How many times the cost of the best journey found so far a journey may cost to be worth
    /// searching for, or `None` for the factor that keeps the alternatives of a plan. A search
    /// for the best journey alone needs no more than 1.
    cost_limit_factor: Option<u32>,
}

impl SearchOptions {
    /// Prices rides as single tickets, with no limit.
    const SINGLE: Self = Self {
        mode: PriceMode::Single,
        maximum_minutes: None,
        maximum_cost: None,
        cost_limit_factor: None,
    };

    /// Prices rides for the days of a pass, with no limit.
    const PASS: Self = Self {
        mode: PriceMode::Pass,
        maximum_minutes: None,
        maximum_cost: None,
        cost_limit_factor: None,
    };
}

/// What a search that looks for an alternative to the chosen journey can leave out.
#[derive(Debug, Clone, Copy, Default)]
struct SearchBounds {
    /// The longest door-to-door time in minutes of a journey worth finding.
    minutes: Option<u32>,
    /// The cost in hundredths of a minute that no journey worth finding exceeds.
    cost: Option<i32>,
}

impl SearchBounds {
    /// How much slack the cost bound has, in hundredths of a minute. The search adds up its
    /// cost without the rounding of the ranking, which differs by less than a minute.
    const COST_SLACK: i32 = 200;

    /// Returns the bounds of the search at the cheapest weight for a journey that is cheaper
    /// than the chosen one. The leg plan shows such a journey only when it takes at most
    /// `CHEAPEST_EXTRA_MINUTES` longer. The chosen journey is a result of the search too, when
    /// the leg is on its own, so a journey that costs more than it does at the cheapest weight
    /// can't be the best.
    fn for_cheaper_than(chosen: &Journey, on_its_own: bool) -> Self {
        let Some(parts) = chosen.cost_parts else {
            return Self::default();
        };
        let cost = parts.price.filter(|_| on_its_own).map(|price| {
            let own = Self::cost_without_price(&parts);
            let price_cost = i64::from(PriceWeight::CHEAPEST.hundredths_of_minute_per_euro())
                * i64::from(price.median().0)
                / 100;
            i32::try_from(i64::from(own) * 100 + price_cost + i64::from(Self::COST_SLACK))
                .unwrap_or(i32::MAX)
        });
        Self {
            minutes: Some(parts.door_to_door_minutes + CHEAPEST_EXTRA_MINUTES),
            cost,
        }
    }

    /// Returns the bounds of the search without a weight for a journey that is faster than the
    /// chosen one: none cost more by time than the chosen journey, when the leg is on its own.
    fn for_faster_than(chosen: &Journey, on_its_own: bool) -> Self {
        let cost = chosen.cost_parts.filter(|_| on_its_own).map(|parts| {
            i32::try_from(i64::from(Self::cost_without_price(&parts)) * 100)
                .unwrap_or(i32::MAX)
                .saturating_add(Self::COST_SLACK)
        });
        Self {
            minutes: None,
            cost,
        }
    }

    /// Returns what the cost parts of a journey add up to without the price and without the
    /// rest of a chain, in minutes.
    fn cost_without_price(parts: &JourneyCostParts) -> i32 {
        parts
            .total_cost
            .saturating_sub(i32::try_from(parts.price_cost / 100).unwrap_or(i32::MAX))
            .saturating_sub(parts.rest_of_chain_cost)
    }
}

/// A journey that a search at another weight found, with the table of the stations it uses.
struct ExtraJourney {
    /// The journey, which indexes `stations`.
    journey: Journey,
    /// The stations of the journey.
    stations: SmallVec<[LegPlanStation; 8]>,
}

/// Moves a journey into the station table of a plan, adding the stations the table lacks.
/// Returns `None` when the table has no room for them.
fn absorb_journey(
    table: &mut SmallVec<[LegPlanStation; 8]>,
    extra: ExtraJourney,
) -> Option<Journey> {
    let mut journey = extra.journey;
    let mapping: Option<Vec<LegPlanStationIndex>> = extra
        .stations
        .into_iter()
        .map(|station| {
            let position = table
                .iter()
                .position(|known| *known == station)
                .unwrap_or_else(|| {
                    table.push(station);
                    table.len() - 1
                });
            u8::try_from(position).ok().map(LegPlanStationIndex)
        })
        .collect();
    journey.remap_stations(&mapping?);
    Some(journey)
}

/// Returns whether two journeys leave at the same time and take the same trains.
fn same_trains(first: &Journey, second: &Journey) -> bool {
    first.first_ride.departure_time == second.first_ride.departure_time
        && first.rides().count() == second.rides().count()
        && first
            .rides()
            .zip(second.rides())
            .all(|(a, b)| a.train_label == b.train_label)
}

/// Returns whether a journey leaves and arrives when another does, costs the planner no more
/// and has a lower price, so it is the same trip for less.
fn is_the_same_trip_for_less(candidate: &Journey, chosen: &Journey) -> bool {
    let (Some(candidate_parts), Some(chosen_parts)) = (candidate.cost_parts, chosen.cost_parts)
    else {
        return false;
    };
    let (Some(candidate_price), Some(chosen_price)) = (candidate_parts.price, chosen_parts.price)
    else {
        return false;
    };
    candidate.first_ride.departure_time == chosen.first_ride.departure_time
        && candidate.last_ride().arrival_time == chosen.last_ride().arrival_time
        && candidate_parts.total_cost <= chosen_parts.total_cost
        && candidate_price.median() < chosen_price.median()
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
    /// The journeys that no other beats on departure, arrival, number of trains and cost.
    journeys: Vec<RaptorJourney>,
    /// How many travel days each journey uses, in the order of `journeys`. Zero on single
    /// tickets and on a journey no pass covers.
    days: Vec<u8>,
    /// The night of an overnight leg in bundle minutes.
    night: Option<std::ops::Range<u32>>,
    /// The moments between which the traveller could leave the departure place.
    departure_window: std::ops::Range<MinutesSinceBundleStart>,
}

/// A journey of a search with its cost.
struct RankedJourney<'search> {
    /// The journey.
    journey: &'search RaptorJourney,
    /// What the cost came from.
    cost_inputs: JourneyCostInputs,
    /// The cost, lower is better. For a leg of a chain, the cost of the best chain through
    /// the journey.
    cost: u32,
    /// The cost of the journey on its own.
    own_cost: u32,
    /// The estimated price of the journey's tickets and night accommodation, or `None` when no
    /// price is known for its trains.
    price: Option<EuroRange>,
    /// What the price costs at the traveller's price weight, in hundredths of a minute of
    /// train travel.
    price_cost: u32,
}

/// The journeys of a search in the order the planner shows them.
struct JourneyRanking<'search> {
    /// Chosen journey first, then the alternatives.
    journeys: Vec<RankedJourney<'search>>,
    /// Whether the first journey is the traveller's pick.
    follows_traveller_choice: bool,
    /// The night of an overnight leg in bundle minutes.
    night: Option<std::ops::Range<u32>>,
    /// The moments between which the traveller could leave the departure place.
    departure_window: std::ops::Range<MinutesSinceBundleStart>,
}

/// Rounds a distance to whole meters. Distances here stay far below the range of `u32`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn rounded_meters(distance_meters: f64) -> u32 {
    distance_meters.round() as u32
}

/// Returns what the cost of a ranked journey is made of. For a leg of a chain, everything the
/// rest of the chain adds goes into `rest_of_chain_cost`, so the parts add up to the cost the
/// journey was ranked by.
/// Returns whether a journey is worth showing as a cheaper one next to the chosen journey:
/// it saves at least a share or a fixed amount of the chosen journey's price, and it takes at
/// most a bounded time longer.
fn cheaper_by_enough(cheapest: &Journey, chosen: &Journey) -> bool {
    let Some(price) = cheapest.cost_parts.and_then(|parts| parts.price) else {
        return false;
    };
    let Some(chosen_price) = chosen.cost_parts.and_then(|parts| parts.price) else {
        return false;
    };
    if JourneyChoice::of_journey(cheapest) == JourneyChoice::of_journey(chosen) {
        return false;
    }
    let saving = chosen_price.median().0.saturating_sub(price.median().0);
    let saves_enough = saving.saturating_mul(100)
        >= chosen_price
            .median()
            .0
            .saturating_mul(CHEAPEST_SAVING_PERCENT)
        || saving >= CHEAPEST_SAVING_CENTS;
    let extra_minutes = cheapest
        .cost_parts
        .map_or(u32::MAX, |parts| parts.door_to_door_minutes)
        .saturating_sub(
            chosen
                .cost_parts
                .map_or(u32::MAX, |parts| parts.door_to_door_minutes),
        );
    saves_enough && extra_minutes <= CHEAPEST_EXTRA_MINUTES
}

/// Returns whether a journey is worth showing as a faster one next to the chosen journey: it
/// takes at least a fixed time less, and its price is at most a bounded share above the chosen
/// journey's.
fn faster_by_enough(fastest: &Journey, chosen: &Journey) -> bool {
    let Some(price) = fastest.cost_parts.and_then(|parts| parts.price) else {
        return false;
    };
    let Some(chosen_price) = chosen.cost_parts.and_then(|parts| parts.price) else {
        return false;
    };
    if JourneyChoice::of_journey(fastest) == JourneyChoice::of_journey(chosen) {
        return false;
    }
    let saving_minutes = chosen
        .cost_parts
        .map_or(0, |parts| parts.door_to_door_minutes)
        .saturating_sub(
            fastest
                .cost_parts
                .map_or(0, |parts| parts.door_to_door_minutes),
        );
    let price_within_bound = price.median().0.saturating_mul(100)
        <= chosen_price
            .median()
            .0
            .saturating_mul(100 + FASTEST_EXTRA_PRICE_PERCENT);
    saving_minutes >= FASTEST_SAVING_MINUTES && price_within_bound
}

fn cost_parts(ranked: &RankedJourney<'_>, input: &LegPlanningInput) -> JourneyCostParts {
    let mut parts = journey_cost_parts(
        ranked.cost_inputs,
        input.transfer_aversion,
        ScoringWeights::DEFAULT,
    );
    let cost = i32::try_from(ranked.cost).unwrap_or(i32::MAX);
    // The price counts in hundredths of a minute, and the cost keeps whole minutes, so the
    // parts add up exactly.
    let price_cost_minutes = i32::try_from(ranked.price_cost / 100).unwrap_or(i32::MAX);
    parts.price = ranked.price;
    parts.price_cost = ranked.price_cost;
    parts.rest_of_chain_cost = cost - parts.total_cost - price_cost_minutes;
    parts.total_cost = cost;
    parts
}

/// Fills what the price of each ranked journey costs and adds it to the cost the ranking
/// orders them by.
///
/// The cheapest price of the leg is the anchor of a dead band: a price within ten percent of
/// it counts as the cheapest itself, because the model that estimates prices is off by more
/// than that, so two journeys that close in price decide on their times. Without a weight the
/// price changes no cost, and the ranking picks by time alone.
fn add_price_costs(journeys: &mut [RankedJourney<'_>], weight: PriceWeight) {
    let weight_hundredths = u32::from(weight.hundredths_of_minute_per_euro());
    if weight_hundredths == 0 {
        return;
    }
    let Some(cheapest) = journeys
        .iter()
        .filter_map(|ranked| ranked.price.map(|price| price.median().0))
        .min()
    else {
        return;
    };
    let band = cheapest + cheapest / 10;
    for ranked in journeys {
        let price_cost = ranked
            .price
            .map(|price| {
                let cents = if price.median().0 <= band {
                    cheapest
                } else {
                    price.median().0
                };
                u64::from(weight_hundredths) * u64::from(cents) / 100
            })
            .map_or(0, |hundredths| {
                u32::try_from(hundredths).unwrap_or(u32::MAX)
            });
        ranked.cost += price_cost / 100;
        ranked.own_cost = ranked.cost;
        ranked.price_cost = price_cost;
    }
}

#[cfg(test)]
mod tests {
    use btt_geo::TimeZoneFinder;
    use btt_model::{
        DailyTimeWindow, DurationMinutes, EuroCents, EuroRange, GeoCoordinates,
        InterrailPassPolicy, LegTravelMode, LocalTimeOfDay, StationSearchRadius, TransferAversion,
    };
    use btt_timetable::{
        BundleBuilder, BundleStationIndex, ServiceDayBitset, StationInput, TrainLabelInput,
        TripInput, TripStopInput,
    };
    use chrono::NaiveDate;

    use super::*;

    /// The price row of a flat price in euro cents: a constant log price.
    fn flat_price_row(cents: EuroCents) -> crate::pricing::RidePriceRow {
        #[expect(clippy::cast_possible_truncation)]
        let constant = (f64::from(cents.0) / 100.0).ln() as f32;
        crate::pricing::RidePriceRow {
            constant,
            distance: 0.0,
            curve: 0.0,
            base: 0.0,
            shift: 0.0,
        }
    }

    /// A pricer that returns the same range for every journey, which shows that prices reach
    /// the plan without touching the ranking. The search prices every ride at the middle of
    /// the range.
    #[derive(Debug, Clone, Copy)]
    struct ConstantPricer(EuroRange);

    impl JourneyPricer for ConstantPricer {
        fn journey_price(&self, _: &Journey, _: &[LegPlanStation]) -> Option<EuroRange> {
            Some(self.0)
        }

        fn price_table(
            &self,
            classes: &[crate::pricing::RidePriceClass],
        ) -> Option<crate::pricing::RidePriceTable> {
            Some(crate::pricing::RidePriceTable {
                rows: classes
                    .iter()
                    .map(|_| flat_price_row(self.0.median()))
                    .collect(),
                reference_kilometres: 1.0,
            })
        }
    }

    /// A bundle with one direct ICE and one regional train that stops on the way, between the
    /// outer stations on one day. The ICE takes 25 minutes and the regional train 50, and the
    /// regional train leaves `regional_departure` minutes into the day, so tests can make it
    /// leave together with the ICE or later. The two stop patterns make the trains two routes,
    /// without which the search would board only the first of them.
    fn bundle(regional_departure: u16) -> LoadedTimetableBundle {
        bundle_with_ice_arrival(505, regional_departure)
    }

    /// Builds the bundle with the ICE arriving `ice_arrival` minutes into the day, so a test
    /// can make it faster than the shared 25 minutes.
    fn bundle_with_ice_arrival(ice_arrival: u16, regional_departure: u16) -> LoadedTimetableBundle {
        let mut builder =
            BundleBuilder::new(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(), 2, 1).unwrap();
        for (name, latitude) in [("S0", 50.0), ("SM", 50.025), ("S1", 50.05)] {
            builder
                .add_station(StationInput {
                    name: name.into(),
                    coordinates: GeoCoordinates::from_degrees(latitude, 8.0).unwrap(),
                    time_zone: "Europe/Berlin".into(),
                    trainline_station_id: None,
                })
                .unwrap();
        }
        let mut day = ServiceDayBitset::default();
        day.insert(0);
        let trip = |stops: &[(u32, u16)]| {
            let stop = |(station, time): &(u32, u16)| TripStopInput {
                station: BundleStationIndex(*station),
                arrival: *time,
                departure: *time,
                boarding_allowed: true,
                alighting_allowed: true,
            };
            TripInput {
                stops: stops.iter().map(stop).collect(),
                service_days: day,
                train_label: TrainLabelInput {
                    category: "ICE".into(),
                    number: "1".into(),
                    agency: "DB Fernverkehr AG".into(),
                    offers_berths: false,
                },
            }
        };
        // The direct ICE from station 0 to station 2, leaving at 480.
        builder.add_trip(trip(&[(0, 480), (2, ice_arrival)]));
        // The regional train from station 0 over the middle station to station 2.
        let middle = regional_departure + 25;
        builder.add_trip(TripInput {
            stops: vec![
                TripStopInput {
                    station: BundleStationIndex(0),
                    arrival: regional_departure,
                    departure: regional_departure,
                    boarding_allowed: true,
                    alighting_allowed: true,
                },
                TripStopInput {
                    station: BundleStationIndex(1),
                    arrival: middle,
                    departure: middle,
                    boarding_allowed: true,
                    alighting_allowed: true,
                },
                TripStopInput {
                    station: BundleStationIndex(2),
                    arrival: regional_departure + 50,
                    departure: regional_departure + 50,
                    boarding_allowed: true,
                    alighting_allowed: true,
                },
            ],
            service_days: day,
            train_label: TrainLabelInput {
                category: "RE".into(),
                number: "2".into(),
                agency: "DB Fernverkehr AG".into(),
                offers_berths: false,
            },
        });
        LoadedTimetableBundle::from_bundle(&builder.build().unwrap()).unwrap()
    }

    /// The leg between the outer stations of the bundle, inside wide windows. The search
    /// radius keeps the middle station of the regional train out of the places' candidates.
    fn leg_input(price_weight: PriceWeight) -> LegPlanningInput {
        let time = |hour| LocalTimeOfDay::from_hour_and_minute(hour, 0).unwrap();
        let endpoint = |latitude| crate::input::LegEndpoint {
            coordinates: GeoCoordinates::from_degrees(latitude, 8.0).unwrap(),
            station_search_radius: StationSearchRadius::new(1_000).unwrap(),
            time_window: DailyTimeWindow::from_start_and_end(time(6), time(20)),
        };
        LegPlanningInput {
            departure: endpoint(50.0),
            arrival: endpoint(50.05),
            departure_date: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            travel_mode: LegTravelMode::Daytime,
            minimum_transfer_time: DurationMinutes(10),
            pass_policy: InterrailPassPolicy::PreferPass,
            transfer_aversion: TransferAversion::new(50).unwrap(),
            price_weight,
            chosen_journey: None,
        }
    }

    /// Returns the found plan of a leg planned with a pricer on a bundle whose regional train
    /// leaves `regional_departure` minutes into the day.
    fn planned_leg<P: JourneyPricer>(
        pricer: P,
        price_weight: PriceWeight,
        regional_departure: u16,
    ) -> Box<btt_model::LegPlan> {
        planned_leg_on_bundle(pricer, price_weight, bundle(regional_departure))
    }

    /// Plans the leg on a given bundle, for tests that need other train times.
    fn planned_leg_on_bundle<P: JourneyPricer>(
        pricer: P,
        price_weight: PriceWeight,
        bundle: LoadedTimetableBundle,
    ) -> Box<btt_model::LegPlan> {
        let router =
            TimetableBundleRouter::from_loaded(bundle, Arc::new(TimeZoneFinder::new()), pricer);
        let LegPlanningResult::Found(plan) =
            router.plan_leg_with_extras(&leg_input(price_weight), || false)
        else {
            panic!("the leg has a journey");
        };
        plan
    }

    #[test]
    fn the_anchors_keep_the_journey_an_unbounded_search_ranks_first() {
        for (ice, regional) in [(12_000, 3_500), (4_300, 3_500), (6_000, 3_500)] {
            let pricer = CategoryPricer {
                ice: EuroCents(ice),
                regional: EuroCents(regional),
            };
            let router = TimetableBundleRouter::from_loaded(
                bundle(480),
                Arc::new(TimeZoneFinder::new()),
                pricer,
            );
            for weight in [
                PriceWeight::BALANCED,
                PriceWeight::FRUGAL,
                PriceWeight::CHEAPEST,
            ] {
                let input = leg_input(weight);
                // The priced search alone, without anchors or bounds.
                let unbounded = router.search_leg(&input, &|| false).unwrap();
                let ranking = router.rank_journeys(&unbounded, &input);
                let expected = ranking.journeys.first().unwrap().journey;
                let bounded = router.search_leg_from_anchors(&input, &|| false).unwrap();
                let ranking = router.rank_journeys(&bounded, &input);
                assert_eq!(ranking.journeys.first().unwrap().journey, expected);
            }
        }
    }

    #[test]
    fn the_main_plan_comes_without_the_extra_searches() {
        let pricer = PassCategoryPricer {
            ice: EuroCents(12_000),
            regional: EuroCents(3_500),
            ice_reservation: EuroCents(1_000),
        };
        let router = TimetableBundleRouter::from_loaded(
            bundle(480),
            Arc::new(TimeZoneFinder::new()),
            pricer,
        );
        let input = leg_input(PriceWeight::FRUGAL);
        let LegPlanningResult::Found(mut plan) = router.plan_leg(&input, || false) else {
            panic!("the leg has a journey");
        };
        // The main plan has the chosen journey and none of what the extra searches find.
        assert_eq!(plan.chosen_journey.first_ride.train_label.category, "RE");
        assert!(
            plan.alternative_journeys
                .iter()
                .all(|journey| journey.alternative_kind.is_none())
        );
        assert!(plan.pass_options.is_empty());
        // A leg joined to others has no pass options.
        let joined = router.plan_extras(&input, &plan, false, || false);
        assert!(joined.pass_options.is_empty());
        // The extras follow from the plan, and applying them gives the complete plan.
        let extras = router.plan_extras(&input, &plan, true, || false);
        assert_eq!(extras.pass_options.len(), 1);
        plan.apply_extras(extras);
        assert_eq!(plan.pass_options.len(), 1);
        assert_eq!(joined.alternative_journeys, plan.alternative_journeys);
    }

    /// Returns the alternative that a search at another weight found, of a kind.
    fn alternative_of_kind(plan: &btt_model::LegPlan, kind: AlternativeKind) -> Option<&Journey> {
        plan.alternative_journeys
            .iter()
            .find(|journey| journey.alternative_kind == Some(kind))
    }

    #[test]
    fn the_price_reaches_the_plan_without_changing_the_ranking() {
        let unpriced = planned_leg(NoPrice, PriceWeight::TIME_FIRST, 480);
        let range = EuroRange::new(EuroCents(3000), EuroCents(4000), EuroCents(6000)).unwrap();
        let priced = planned_leg(ConstantPricer(range), PriceWeight::TIME_FIRST, 480);
        // The chosen journey is the same one, at the same cost.
        assert_eq!(
            unpriced.chosen_journey.first_ride.departure_time,
            priced.chosen_journey.first_ride.departure_time
        );
        assert_eq!(
            unpriced
                .chosen_journey
                .cost_parts
                .as_ref()
                .unwrap()
                .total_cost,
            priced
                .chosen_journey
                .cost_parts
                .as_ref()
                .unwrap()
                .total_cost
        );
        assert_eq!(unpriced.chosen_journey.cost_parts.unwrap().price, None);
        assert_eq!(priced.chosen_journey.cost_parts.unwrap().price, Some(range));
    }

    /// A pricer that charges each train by its category, which tells the fast expensive train
    /// from the slow cheap one.
    #[derive(Clone, Copy)]
    struct CategoryPricer {
        /// Price of the ICE, in euro cents.
        ice: EuroCents,
        /// Price of the regional train, in euro cents.
        regional: EuroCents,
    }

    impl JourneyPricer for CategoryPricer {
        fn journey_price(&self, journey: &Journey, _: &[LegPlanStation]) -> Option<EuroRange> {
            let cents = if journey.first_ride.train_label.category == "ICE" {
                self.ice
            } else {
                self.regional
            };
            Some(EuroRange::new(cents, cents, cents).unwrap())
        }

        fn price_table(
            &self,
            classes: &[crate::pricing::RidePriceClass],
        ) -> Option<crate::pricing::RidePriceTable> {
            Some(crate::pricing::RidePriceTable {
                rows: classes
                    .iter()
                    .map(|class| {
                        let cents = if class.category == "ICE" {
                            self.ice
                        } else {
                            self.regional
                        };
                        flat_price_row(cents)
                    })
                    .collect(),
                reference_kilometres: 1.0,
            })
        }
    }

    #[test]
    fn a_weight_lets_a_cheap_slow_train_win() {
        let pricer = CategoryPricer {
            ice: EuroCents(12_000),
            regional: EuroCents(3_500),
        };
        // Without a weight the fast train wins, and at three minutes per euro the cheap one
        // saves more than the extra 35 minutes it takes.
        let by_time = planned_leg(pricer, PriceWeight::TIME_FIRST, 480);
        assert_eq!(
            by_time.chosen_journey.first_ride.train_label.category,
            "ICE"
        );
        let by_price = planned_leg(pricer, PriceWeight::FRUGAL, 480);
        assert_eq!(
            by_price.chosen_journey.first_ride.train_label.category,
            "RE"
        );
        let parts = by_price.chosen_journey.cost_parts.unwrap();
        // Three minutes per euro and 35 euro of price count 105 minutes on top of the 50 the
        // train takes.
        assert_eq!(parts.price_cost, 10_500);
        assert_eq!(parts.total_cost, 155);
    }

    #[test]
    fn prices_within_the_dead_band_decide_on_times() {
        // The fast train is within ten percent of the cheap one, so the model cannot tell
        // them apart and the ranking keeps the faster journey.
        let pricer = CategoryPricer {
            ice: EuroCents(3_700),
            regional: EuroCents(3_500),
        };
        let within_band = planned_leg(pricer, PriceWeight::FRUGAL, 490);
        assert_eq!(
            within_band.chosen_journey.first_ride.train_label.category,
            "ICE"
        );
        let parts = within_band.chosen_journey.cost_parts.unwrap();
        // Both count as the cheapest price, 35 euro at three minutes per euro.
        assert_eq!(parts.price_cost, 10_500);
        assert_eq!(parts.total_cost, 130);
    }

    #[test]
    fn the_search_drops_the_cheap_train_only_without_a_weight() {
        let pricer = CategoryPricer {
            ice: EuroCents(6_000),
            regional: EuroCents(3_500),
        };
        // By time alone the regional train is dominated: it leaves no earlier, arrives no
        // earlier and takes longer. At a weight its lower price keeps it in the search, and
        // the plan shows the fast train as the alternative.
        let ranked = |plan: &btt_model::LegPlan| {
            plan.alternative_journeys
                .iter()
                .filter(|journey| journey.alternative_kind.is_none())
                .map(|journey| journey.first_ride.train_label.category.to_string())
                .collect::<Vec<_>>()
        };
        let by_time = planned_leg(pricer, PriceWeight::TIME_FIRST, 480);
        assert!(ranked(&by_time).is_empty());
        let by_price = planned_leg(pricer, PriceWeight::FRUGAL, 480);
        assert_eq!(ranked(&by_price), ["ICE"]);
    }

    #[test]
    fn the_plan_shows_a_much_cheaper_journey_at_a_low_weight() {
        let pricer = CategoryPricer {
            ice: EuroCents(12_000),
            regional: EuroCents(3_500),
        };
        // At no weight the chosen journey is the fast expensive one, and the cheapest search
        // finds the regional train, which saves 85 euro for 25 minutes more.
        let plan = planned_leg(pricer, PriceWeight::TIME_FIRST, 480);
        assert_eq!(plan.chosen_journey.first_ride.train_label.category, "ICE");
        let cheaper = alternative_of_kind(&plan, AlternativeKind::Cheaper).unwrap();
        assert_eq!(cheaper.first_ride.train_label.category, "RE");
        assert!(
            alternative_of_kind(&plan, AlternativeKind::Faster).is_none(),
            "no weight means no faster search"
        );
        // The cheaper journey costs what it costs at the traveller's weight, like the others.
        let parts = cheaper.cost_parts.unwrap();
        assert_eq!(parts.price_cost, 0);
    }

    #[test]
    fn no_cheaper_journey_shows_when_the_saving_is_small() {
        let pricer = CategoryPricer {
            ice: EuroCents(3_700),
            regional: EuroCents(3_500),
        };
        // The prices lie within the noise of the model, so the cheapest search picks the same
        // fast train and nothing cheaper shows.
        let plan = planned_leg(pricer, PriceWeight::TIME_FIRST, 480);
        assert!(alternative_of_kind(&plan, AlternativeKind::Cheaper).is_none());
    }

    #[test]
    fn the_plan_shows_a_much_faster_journey_at_a_weight() {
        let pricer = CategoryPricer {
            ice: EuroCents(4_300),
            regional: EuroCents(3_500),
        };
        // At the cheapest weight the chosen journey is the regional train, and the search
        // without a weight finds an ICE that saves 35 minutes for 8 euro more.
        let plan = planned_leg_on_bundle(
            pricer,
            PriceWeight::CHEAPEST,
            bundle_with_ice_arrival(495, 480),
        );
        assert_eq!(plan.chosen_journey.first_ride.train_label.category, "RE");
        let faster = alternative_of_kind(&plan, AlternativeKind::Faster).unwrap();
        assert_eq!(faster.first_ride.train_label.category, "ICE");
        assert!(
            alternative_of_kind(&plan, AlternativeKind::Cheaper).is_none(),
            "the cheapest weight needs no cheaper search"
        );
    }

    #[test]
    fn no_faster_journey_shows_when_it_costs_too_much_more() {
        let pricer = CategoryPricer {
            ice: EuroCents(6_000),
            regional: EuroCents(3_500),
        };
        // The ICE saves the 35 minutes but costs 71 percent more than the regional train,
        // which is more than the bound on the faster journey.
        let plan = planned_leg_on_bundle(
            pricer,
            PriceWeight::CHEAPEST,
            bundle_with_ice_arrival(495, 480),
        );
        assert!(alternative_of_kind(&plan, AlternativeKind::Faster).is_none());
    }

    /// A bundle where the ICE and the regional train leave and arrive together, so only their
    /// prices tell them apart.
    fn bundle_of_equal_times() -> LoadedTimetableBundle {
        bundle_with_ice_arrival(530, 480)
    }

    #[test]
    fn a_cheaper_journey_with_the_same_times_becomes_the_chosen_one() {
        let pricer = CategoryPricer {
            ice: EuroCents(12_000),
            regional: EuroCents(3_500),
        };
        // Without a weight the two trains tie, and the search keeps one of them. The extras
        // find the cheaper one with the same times, which then is the chosen journey, and the
        // other is an alternative.
        let plan = planned_leg_on_bundle(pricer, PriceWeight::TIME_FIRST, bundle_of_equal_times());
        assert_eq!(plan.chosen_journey.first_ride.train_label.category, "RE");
        assert!(
            plan.alternative_journeys
                .iter()
                .any(|journey| journey.first_ride.train_label.category == "ICE")
        );
    }

    #[test]
    fn the_travellers_pick_stays_when_a_cheaper_journey_has_the_same_times() {
        let pricer = CategoryPricer {
            ice: EuroCents(12_000),
            regional: EuroCents(3_500),
        };
        let router = TimetableBundleRouter::from_loaded(
            bundle_of_equal_times(),
            Arc::new(TimeZoneFinder::new()),
            pricer,
        );
        let mut input = leg_input(PriceWeight::TIME_FIRST);
        let LegPlanningResult::Found(plan) = router.plan_leg_with_extras(&input, || false) else {
            panic!("the leg has a journey");
        };
        // The traveller picks the expensive train, whichever one the plan had.
        let ice = std::iter::once(&plan.chosen_journey)
            .chain(&plan.alternative_journeys)
            .find(|journey| journey.first_ride.train_label.category == "ICE")
            .unwrap();
        input.chosen_journey = Some(JourneyChoice::of_journey(ice));
        let LegPlanningResult::Found(picked) = router.plan_leg_with_extras(&input, || false) else {
            panic!("the leg has a journey");
        };
        assert!(picked.follows_traveller_choice);
        assert_eq!(picked.chosen_journey.first_ride.train_label.category, "ICE");
    }

    #[test]
    fn a_pick_that_only_another_weight_finds_is_still_found() {
        let pricer = CategoryPricer {
            ice: EuroCents(12_000),
            regional: EuroCents(3_500),
        };
        let router = TimetableBundleRouter::from_loaded(
            bundle_of_equal_times(),
            Arc::new(TimeZoneFinder::new()),
            pricer,
        );
        let input = leg_input(PriceWeight::TIME_FIRST);
        // One of the two tied trains is not among the journeys of the search without a weight.
        // Picking either of them must work, so the one the search drops comes from another
        // weight.
        for category in ["ICE", "RE"] {
            let LegPlanningResult::Found(plan) = router.plan_leg_with_extras(&input, || false)
            else {
                panic!("the leg has a journey");
            };
            let wanted = std::iter::once(&plan.chosen_journey)
                .chain(&plan.alternative_journeys)
                .find(|journey| journey.first_ride.train_label.category == category)
                .unwrap();
            let mut picking = input.clone();
            picking.chosen_journey = Some(JourneyChoice::of_journey(wanted));
            let LegPlanningResult::Found(picked) = router.plan_leg(&picking, || false) else {
                panic!("the leg has a journey");
            };
            assert!(picked.follows_traveller_choice, "{category} was picked");
            assert_eq!(
                picked.chosen_journey.first_ride.train_label.category,
                category
            );
        }
    }

    #[test]
    fn the_journeys_of_the_extras_index_the_station_table_of_the_plan() {
        let pricer = PassCategoryPricer {
            ice: EuroCents(12_000),
            regional: EuroCents(3_500),
            ice_reservation: EuroCents(1_000),
        };
        let plan = planned_leg(pricer, PriceWeight::FRUGAL, 480);
        let journeys = std::iter::once(&plan.chosen_journey)
            .chain(&plan.alternative_journeys)
            .chain(plan.pass_options.iter().map(|option| &option.journey));
        for journey in journeys {
            for ride in journey.rides() {
                for station in [ride.boarding_station, ride.alighting_station] {
                    assert!(usize::from(station.0) < plan.station_table.len());
                }
            }
        }
        // The pass journey boards where the chosen journey does, and the table names it so.
        let option = &plan.pass_options[0];
        let name = |journey: &Journey| {
            plan.station_table[usize::from(journey.first_ride.boarding_station.0)]
                .name
                .clone()
        };
        assert_eq!(name(&option.journey), name(&plan.chosen_journey));
    }

    #[test]
    fn a_duration_limit_leaves_out_the_slower_journeys() {
        let pricer = CategoryPricer {
            ice: EuroCents(12_000),
            regional: EuroCents(3_500),
        };
        let router = TimetableBundleRouter::from_loaded(
            bundle(480),
            Arc::new(TimeZoneFinder::new()),
            pricer,
        );
        let input = leg_input(PriceWeight::TIME_FIRST);
        let LegPlanningResult::Found(plan) = router.plan_leg(&input, || false) else {
            panic!("the leg has a journey");
        };
        let fast_minutes = plan.chosen_journey.cost_parts.unwrap().door_to_door_minutes;
        // The regional train takes 25 minutes longer than the ICE and is much cheaper, so the
        // search at the cheapest weight finds it unless a limit leaves it out.
        let category = |limit| {
            router
                .best_journey_at(
                    &input,
                    PriceWeight::CHEAPEST,
                    SearchBounds {
                        minutes: limit,
                        cost: None,
                    },
                    &|| false,
                )
                .map(|found| found.journey.first_ride.train_label.category)
        };
        assert_eq!(category(None).as_deref(), Some("RE"));
        assert_eq!(category(Some(fast_minutes + 60)).as_deref(), Some("RE"));
        assert_eq!(category(Some(fast_minutes + 10)).as_deref(), Some("ICE"));
    }

    /// A category pricer whose pass covers the fast train for a reservation and no other, so
    /// a pass day turns the expensive train into the cheap one.
    #[derive(Clone, Copy)]
    struct PassCategoryPricer {
        /// Price of the ICE on single tickets, in euro cents.
        ice: EuroCents,
        /// Price of the regional train, in euro cents.
        regional: EuroCents,
        /// Reservation the pass charges for the ICE, in euro cents.
        ice_reservation: EuroCents,
    }

    impl PassCategoryPricer {
        /// Returns the cents of a journey by the category of its first train.
        fn cents_of(&self, journey: &Journey) -> EuroCents {
            if journey.first_ride.train_label.category == "ICE" {
                self.ice
            } else {
                self.regional
            }
        }
    }

    impl JourneyPricer for PassCategoryPricer {
        fn journey_price(&self, journey: &Journey, _: &[LegPlanStation]) -> Option<EuroRange> {
            let cents = self.cents_of(journey);
            Some(EuroRange::new(cents, cents, cents).unwrap())
        }

        fn journey_price_with_pass(
            &self,
            journey: &Journey,
            _: &[LegPlanStation],
        ) -> Option<EuroRange> {
            let cents = if journey.first_ride.train_label.category == "ICE" {
                self.ice_reservation
            } else {
                self.regional
            };
            Some(EuroRange::new(cents, cents, cents).unwrap())
        }

        fn price_table(
            &self,
            classes: &[crate::pricing::RidePriceClass],
        ) -> Option<crate::pricing::RidePriceTable> {
            Some(crate::pricing::RidePriceTable {
                rows: classes
                    .iter()
                    .map(|class| {
                        let cents = if class.category == "ICE" {
                            self.ice
                        } else {
                            self.regional
                        };
                        flat_price_row(cents)
                    })
                    .collect(),
                reference_kilometres: 1.0,
            })
        }

        fn pass_price_table(
            &self,
            classes: &[crate::pricing::RidePriceClass],
        ) -> Option<crate::pricing::RidePassTable> {
            Some(crate::pricing::RidePassTable {
                rows: classes
                    .iter()
                    .map(|class| crate::pricing::RidePassRow {
                        takes: class.category == "ICE",
                        cents: if class.category == "ICE" {
                            self.ice_reservation.0
                        } else {
                            0
                        },
                    })
                    .collect(),
            })
        }
    }

    #[test]
    fn the_plan_offers_the_fast_train_as_a_pass_day() {
        let pricer = PassCategoryPricer {
            ice: EuroCents(12_000),
            regional: EuroCents(3_500),
            ice_reservation: EuroCents(1_000),
        };
        // On single tickets the cheap regional train wins the ranking, but the pass takes the
        // ICE for its reservation, so the plan holds a pass option with the fast train.
        let by_price = planned_leg(pricer, PriceWeight::FRUGAL, 480);
        assert_eq!(
            by_price.chosen_journey.first_ride.train_label.category,
            "RE"
        );
        let [option] = &by_price.pass_options[..] else {
            panic!("the leg has one pass option per number of days");
        };
        assert_eq!(option.journey.first_ride.train_label.category, "ICE");
        assert_eq!(option.days, PassDays::new(1).unwrap());
        assert_eq!(option.price_cents, EuroCents(1_000));
        assert_eq!(
            option.first_date,
            NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()
        );
        // The option costs the fast train's 25 minutes, not the fare.
        assert_eq!(option.cost_minutes, 25);
        // Its journey shows the reservation as its price, weighted at three minutes per euro.
        let parts = option.journey.cost_parts.as_ref().unwrap();
        assert_eq!(parts.price.unwrap().median(), EuroCents(1_000));
        assert_eq!(parts.total_cost, 25 + 30);
    }

    #[test]
    fn without_a_weight_no_leg_is_searched_on_pass_days() {
        let pricer = PassCategoryPricer {
            ice: EuroCents(12_000),
            regional: EuroCents(3_500),
            ice_reservation: EuroCents(1_000),
        };
        // Without a weight a pass saves nothing that counts, so the planner searches no pass
        // days and offers no pass option.
        let by_time = planned_leg(pricer, PriceWeight::TIME_FIRST, 480);
        assert!(by_time.pass_options.is_empty());
    }

    #[test]
    fn no_pass_option_without_a_pass_that_takes_a_train() {
        let pricer = CategoryPricer {
            ice: EuroCents(12_000),
            regional: EuroCents(3_500),
        };
        // The pricer offers no pass table, which means no pass covers a train of the leg.
        let plan = planned_leg(pricer, PriceWeight::FRUGAL, 480);
        assert!(plan.pass_options.is_empty());
    }
}
