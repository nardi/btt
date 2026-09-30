//! Round-based search that keeps the journey cost as a criterion next to departure, arrival and
//! number of trains.
//!
//! The plain search drops a journey when another leaves no earlier, arrives no later and uses no
//! more trains. When part of the cost doesn't follow the travel time, such as sleep on a night
//! train or the weight of the ways to and from stations, that can drop the cheapest journey. This
//! search keeps a bag of labels per station and round instead of one label, as McRAPTOR does, and
//! a label only drops another that it beats on arrival and on cost.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use btt_timetable::{
    BundleStationIndex, BundleTripIndex, LoadedTimetableBundle, RaptorRoute, RaptorRouteIndex,
};
use fixedbitset::FixedBitSet;
use smallvec::SmallVec;

use crate::index::SearchIndex;
use crate::lower_bound::UNREACHABLE;
use crate::search::{
    BoardedTrip, CandidateStation, MINUTES_PER_DAY, MinutesSinceBundleStart, RaptorJourney,
    RaptorLegQuery, RaptorRide, RideOrigin, collect_departure_times, earliest_trip, raptor_ride,
};

/// Marks a route that is not queued for scanning in the current round.
const NOT_QUEUED: u16 = u16::MAX;

/// Marks a station and round without a bag, or a label without a parent.
const NONE: u32 = u32::MAX;

/// Costs that a search adds up along a journey, in a unit the model chooses.
///
/// The search relies on three properties. `elapsed_cost` never falls when the arrival moves
/// later or the departure moves earlier. `ride_cost` never rises when the traveller boards the
/// same trip at an earlier stop. And `remaining_cost_lower_bound` never exceeds the cost still to
/// come.
pub trait JourneyCostModel {
    /// Returns the added cost of getting from the departure place to a station.
    fn access_cost(&self, candidate: &CandidateStation) -> i32;

    /// Returns the added cost of a ride on a trip from its departure to its arrival.
    /// `is_first_ride` is `false` when the traveller changes to it from another train.
    fn ride_cost(
        &self,
        raptor_route: RaptorRouteIndex,
        trip: BundleTripIndex,
        departure: MinutesSinceBundleStart,
        arrival: MinutesSinceBundleStart,
        is_first_ride: bool,
    ) -> i32;

    /// Returns the added cost of getting from an arrival candidate station to the arrival
    /// place. `final_footpath_minutes` holds the walk between stations that brought the
    /// traveller to the candidate after the last ride, if there was one.
    fn egress_cost(&self, candidate: &CandidateStation, final_footpath_minutes: Option<u16>)
    -> i32;

    /// Returns the cost of the time between leaving the departure place and reaching the
    /// arrival place.
    fn elapsed_cost(
        &self,
        departure: MinutesSinceBundleStart,
        arrival: MinutesSinceBundleStart,
    ) -> i32;

    /// Returns a lower bound on the elapsed cost plus every added cost still to come, for a
    /// traveller who left at `departure`, is at a station at `moment` and can't reach the
    /// arrival place before `earliest_arrival`.
    fn remaining_cost_lower_bound(
        &self,
        departure: MinutesSinceBundleStart,
        moment: MinutesSinceBundleStart,
        earliest_arrival: MinutesSinceBundleStart,
    ) -> i32;

    /// Returns the highest cost worth searching for once a journey that costs `best_cost` is
    /// known. The search drops labels whose lower bound lies above it, even when their journeys
    /// would not be beaten on every criterion. The default keeps all such journeys.
    fn cost_limit(&self, best_cost: i32) -> i32 {
        let _ = best_cost;
        i32::MAX
    }

    /// Returns whether the cost of a ride on a route can depend on the trip's times. When it
    /// can't, the search lets an earlier trip of the route stand for the later ones. The
    /// default assumes it can.
    fn ride_cost_depends_on_time(&self, raptor_route: RaptorRouteIndex) -> bool {
        let _ = raptor_route;
        true
    }
}

/// A journey with its cost under the model that found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostedJourney {
    /// The journey.
    pub journey: RaptorJourney,
    /// The elapsed cost plus every added cost, in the model's unit.
    pub cost: i32,
}

impl CostedJourney {
    /// Returns whether this journey is at least as good as another in departure, arrival,
    /// rides and cost, and better in at least one.
    fn dominates(&self, other: &Self) -> bool {
        let (own, theirs) = (&self.journey, &other.journey);
        let at_least_as_good = own.departure >= theirs.departure
            && own.arrival <= theirs.arrival
            && own.ride_count() <= theirs.ride_count()
            && self.cost <= other.cost;
        let better = own.departure > theirs.departure
            || own.arrival < theirs.arrival
            || own.ride_count() < theirs.ride_count()
            || self.cost < other.cost;
        at_least_as_good && better
    }
}

/// Journeys none of which is better than another in departure, arrival, number of trains and
/// cost.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CostParetoJourneySet {
    /// The journeys, sorted by arrival.
    pub journeys: Vec<CostedJourney>,
}

impl CostParetoJourneySet {
    /// Joins the results of searches over parts of one departure window, keeping only the
    /// journeys no journey of any part beats.
    pub fn merge(sets: impl IntoIterator<Item = Self>) -> Self {
        let mut merged = Self {
            journeys: sets.into_iter().flat_map(|set| set.journeys).collect(),
        };
        merged.keep_pareto_optimal();
        merged
    }

    /// Removes dominated and repeated journeys and sorts the rest by arrival.
    fn keep_pareto_optimal(&mut self) {
        let journeys = std::mem::take(&mut self.journeys);
        for (index, journey) in journeys.iter().enumerate() {
            let dominated = journeys.iter().enumerate().any(|(other_index, other)| {
                other_index != index
                    && (other.dominates(journey) || (other == journey && other_index < index))
            });
            if !dominated {
                self.journeys.push(journey.clone());
            }
        }
        self.journeys.sort_by_key(|costed| {
            (
                costed.journey.arrival,
                costed.journey.ride_count(),
                Reverse(costed.journey.departure),
                costed.cost,
            )
        });
    }
}

/// How a label was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CostLabelOrigin {
    /// The traveller walked there from the departure place.
    DepartureCandidate {
        /// Position in `RaptorLegQuery::departure_candidates`.
        candidate: u16,
    },
    /// The traveller rode a trip there.
    TrainRide(RideOrigin),
    /// The traveller rode a trip to another station and walked from there.
    TrainRideAndFootpath {
        ride: RideOrigin,
        alighting_station: BundleStationIndex,
        footpath_minutes: u16,
    },
}

/// One way of being at a station, with what it cost so far.
#[derive(Debug, Clone, Copy)]
struct CostLabel {
    /// Arrival at the station.
    arrival: u32,
    /// Earliest departure of a train the traveller can board here.
    ready: u32,
    /// Added cost so far.
    cost: i32,
    /// Added cost including the way to the arrival place, at an arrival candidate station.
    /// Elsewhere it equals `cost`. Two labels at a station compare on both, because the way
    /// to the place costs more after a walk between stations than after a ride.
    cost_with_egress: i32,
    /// The station.
    station: BundleStationIndex,
    /// The label the traveller boarded the last ride from, or `NONE` at the departure place.
    parent: u32,
    /// Departure time run that created the label.
    run: u32,
    /// Round that created the label.
    round: u8,
    /// How the label was reached.
    origin: CostLabelOrigin,
}

impl CostLabel {
    /// Returns whether this label is at least as good as another at the same station in every
    /// criterion.
    #[inline]
    fn covers(&self, other: &Self) -> bool {
        self.arrival <= other.arrival
            && self.ready <= other.ready
            && self.cost <= other.cost
            && self.cost_with_egress <= other.cost_with_egress
    }
}

/// A trip that a route scan rides, boarded from one label.
#[derive(Debug, Clone, Copy)]
struct RouteLabel {
    trip_offset: u32,
    service_day: u16,
    boarding_position: u16,
    /// Departure from the boarding stop, in minutes since the bundle start.
    departure: u32,
    /// The label the traveller boarded from.
    parent: u32,
    /// Added cost of the parent label.
    cost_before_ride: i32,
    /// Whether the ride is the first of the journey.
    is_first_ride: bool,
}

impl RouteLabel {
    /// Returns whether riding this is at least as good as riding another at every later stop.
    ///
    /// On the same trip, boarding earlier with no more cost is. On another trip, only when
    /// the ride cost doesn't depend on the trip's times, and the trip runs earlier on the same
    /// service day, so it reaches every later stop no later.
    #[inline]
    fn covers(&self, other: &Self, ride_cost_depends_on_time: bool) -> bool {
        let no_worse_start = self.cost_before_ride <= other.cost_before_ride
            && (self.is_first_ride || !other.is_first_ride);
        let same_trip =
            self.trip_offset == other.trip_offset && self.service_day == other.service_day;
        if same_trip {
            return no_worse_start && self.departure <= other.departure;
        }
        !ride_cost_depends_on_time
            && no_worse_start
            && self.service_day == other.service_day
            && self.trip_offset < other.trip_offset
    }
}

/// Arrivals at the arrival place, as the arrival and a cost that the journey's cost doesn't
/// exceed, keeping only those no other arrival beats on both. Sorted by arrival, the costs
/// fall, so the cheapest arrival by a moment is the last one before it.
#[derive(Debug, Default)]
struct ArrivalStaircase {
    entries: Vec<(u32, i32)>,
}

impl ArrivalStaircase {
    /// Returns whether an arrival no later than `arrival` costs no more than `cost`.
    #[inline]
    fn beats(&self, arrival: u32, cost: i32) -> bool {
        let after = self.entries.partition_point(|&(other, _)| other <= arrival);
        after > 0 && self.entries[after - 1].1 <= cost
    }

    /// Adds an arrival unless another beats it, and removes the arrivals it beats. Returns
    /// whether it was added.
    fn insert(&mut self, arrival: u32, cost: i32) -> bool {
        if self.beats(arrival, cost) {
            return false;
        }
        let start = self.entries.partition_point(|&(other, _)| other < arrival);
        let end = start
            + self.entries[start..]
                .iter()
                .take_while(|&&(_, other_cost)| other_cost >= cost)
                .count();
        self.entries.splice(start..end, [(arrival, cost)]);
        true
    }
}

/// A train arrival that footpaths start from.
#[derive(Debug, Clone, Copy)]
struct RideArrival {
    station: BundleStationIndex,
    arrival: u32,
    cost: i32,
    parent: u32,
    ride: RideOrigin,
}

/// Working memory of a cost-aware search. Reusing it across queries avoids most allocation
/// after the first query on a thread.
#[derive(Debug, Default)]
pub struct CostSearchBuffers {
    /// Every label of the query. Bags refer to labels by their position here.
    labels: Vec<CostLabel>,
    /// Bag per round and station, flattened as `round * station_count + station`, or `NONE`.
    bag_by_slot: Vec<u32>,
    /// Slots with a bag, so the next reset only clears those.
    used_slots: Vec<u32>,
    /// Label positions per bag. Only the first `bag_count` are in use.
    bags: Vec<SmallVec<[u32; 4]>>,
    bag_count: usize,
    /// Arrivals at the arrival place found per round. Labels that can't beat them are pruned.
    targets_by_round: Vec<ArrivalStaircase>,
    /// Highest cost worth searching for, from the model and the cheapest arrival found so far.
    cost_limit: i32,
    /// Stations with labels the current round created, and a mark per station against
    /// repeats.
    stations_created: Vec<BundleStationIndex>,
    created_marks: FixedBitSet,
    /// Stations whose bag the current round changed, by creating or copying labels.
    stations_changed: Vec<BundleStationIndex>,
    changed_marks: FixedBitSet,
    /// The two lists above for the previous round.
    stations_created_before: Vec<BundleStationIndex>,
    stations_changed_before: Vec<BundleStationIndex>,
    /// Earliest stop position to scan per route, or `NOT_QUEUED`.
    scan_start_by_route: Vec<u16>,
    /// Routes to scan in the current round.
    queued_routes: Vec<RaptorRouteIndex>,
    /// Trips the current route scan rides.
    route_labels: Vec<RouteLabel>,
    /// Train arrivals of the current round at stations with footpaths.
    ride_arrivals: Vec<RideArrival>,
    /// Position in `RaptorLegQuery::arrival_candidates` per station, or `u16::MAX`.
    arrival_candidate_by_station: Vec<u16>,
    /// Stations of the last query with an arrival candidate, so the next reset only clears
    /// those.
    arrival_stations: Vec<BundleStationIndex>,
    /// Fewest minutes from each station to the arrival place, or `UNREACHABLE`.
    lower_bound_by_station: Vec<u32>,
    /// Priority queue of the lower bound search.
    lower_bound_queue: BinaryHeap<Reverse<(u32, u32)>>,
}

impl CostSearchBuffers {
    /// Creates empty buffers. The first search sizes them to its bundle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears the buffers for a search over a bundle.
    fn reset(&mut self, station_count: usize, route_count: usize, rounds: usize) {
        self.labels.clear();
        for slot in self.used_slots.drain(..) {
            if let Some(bag) = self.bag_by_slot.get_mut(slot as usize) {
                *bag = NONE;
            }
        }
        self.bag_by_slot.resize(station_count * rounds, NONE);
        self.bag_count = 0;
        self.targets_by_round
            .resize_with(rounds, ArrivalStaircase::default);
        self.cost_limit = i32::MAX;
        for targets in &mut self.targets_by_round {
            targets.entries.clear();
        }
        self.stations_created.clear();
        self.stations_changed.clear();
        self.stations_created_before.clear();
        self.stations_changed_before.clear();
        self.created_marks.clear();
        self.created_marks.grow(station_count);
        self.changed_marks.clear();
        self.changed_marks.grow(station_count);
        self.scan_start_by_route.clear();
        self.scan_start_by_route.resize(route_count, NOT_QUEUED);
        self.queued_routes.clear();
        self.route_labels.clear();
        self.ride_arrivals.clear();
        self.arrival_candidate_by_station
            .resize(station_count, u16::MAX);
        for station in self.arrival_stations.drain(..) {
            if let Some(slot) = self
                .arrival_candidate_by_station
                .get_mut(station.as_usize())
            {
                *slot = u16::MAX;
            }
        }
    }
}

/// One cost-aware search, with the query, the bundle, the model and the working memory.
struct CostSearch<'a, M: JourneyCostModel> {
    bundle: &'a LoadedTimetableBundle,
    index: &'a SearchIndex,
    query: &'a RaptorLegQuery<'a>,
    model: &'a M,
    buffers: &'a mut CostSearchBuffers,
    station_count: usize,
    /// Departure time of the current run.
    departure: u32,
    /// Number of the current run.
    run: u32,
}

/// Searches journeys for one leg and returns the Pareto set over departure, arrival, number of
/// trains and cost under `model`.
///
/// `index` must be built from the same bundle. `should_stop` is checked between departure
/// times, and the search returns what it has when it returns `true`.
pub fn search_leg_journeys_by_cost<M: JourneyCostModel>(
    bundle: &LoadedTimetableBundle,
    index: &SearchIndex,
    query: &RaptorLegQuery<'_>,
    model: &M,
    buffers: &mut CostSearchBuffers,
    should_stop: impl Fn() -> bool,
) -> CostParetoJourneySet {
    let rounds = usize::from(query.maximum_train_rides.get()) + 1;
    let station_count = bundle.station_count();
    buffers.reset(station_count, bundle.route_count(), rounds);
    index.lower_bounds.compute_bounds(
        query.arrival_candidates,
        &mut buffers.lower_bound_by_station,
        &mut buffers.lower_bound_queue,
    );
    for (candidate_index, candidate) in (0_u16..).zip(query.arrival_candidates) {
        let Some(slot) = buffers
            .arrival_candidate_by_station
            .get_mut(candidate.station.as_usize())
        else {
            continue;
        };
        // A station listed twice keeps the candidate with the shorter way to the place.
        let is_closer = query
            .arrival_candidates
            .get(usize::from(*slot))
            .is_none_or(|current| candidate.time_to_place_minutes < current.time_to_place_minutes);
        if is_closer {
            if *slot == u16::MAX {
                buffers.arrival_stations.push(candidate.station);
            }
            *slot = candidate_index;
        }
    }
    let departure_times = collect_departure_times(bundle, query);
    let mut search = CostSearch {
        bundle,
        index,
        query,
        model,
        buffers,
        station_count,
        departure: 0,
        run: 0,
    };
    let mut found = CostParetoJourneySet::default();
    for departures in departure_times.chunk_by(|left, right| left.0 == right.0) {
        if should_stop() {
            break;
        }
        search.departure = departures[0].0;
        let departing_candidates: SmallVec<[u16; 4]> =
            departures.iter().map(|&(_, candidate)| candidate).collect();
        search.run_rounds(&departing_candidates);
        search.collect_journeys(&mut found.journeys);
        search.run += 1;
    }
    found.keep_pareto_optimal();
    found
}

impl<M: JourneyCostModel> CostSearch<'_, M> {
    /// Returns the arrival candidate at a station, if it is one.
    #[inline]
    fn arrival_candidate(&self, station: BundleStationIndex) -> Option<&CandidateStation> {
        let position = self.buffers.arrival_candidate_by_station[station.as_usize()];
        self.query.arrival_candidates.get(usize::from(position))
    }

    /// Returns the label positions in the bag of a station in a round.
    #[inline]
    fn bag(&self, round: usize, station: BundleStationIndex) -> &[u32] {
        let bag = self.buffers.bag_by_slot[round * self.station_count + station.as_usize()];
        if bag == NONE {
            &[]
        } else {
            &self.buffers.bags[bag as usize]
        }
    }

    /// Returns the added cost including the way to the arrival place, for a label at a station
    /// with a cost so far and the walk between stations that brought it there.
    fn cost_with_egress(
        &self,
        station: BundleStationIndex,
        cost: i32,
        footpath_minutes: Option<u16>,
    ) -> i32 {
        self.arrival_candidate(station).map_or(cost, |candidate| {
            cost.saturating_add(self.model.egress_cost(candidate, footpath_minutes))
        })
    }

    /// Returns whether a label at a station in a round can still lead to a journey that no
    /// arrival found so far beats, by the station's lower bound and the model's lower bound on
    /// the cost to come.
    ///
    /// Arrivals found in earlier runs left later, so beating a label's bound on arrival and
    /// cost means beating it on every criterion.
    fn may_beat_targets(
        &self,
        round: usize,
        station: BundleStationIndex,
        label: &CostLabel,
    ) -> bool {
        let lower_bound = self.buffers.lower_bound_by_station[station.as_usize()];
        if lower_bound == UNREACHABLE {
            return false;
        }
        let earliest_arrival = label.arrival.saturating_add(lower_bound);
        let cost_bound = self
            .model
            .remaining_cost_lower_bound(
                MinutesSinceBundleStart(self.departure),
                MinutesSinceBundleStart(label.arrival),
                MinutesSinceBundleStart(earliest_arrival),
            )
            .saturating_add(label.cost);
        if cost_bound > self.buffers.cost_limit {
            return false;
        }
        !self.buffers.targets_by_round[1..=round]
            .iter()
            .any(|targets| targets.beats(earliest_arrival, cost_bound))
    }

    /// Returns the bag of a slot for changing, creating it when the slot has none.
    fn bag_for_insert(&mut self, slot: usize) -> usize {
        let existing = self.buffers.bag_by_slot[slot];
        if existing != NONE {
            return existing as usize;
        }
        let bag = self.buffers.bag_count;
        if bag == self.buffers.bags.len() {
            self.buffers.bags.push(SmallVec::new());
        } else {
            self.buffers.bags[bag].clear();
        }
        self.buffers.bag_count += 1;
        self.buffers.bag_by_slot[slot] = u32::try_from(bag).unwrap_or(NONE);
        self.buffers
            .used_slots
            .push(u32::try_from(slot).unwrap_or(NONE));
        bag
    }

    /// Adds a label to the bag of its station in a round, unless a label there covers it, and
    /// removes the labels it covers. `existing` is the label's position when it is already
    /// stored. Returns the label's position when the bag took it.
    fn insert(&mut self, round: usize, label: CostLabel, existing: Option<u32>) -> Option<u32> {
        let slot = round * self.station_count + label.station.as_usize();
        let labels = &self.buffers.labels;
        let existing_bag = self.buffers.bag_by_slot[slot];
        if existing_bag != NONE {
            let entries = &mut self.buffers.bags[existing_bag as usize];
            if entries
                .iter()
                .any(|&other| labels[other as usize].covers(&label))
            {
                return None;
            }
            entries.retain(|other| !label.covers(&labels[*other as usize]));
        }
        let bag = self.bag_for_insert(slot);
        let position = existing.unwrap_or_else(|| {
            let position = u32::try_from(self.buffers.labels.len()).unwrap_or(NONE);
            self.buffers.labels.push(label);
            position
        });
        self.buffers.bags[bag].push(position);
        let index = label.station.as_usize();
        if !self.buffers.changed_marks.put(index) {
            self.buffers.stations_changed.push(label.station);
        }
        if existing.is_none() && !self.buffers.created_marks.put(index) {
            self.buffers.stations_created.push(label.station);
        }
        if round > 0
            && let Some(candidate) = self.arrival_candidate(label.station)
        {
            let arrival = label.arrival + u32::from(candidate.time_to_place_minutes);
            let cost = self
                .model
                .elapsed_cost(
                    MinutesSinceBundleStart(self.departure),
                    MinutesSinceBundleStart(arrival),
                )
                .saturating_add(label.cost_with_egress);
            if self.buffers.targets_by_round[round].insert(arrival, cost) {
                self.buffers.cost_limit = self.buffers.cost_limit.min(self.model.cost_limit(cost));
            }
        }
        Some(position)
    }

    /// Moves the station lists of the round that just ended to the lists of the previous round
    /// and clears the marks.
    fn end_round(&mut self) {
        let buffers = &mut *self.buffers;
        for station in &buffers.stations_created {
            buffers.created_marks.set(station.as_usize(), false);
        }
        for station in &buffers.stations_changed {
            buffers.changed_marks.set(station.as_usize(), false);
        }
        std::mem::swap(
            &mut buffers.stations_created,
            &mut buffers.stations_created_before,
        );
        std::mem::swap(
            &mut buffers.stations_changed,
            &mut buffers.stations_changed_before,
        );
        buffers.stations_created.clear();
        buffers.stations_changed.clear();
    }

    /// Runs all rounds for the current departure time. Labels from runs for later departure
    /// times stay, and a new label only enters a bag when no label there covers it.
    fn run_rounds(&mut self, departing_candidates: &[u16]) {
        self.buffers.stations_created_before.clear();
        self.buffers.stations_changed_before.clear();
        for &candidate_index in departing_candidates {
            let Some(candidate) = self
                .query
                .departure_candidates
                .get(usize::from(candidate_index))
            else {
                continue;
            };
            let arrival = self.departure + u32::from(candidate.time_to_place_minutes);
            let cost = self.model.access_cost(candidate);
            let label = CostLabel {
                arrival,
                ready: arrival,
                cost,
                cost_with_egress: cost,
                station: candidate.station,
                parent: NONE,
                run: self.run,
                round: 0,
                origin: CostLabelOrigin::DepartureCandidate {
                    candidate: candidate_index,
                },
            };
            self.insert(0, label, None);
        }
        self.end_round();
        let rounds = usize::from(self.query.maximum_train_rides.get()) + 1;
        for round in 1..rounds {
            self.carry_labels_forward(round);
            if !self.buffers.stations_created_before.is_empty() {
                self.queue_routes();
                let queued = std::mem::take(&mut self.buffers.queued_routes);
                for &raptor_route in &queued {
                    let start = self.buffers.scan_start_by_route[raptor_route.as_usize()];
                    self.buffers.scan_start_by_route[raptor_route.as_usize()] = NOT_QUEUED;
                    self.scan_route(round, raptor_route, start);
                }
                self.buffers.queued_routes = queued;
                self.buffers.queued_routes.clear();
                self.relax_footpaths(round);
            }
            self.end_round();
            if self.buffers.stations_changed_before.is_empty() {
                break;
            }
        }
    }

    /// Copies the labels of this run that changed in the previous round into this round,
    /// because a journey with fewer rides also counts as one with at most this many.
    fn carry_labels_forward(&mut self, round: usize) {
        let changed = std::mem::take(&mut self.buffers.stations_changed_before);
        for &station in &changed {
            let positions: SmallVec<[u32; 8]> = self
                .bag(round - 1, station)
                .iter()
                .copied()
                .filter(|&position| self.buffers.labels[position as usize].run == self.run)
                .collect();
            for position in positions {
                let label = self.buffers.labels[position as usize];
                self.insert(round, label, Some(position));
            }
        }
        self.buffers.stations_changed_before = changed;
    }

    /// Queues every route that stops at a station with labels created in the previous round,
    /// from the earliest such stop.
    fn queue_routes(&mut self) {
        for &station in &self.buffers.stations_created_before {
            for route_stop in self.bundle.route_stops_at(station) {
                let slot =
                    &mut self.buffers.scan_start_by_route[route_stop.raptor_route.as_usize()];
                if *slot == NOT_QUEUED {
                    self.buffers.queued_routes.push(route_stop.raptor_route);
                    *slot = route_stop.stop_position;
                } else {
                    *slot = (*slot).min(route_stop.stop_position);
                }
            }
        }
    }

    /// Scans one route from a stop position. Each label the previous round created at a stop
    /// boards the earliest trip it can reach, and every trip ridden adds a label at each later
    /// stop.
    fn scan_route(&mut self, round: usize, raptor_route: RaptorRouteIndex, start: u16) {
        let route = self.bundle.route(raptor_route);
        let mut route_labels = std::mem::take(&mut self.buffers.route_labels);
        let ride_cost_depends_on_time = self.model.ride_cost_depends_on_time(raptor_route);
        route_labels.clear();
        let minimum_transfer = u32::from(self.query.minimum_transfer_minutes);
        for position in start..route.stop_count {
            let pattern_stop = self.bundle.pattern_stop(&route, position);
            let station = pattern_stop.station();
            if pattern_stop.alighting_allowed() {
                for route_label in &route_labels {
                    let times = self
                        .bundle
                        .stop_event(&route, route_label.trip_offset, position);
                    let arrival = u32::from(route_label.service_day) * MINUTES_PER_DAY
                        + u32::from(times.arrival.0);
                    let ride_cost = self.model.ride_cost(
                        raptor_route,
                        BundleTripIndex(route.first_trip_index + route_label.trip_offset),
                        MinutesSinceBundleStart(route_label.departure),
                        MinutesSinceBundleStart(arrival),
                        route_label.is_first_ride,
                    );
                    let cost = route_label.cost_before_ride.saturating_add(ride_cost);
                    let ride = RideOrigin {
                        raptor_route,
                        trip_offset: route_label.trip_offset,
                        service_day: route_label.service_day,
                        boarding_position: route_label.boarding_position,
                    };
                    let label = CostLabel {
                        arrival,
                        ready: arrival + minimum_transfer,
                        cost,
                        cost_with_egress: self.cost_with_egress(station, cost, None),
                        station,
                        parent: route_label.parent,
                        run: self.run,
                        round: u8::try_from(round).unwrap_or(u8::MAX),
                        origin: CostLabelOrigin::TrainRide(ride),
                    };
                    if !self.may_beat_targets(round, station, &label) {
                        continue;
                    }
                    // Footpaths start from every train arrival, because a label that arrived
                    // on foot can cover this one here but can't walk on.
                    if self.bundle.footpaths_from(station).len() > 0 {
                        self.buffers.ride_arrivals.push(RideArrival {
                            station,
                            arrival,
                            cost,
                            parent: route_label.parent,
                            ride,
                        });
                    }
                    self.insert(round, label, None);
                }
            }
            if !pattern_stop.boarding_allowed() {
                continue;
            }
            self.board_route(
                round,
                raptor_route,
                &route,
                position,
                &mut route_labels,
                ride_cost_depends_on_time,
            );
        }
        self.buffers.route_labels = route_labels;
    }

    /// Lets each label the previous round created at a stop of a route board the earliest
    /// trip it can reach there, and adds the rides to the route's labels unless one there
    /// covers them.
    fn board_route(
        &self,
        round: usize,
        raptor_route: RaptorRouteIndex,
        route: &RaptorRoute,
        position: u16,
        route_labels: &mut Vec<RouteLabel>,
        ride_cost_depends_on_time: bool,
    ) {
        let station = self.bundle.pattern_stop(route, position).station();
        let previous_round = u8::try_from(round - 1).unwrap_or(u8::MAX);
        let mut positions: SmallVec<[(u32, u32); 8]> = self
            .bag(round - 1, station)
            .iter()
            .map(|&label_position| (self.buffers.labels[label_position as usize], label_position))
            .filter(|(label, _)| label.run == self.run && label.round == previous_round)
            .map(|(label, label_position)| (label.ready, label_position))
            .collect();
        // Labels in order of readiness share one trip lookup until they are ready after its
        // departure.
        positions.sort_unstable();
        let mut boarded: Option<BoardedTrip> = None;
        for (ready, parent) in positions {
            let trip = match boarded {
                Some(trip) if ready <= trip.departure => trip,
                _ => {
                    let Some(trip) = earliest_trip(
                        self.bundle,
                        self.index,
                        raptor_route,
                        route,
                        position,
                        ready,
                        |_, _| true,
                    ) else {
                        // Labels that are ready later find no trip either.
                        break;
                    };
                    boarded = Some(trip);
                    trip
                }
            };
            let label = self.buffers.labels[parent as usize];
            let route_label = RouteLabel {
                trip_offset: trip.trip_offset,
                service_day: trip.service_day,
                boarding_position: trip.boarding_position,
                departure: trip.departure,
                parent,
                cost_before_ride: label.cost,
                is_first_ride: matches!(label.origin, CostLabelOrigin::DepartureCandidate { .. }),
            };
            if route_labels
                .iter()
                .any(|other| other.covers(&route_label, ride_cost_depends_on_time))
            {
                continue;
            }
            route_labels.retain(|other| !route_label.covers(other, ride_cost_depends_on_time));
            route_labels.push(route_label);
        }
    }

    /// Walks footpaths from the train arrivals of this round. Footpaths are one hop, so
    /// arrivals on foot are not walked on. A walk between stations adds no cost of its own
    /// until it ends at an arrival candidate.
    fn relax_footpaths(&mut self, round: usize) {
        let mut arrivals = std::mem::take(&mut self.buffers.ride_arrivals);
        // Walking on from an arrival that another at the same station beats on time and cost
        // leads nowhere new, so in order of arrival only arrivals cheaper than all before walk.
        arrivals.sort_unstable_by_key(|ride_arrival| {
            (
                ride_arrival.station,
                ride_arrival.arrival,
                ride_arrival.cost,
            )
        });
        let mut cheapest_before: Option<(BundleStationIndex, i32)> = None;
        for ride_arrival in &arrivals {
            match cheapest_before {
                Some((station, cost))
                    if station == ride_arrival.station && cost <= ride_arrival.cost =>
                {
                    continue;
                }
                _ => cheapest_before = Some((ride_arrival.station, ride_arrival.cost)),
            }
            for footpath in self.bundle.footpaths_from(ride_arrival.station) {
                let arrival = ride_arrival.arrival + u32::from(footpath.duration_minutes);
                let label = CostLabel {
                    arrival,
                    ready: arrival,
                    cost: ride_arrival.cost,
                    cost_with_egress: self.cost_with_egress(
                        footpath.to_station,
                        ride_arrival.cost,
                        Some(footpath.duration_minutes),
                    ),
                    station: footpath.to_station,
                    parent: ride_arrival.parent,
                    run: self.run,
                    round: u8::try_from(round).unwrap_or(u8::MAX),
                    origin: CostLabelOrigin::TrainRideAndFootpath {
                        ride: ride_arrival.ride,
                        alighting_station: ride_arrival.station,
                        footpath_minutes: footpath.duration_minutes,
                    },
                };
                if self.may_beat_targets(round, footpath.to_station, &label) {
                    self.insert(round, label, None);
                }
            }
        }
        self.buffers.ride_arrivals = arrivals;
        self.buffers.ride_arrivals.clear();
    }

    /// Adds a journey for every label this run created at an arrival candidate, in the round
    /// that created it.
    fn collect_journeys(&self, journeys: &mut Vec<CostedJourney>) {
        let rounds = usize::from(self.query.maximum_train_rides.get()) + 1;
        for round in 1..rounds {
            for station in &self.buffers.arrival_stations {
                for &position in self.bag(round, *station) {
                    let label = &self.buffers.labels[position as usize];
                    if label.run != self.run || usize::from(label.round) != round {
                        continue;
                    }
                    let Some(journey) = self.reconstruct(position) else {
                        continue;
                    };
                    // A journey whose first train leaves long after the window would have the
                    // traveller leave in time and wait at the station, so it counts as outside
                    // the window.
                    if journey.departure >= self.query.departure_window.end {
                        continue;
                    }
                    let cost = self
                        .model
                        .elapsed_cost(journey.departure, journey.arrival)
                        .saturating_add(label.cost_with_egress);
                    journeys.push(CostedJourney { journey, cost });
                }
            }
        }
    }

    /// Follows the parents of a label at an arrival candidate back to a departure candidate.
    fn reconstruct(&self, position: u32) -> Option<RaptorJourney> {
        let last = &self.buffers.labels[position as usize];
        let arrival_candidate = *self.arrival_candidate(last.station)?;
        let arrival = last.arrival + u32::from(arrival_candidate.time_to_place_minutes);
        let mut rides: SmallVec<[RaptorRide; 4]> = SmallVec::new();
        let mut current = position;
        // Every step goes back one ride or ends at the departure place.
        for _ in 0..=usize::from(self.query.maximum_train_rides.get()) {
            let label = self.buffers.labels.get(current as usize)?;
            let (ride, alighting_station) = match label.origin {
                CostLabelOrigin::DepartureCandidate { candidate } => {
                    rides.reverse();
                    let departure_candidate = *self
                        .query
                        .departure_candidates
                        .get(usize::from(candidate))?;
                    let first_ride = rides.first()?;
                    let departure = first_ride
                        .departure
                        .0
                        .checked_sub(u32::from(departure_candidate.time_to_place_minutes))?;
                    return Some(RaptorJourney {
                        departure_candidate,
                        arrival_candidate,
                        rides,
                        departure: MinutesSinceBundleStart(departure),
                        arrival: MinutesSinceBundleStart(arrival),
                    });
                }
                CostLabelOrigin::TrainRide(ride) => (ride, label.station),
                CostLabelOrigin::TrainRideAndFootpath {
                    ride,
                    alighting_station,
                    ..
                } => (ride, alighting_station),
            };
            rides.push(raptor_ride(self.bundle, ride, alighting_station)?);
            current = label.parent;
        }
        None
    }
}
