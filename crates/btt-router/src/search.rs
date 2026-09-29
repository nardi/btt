//! Round-based search with multi-source starts and range queries.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::num::NonZeroU8;
use std::ops::Range;

use btt_timetable::{
    BundleStationIndex, BundleTripIndex, LoadedTimetableBundle, RaptorRoute, RaptorRouteIndex,
};
use fixedbitset::FixedBitSet;
use smallvec::SmallVec;

use crate::index::SearchIndex;

/// Minutes in one day.
const MINUTES_PER_DAY: u32 = 1440;

/// Marks a route that is not queued for scanning in the current round.
const NOT_QUEUED: u16 = u16::MAX;

/// Minutes since 00:00 UTC on the first day of the bundle's validity window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MinutesSinceBundleStart(pub u32);

/// A station near a trip place, with the time to get between the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateStation {
    /// The station.
    pub station: BundleStationIndex,
    /// Minutes between the place and the station, in either direction.
    pub time_to_place_minutes: u16,
}

/// What to search for one leg.
#[derive(Debug, Clone)]
pub struct RaptorLegQuery<'a> {
    /// Stations the traveller may start from.
    pub departure_candidates: &'a [CandidateStation],
    /// Stations the traveller may end at.
    pub arrival_candidates: &'a [CandidateStation],
    /// Times at which the traveller may leave the departure place.
    pub departure_window: Range<MinutesSinceBundleStart>,
    /// Shortest time between arriving on one train and departing on the next at a station.
    pub minimum_transfer_minutes: u16,
    /// Largest number of trains in a journey.
    pub maximum_train_rides: NonZeroU8,
}

/// Decides which trips the router may board, and where the traveller may leave them.
pub trait RideFilter {
    /// Returns whether the traveller may board a trip that departs at a moment.
    fn may_board(
        &self,
        raptor_route: RaptorRouteIndex,
        trip: BundleTripIndex,
        departure: MinutesSinceBundleStart,
    ) -> bool;

    /// Returns whether the traveller may leave a trip at a stop where it arrives at a moment.
    /// `is_last_stop` says whether the trip ends there. Every stop is allowed by default.
    #[inline]
    fn may_alight(
        &self,
        _raptor_route: RaptorRouteIndex,
        _trip: BundleTripIndex,
        _arrival: MinutesSinceBundleStart,
        _is_last_stop: bool,
    ) -> bool {
        true
    }
}

/// Allows every trip and every stop.
#[derive(Debug, Clone, Copy, Default)]
pub struct AllowEveryRide;

impl RideFilter for AllowEveryRide {
    #[inline]
    fn may_board(
        &self,
        _: RaptorRouteIndex,
        _: BundleTripIndex,
        _: MinutesSinceBundleStart,
    ) -> bool {
        true
    }
}

/// One train ridden from boarding to alighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RaptorRide {
    /// The route of the trip.
    pub raptor_route: RaptorRouteIndex,
    /// The trip.
    pub trip: BundleTripIndex,
    /// Day of the validity window on which the trip left its first stop.
    pub service_day: u16,
    /// Where the traveller boards.
    pub boarding_station: BundleStationIndex,
    /// Position of the boarding station in the route's stop sequence.
    pub boarding_position: u16,
    /// Where the traveller alights.
    pub alighting_station: BundleStationIndex,
    /// Position of the alighting station in the route's stop sequence.
    pub alighting_position: u16,
    /// Departure from the boarding station.
    pub departure: MinutesSinceBundleStart,
    /// Arrival at the alighting station.
    pub arrival: MinutesSinceBundleStart,
}

/// A journey from the departure place to the arrival place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaptorJourney {
    /// Station the journey starts from.
    pub departure_candidate: CandidateStation,
    /// Station the journey ends at.
    pub arrival_candidate: CandidateStation,
    /// Trains in travel order. Never empty.
    pub rides: SmallVec<[RaptorRide; 4]>,
    /// Latest time to leave the departure place.
    pub departure: MinutesSinceBundleStart,
    /// Arrival at the arrival place.
    pub arrival: MinutesSinceBundleStart,
}

impl RaptorJourney {
    /// Returns the number of trains.
    pub fn ride_count(&self) -> usize {
        self.rides.len()
    }

    /// Returns whether this journey is at least as good as another in departure, arrival and
    /// rides, and better in at least one.
    fn dominates(&self, other: &Self) -> bool {
        let at_least_as_good = self.departure >= other.departure
            && self.arrival <= other.arrival
            && self.ride_count() <= other.ride_count();
        let better = self.departure > other.departure
            || self.arrival < other.arrival
            || self.ride_count() < other.ride_count();
        at_least_as_good && better
    }
}

/// Journeys none of which is better than another in departure, arrival and number of trains.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParetoJourneySet {
    /// The journeys, sorted by arrival.
    pub journeys: SmallVec<[RaptorJourney; 8]>,
}

impl ParetoJourneySet {
    /// Joins the results of searches over parts of one departure window, keeping only the
    /// journeys no journey of any part beats.
    pub fn merge(sets: impl IntoIterator<Item = Self>) -> Self {
        let mut merged = Self {
            journeys: sets.into_iter().flat_map(|set| set.journeys).collect(),
        };
        keep_pareto_optimal(&mut merged);
        merged
    }
}

/// A ride on a trip from a boarding stop, as a label records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RideOrigin {
    raptor_route: RaptorRouteIndex,
    trip_offset: u32,
    service_day: u16,
    boarding_position: u16,
}

/// How the arrival label at a station in a round was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LabelOrigin {
    /// The traveller walked there from the departure place.
    DepartureCandidate {
        /// Position in `RaptorLegQuery::departure_candidates`.
        candidate: u16,
    },
    /// The traveller rode a trip there.
    TrainRide(RideOrigin),
    /// The traveller rode a trip to another station and walked from there.
    ///
    /// The ride is part of the label, because the label of the station where the ride ended
    /// may come from an earlier arrival on foot, which cannot continue on foot.
    TrainRideAndFootpath {
        ride: RideOrigin,
        alighting_station: BundleStationIndex,
    },
}

/// The earliest known arrival at a station within a number of rides, and how it was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Label {
    arrival: u32,
    origin: LabelOrigin,
}

/// Working memory of a search. Reusing it across queries avoids allocation after the first
/// query on a thread.
#[derive(Debug, Default)]
pub struct RaptorSearchBuffers {
    /// Flattened as `round * station_count + station`.
    labels_by_round_and_station: Vec<Option<Label>>,
    /// Best arrival at the arrival place per round, for target pruning. `u32::MAX` when none.
    target_bound_by_round: Vec<u32>,
    /// Stations whose label improved in the current round.
    stations_improved: FixedBitSet,
    /// Earliest train arrival per station in the current round, whether or not it improved the
    /// station's label. Footpaths start from these.
    ride_arrival_by_station: Vec<Option<(u32, RideOrigin)>>,
    /// Stations with an entry in `ride_arrival_by_station`, so the next round clears only those.
    stations_reached_by_rides: Vec<BundleStationIndex>,
    /// Stations whose label changed in the previous round, which the current round copies
    /// forward.
    stations_changed_in_previous_round: Vec<BundleStationIndex>,
    /// Spare list that `carry_labels_forward` swaps with the one above.
    stations_carried: Vec<BundleStationIndex>,
    /// Earliest stop position to scan per route, or `NOT_QUEUED`.
    scan_start_by_route: Vec<u16>,
    /// Routes to scan in the current round.
    queued_routes: Vec<RaptorRouteIndex>,
    /// Minutes from each arrival candidate station to the arrival place, or `u16::MAX`.
    egress_by_station: Vec<u16>,
    /// Stations of the last query that have an egress, so the next reset only clears those.
    egress_stations: Vec<BundleStationIndex>,
    /// Fewest minutes from each station to the arrival place, or `u32::MAX` when it cannot
    /// reach the place.
    lower_bound_by_station: Vec<u32>,
    /// Priority queue of the lower bound search.
    lower_bound_queue: BinaryHeap<Reverse<(u32, u32)>>,
}

impl RaptorSearchBuffers {
    /// Creates empty buffers. The first search sizes them to its bundle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears the buffers for a search over a bundle.
    fn reset(&mut self, station_count: usize, route_count: usize, rounds: usize) {
        self.labels_by_round_and_station.clear();
        self.labels_by_round_and_station
            .resize(station_count * rounds, None);
        self.target_bound_by_round.clear();
        self.target_bound_by_round.resize(rounds, u32::MAX);
        self.stations_improved.clear();
        self.stations_improved.grow(station_count);
        self.ride_arrival_by_station.clear();
        self.ride_arrival_by_station.resize(station_count, None);
        self.stations_reached_by_rides.clear();
        self.stations_changed_in_previous_round.clear();
        self.stations_carried.clear();
        self.scan_start_by_route.clear();
        self.scan_start_by_route.resize(route_count, NOT_QUEUED);
        self.queued_routes.clear();
        self.egress_by_station.resize(station_count, u16::MAX);
        for station in self.egress_stations.drain(..) {
            if let Some(egress) = self.egress_by_station.get_mut(station.as_usize()) {
                *egress = u16::MAX;
            }
        }
    }
}

/// A trip the scan currently rides.
#[derive(Debug, Clone, Copy)]
struct BoardedTrip {
    trip_offset: u32,
    service_day: u16,
    boarding_position: u16,
    /// Departure from the boarding stop, in minutes since the bundle start.
    departure: u32,
}

/// One search, with the query, the bundle and the working memory.
struct Search<'a, F: RideFilter> {
    bundle: &'a LoadedTimetableBundle,
    index: &'a SearchIndex,
    query: &'a RaptorLegQuery<'a>,
    filter: &'a F,
    buffers: &'a mut RaptorSearchBuffers,
    station_count: usize,
}

/// Searches journeys for one leg and returns the Pareto set over departure, arrival and number
/// of trains.
///
/// `index` must be built from the same bundle. `should_stop` is checked between
/// departure times, and the search returns what it has when it returns `true`.
pub fn search_leg_journeys<F: RideFilter>(
    bundle: &LoadedTimetableBundle,
    index: &SearchIndex,
    query: &RaptorLegQuery<'_>,
    filter: &F,
    buffers: &mut RaptorSearchBuffers,
    should_stop: impl Fn() -> bool,
) -> ParetoJourneySet {
    let rounds = usize::from(query.maximum_train_rides.get()) + 1;
    let station_count = bundle.station_count();
    buffers.reset(station_count, bundle.route_count(), rounds);
    index.lower_bounds.compute_bounds(
        query.arrival_candidates,
        &mut buffers.lower_bound_by_station,
        &mut buffers.lower_bound_queue,
    );
    for candidate in query.arrival_candidates {
        if let Some(egress) = buffers
            .egress_by_station
            .get_mut(candidate.station.as_usize())
        {
            *egress = (*egress).min(candidate.time_to_place_minutes);
            buffers.egress_stations.push(candidate.station);
        }
    }
    let departure_times = collect_departure_times(bundle, query);
    let mut search = Search {
        bundle,
        index,
        query,
        filter,
        buffers,
        station_count,
    };
    let mut best_arrival_by_rides = vec![u32::MAX; rounds];
    let mut found = ParetoJourneySet::default();
    for departures in departure_times.chunk_by(|left, right| left.0 == right.0) {
        if should_stop() {
            break;
        }
        let departure_time = departures[0].0;
        let departing_candidates: SmallVec<[u16; 4]> =
            departures.iter().map(|&(_, candidate)| candidate).collect();
        search.run_rounds(departure_time, &departing_candidates);
        // Round 0 is walking only and never reaches the arrival place by train.
        for (round, best_arrival) in best_arrival_by_rides.iter_mut().enumerate().skip(1) {
            let Some((arrival, station)) = search.best_target_arrival(round) else {
                continue;
            };
            if arrival >= *best_arrival {
                continue;
            }
            *best_arrival = arrival;
            // A journey whose first train leaves long after the window would have the
            // traveller leave in time and wait at the station, so it counts as outside the
            // window.
            if let Some(journey) = search.reconstruct(round, station, arrival)
                && journey.departure < query.departure_window.end
            {
                found.journeys.push(journey);
            }
        }
    }
    keep_pareto_optimal(&mut found);
    found
}

impl<F: RideFilter> Search<'_, F> {
    /// Returns the label of a station in a round.
    #[inline]
    fn label(&self, round: usize, station: BundleStationIndex) -> Option<Label> {
        self.buffers.labels_by_round_and_station[round * self.station_count + station.as_usize()]
    }

    /// Stores a label if it improves the station in this round, and updates the target bound.
    /// Returns whether it did.
    ///
    /// Both checks compare within one round only. Range search keeps labels from later
    /// departure times, and a bound over all rounds could then prune a slower journey with
    /// fewer trains, which is still Pareto-optimal.
    #[inline]
    fn improve(&mut self, round: usize, station: BundleStationIndex, label: Label) -> bool {
        let index = station.as_usize();
        let slot_index = round * self.station_count + index;
        let existing = self.buffers.labels_by_round_and_station[slot_index];
        if existing.is_some_and(|existing| label.arrival >= existing.arrival)
            || !self.may_beat_target(round, station, label.arrival)
        {
            return false;
        }
        self.buffers.labels_by_round_and_station[slot_index] = Some(label);
        self.buffers.stations_improved.insert(index);
        let egress = self.buffers.egress_by_station[index];
        if egress != u16::MAX {
            let bound = &mut self.buffers.target_bound_by_round[round];
            *bound = (*bound).min(label.arrival + u32::from(egress));
        }
        true
    }

    /// Returns whether being at a station at a moment can still lead to an arrival at the
    /// arrival place before the best one known for this round, by the station's lower bound.
    /// Stations that cannot reach the place never can.
    #[inline]
    fn may_beat_target(&self, round: usize, station: BundleStationIndex, moment: u32) -> bool {
        let lower_bound = self.buffers.lower_bound_by_station[station.as_usize()];
        moment.saturating_add(lower_bound) < self.buffers.target_bound_by_round[round]
    }

    /// Runs all rounds for one departure time from the departure place. Labels from runs for
    /// later departure times stay, because a journey that leaves later can also be caught by
    /// leaving earlier and waiting.
    ///
    /// Every candidate gets its earlier label, but only `departing_candidates` start route
    /// scans. The earlier label of another candidate only opens trains that leave it before the
    /// previous run's departure time, and each of those has a run of its own.
    fn run_rounds(&mut self, departure_time: u32, departing_candidates: &[u16]) {
        self.buffers.stations_improved.clear();
        self.buffers.stations_changed_in_previous_round.clear();
        for (candidate_index, candidate) in (0_u16..).zip(self.query.departure_candidates) {
            let label = Label {
                arrival: departure_time + u32::from(candidate.time_to_place_minutes),
                origin: LabelOrigin::DepartureCandidate {
                    candidate: candidate_index,
                },
            };
            // Round 0 has no target bound, because walking alone never reaches the arrival
            // place.
            let index = candidate.station.as_usize();
            let slot = &mut self.buffers.labels_by_round_and_station[index];
            if slot.is_none_or(|existing| label.arrival < existing.arrival) {
                *slot = Some(label);
                self.buffers
                    .stations_changed_in_previous_round
                    .push(candidate.station);
                if departing_candidates.contains(&candidate_index) {
                    self.buffers.stations_improved.insert(index);
                }
            }
        }
        let rounds = usize::from(self.query.maximum_train_rides.get()) + 1;
        for round in 1..rounds {
            // The copies change this round's labels, so they count as changes for the next
            // round too, next to the improvements the scans make.
            self.carry_labels_forward(round);
            if !self.buffers.stations_improved.is_clear() {
                self.queue_routes();
                self.buffers.stations_improved.clear();
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
            let improved = self
                .buffers
                .stations_improved
                .ones()
                .map(|index| BundleStationIndex(u32::try_from(index).unwrap_or(u32::MAX)));
            self.buffers
                .stations_changed_in_previous_round
                .extend(improved);
            if self.buffers.stations_changed_in_previous_round.is_empty() {
                break;
            }
        }
    }

    /// Copies each label that changed in the previous round into this round where it is better,
    /// because a journey with fewer rides also counts as one with at most this many.
    ///
    /// Labels that did not change already satisfy this from earlier rounds and runs, so only
    /// the changed stations need a look.
    fn carry_labels_forward(&mut self, round: usize) {
        let previous_bound = self.buffers.target_bound_by_round[round - 1];
        let bound = &mut self.buffers.target_bound_by_round[round];
        *bound = (*bound).min(previous_bound);
        let previous_offset = (round - 1) * self.station_count;
        let current_offset = round * self.station_count;
        let changed = std::mem::take(&mut self.buffers.stations_changed_in_previous_round);
        let mut updated = std::mem::take(&mut self.buffers.stations_carried);
        updated.clear();
        for &station in &changed {
            let index = station.as_usize();
            let labels = &mut self.buffers.labels_by_round_and_station;
            if let Some(previous) = labels[previous_offset + index]
                && labels[current_offset + index]
                    .is_none_or(|current| previous.arrival < current.arrival)
            {
                labels[current_offset + index] = Some(previous);
                updated.push(station);
            }
        }
        // The list of changes for the next round starts with the copies. The two buffers swap,
        // so neither allocates after the first query.
        self.buffers.stations_carried = changed;
        self.buffers.stations_changed_in_previous_round = updated;
    }

    /// Queues every route that stops at a station improved in the previous round, from the
    /// earliest such stop.
    fn queue_routes(&mut self) {
        for station_index in self.buffers.stations_improved.ones() {
            let station = BundleStationIndex(u32::try_from(station_index).unwrap_or(u32::MAX));
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

    /// Scans one route from a stop position, boarding the earliest reachable trip and improving
    /// the stations it reaches.
    fn scan_route(&mut self, round: usize, raptor_route: RaptorRouteIndex, start: u16) {
        let route = self.bundle.route(raptor_route);
        let mut boarded: Option<BoardedTrip> = None;
        for position in start..route.stop_count {
            let pattern_stop = self.bundle.pattern_stop(&route, position);
            let station = pattern_stop.station();
            if let Some(trip) = boarded
                && pattern_stop.alighting_allowed()
            {
                let times = self.bundle.stop_event(&route, trip.trip_offset, position);
                let arrival =
                    u32::from(trip.service_day) * MINUTES_PER_DAY + u32::from(times.arrival.0);
                let may_alight = self.filter.may_alight(
                    raptor_route,
                    BundleTripIndex(route.first_trip_index + trip.trip_offset),
                    MinutesSinceBundleStart(arrival),
                    position + 1 == route.stop_count,
                );
                if may_alight {
                    let ride = RideOrigin {
                        raptor_route,
                        trip_offset: trip.trip_offset,
                        service_day: trip.service_day,
                        boarding_position: trip.boarding_position,
                    };
                    self.record_ride_arrival(round, station, arrival, ride);
                    self.improve(
                        round,
                        station,
                        Label {
                            arrival,
                            origin: LabelOrigin::TrainRide(ride),
                        },
                    );
                }
            }
            if !pattern_stop.boarding_allowed() {
                continue;
            }
            let Some(previous) = self.label(round - 1, station) else {
                continue;
            };
            let ready = previous.arrival + self.transfer_slack(previous.origin);
            if boarded.is_some_and(|trip| trip.departure <= ready)
                || !self.may_beat_target(round, station, ready)
            {
                continue;
            }
            if let Some(trip) = self.earliest_trip(raptor_route, &route, position, ready)
                && boarded.is_none_or(|current| trip.departure < current.departure)
            {
                boarded = Some(trip);
            }
        }
    }

    /// Returns the minutes a traveller needs at a station before boarding, which is the minimum
    /// transfer time after a ride and nothing after walking there.
    #[inline]
    fn transfer_slack(&self, origin: LabelOrigin) -> u32 {
        match origin {
            LabelOrigin::TrainRide(_) => u32::from(self.query.minimum_transfer_minutes),
            LabelOrigin::DepartureCandidate { .. } | LabelOrigin::TrainRideAndFootpath { .. } => 0,
        }
    }

    /// Keeps the earliest train arrival at a station in this round, for the footpaths that start
    /// there. Arrivals that cannot beat the target bound are left out. The lower bound of the
    /// station covers its footpaths, because they are edges of the lower bound graph.
    #[inline]
    fn record_ride_arrival(
        &mut self,
        round: usize,
        station: BundleStationIndex,
        arrival: u32,
        ride: RideOrigin,
    ) {
        if !self.may_beat_target(round, station, arrival) {
            return;
        }
        let slot = &mut self.buffers.ride_arrival_by_station[station.as_usize()];
        match slot {
            Some((existing, _)) if *existing <= arrival => {}
            Some(_) => *slot = Some((arrival, ride)),
            None => {
                *slot = Some((arrival, ride));
                self.buffers.stations_reached_by_rides.push(station);
            }
        }
    }

    /// Finds the earliest trip of a route that departs from a stop position at or after
    /// `ready` and runs on the matching day.
    ///
    /// A trip with service day `d` departs at `d * 1440 + departure`. Trips that run past
    /// midnight can depart up to `longest_trip_days` days after their service day, so the
    /// search looks at that many earlier service days, and at the next day.
    fn earliest_trip(
        &self,
        raptor_route: RaptorRouteIndex,
        route: &RaptorRoute,
        position: u16,
        ready: u32,
    ) -> Option<BoardedTrip> {
        let ready_day = ready / MINUTES_PER_DAY;
        let first_day = ready_day.saturating_sub(u32::from(route.longest_trip_days));
        let last_day =
            (ready_day + 1).min(u32::from(self.bundle.validity_days()).saturating_sub(1));
        let departure_at = |trip_offset: u32| {
            self.bundle
                .stop_event(route, trip_offset, position)
                .departure
                .0
        };
        let mut best: Option<BoardedTrip> = None;
        let last_trip = route.trip_count.checked_sub(1)?;
        let route_days = self.index.service_days_by_route[raptor_route.as_usize()];
        for day in first_day..=last_day {
            let service_day = u16::try_from(day).unwrap_or(u16::MAX);
            if !route_days.runs_on(service_day) {
                continue;
            }
            let day_start = day * MINUTES_PER_DAY;
            // Trips of a later service date all depart after any trip found on an earlier one.
            if best.is_some_and(|best| day_start + u32::from(departure_at(0)) >= best.departure) {
                break;
            }
            let Ok(earliest_minute) = u16::try_from(ready.saturating_sub(day_start)) else {
                continue;
            };
            // Most routes have no trip that is still underway a day later at this stop, which
            // the last trip shows without a search.
            if departure_at(last_trip) < earliest_minute {
                continue;
            }
            let mut offset = partition_point(route.trip_count, |trip_offset| {
                departure_at(trip_offset) < earliest_minute
            });
            while offset < route.trip_count {
                let departure = day_start + u32::from(departure_at(offset));
                if best.is_some_and(|best| departure >= best.departure) {
                    break;
                }
                let trip = BundleTripIndex(route.first_trip_index + offset);
                if self.bundle.trip_service_days(trip).runs_on(service_day)
                    && self
                        .filter
                        .may_board(raptor_route, trip, MinutesSinceBundleStart(departure))
                {
                    best = Some(BoardedTrip {
                        trip_offset: offset,
                        service_day,
                        boarding_position: position,
                        departure,
                    });
                    break;
                }
                offset += 1;
            }
        }
        best
    }

    /// Walks footpaths from every train arrival of this round, whether or not the arrival
    /// improved its station. Footpaths are one hop, so arrivals on foot are not walked on.
    fn relax_footpaths(&mut self, round: usize) {
        let reached = std::mem::take(&mut self.buffers.stations_reached_by_rides);
        for &station in &reached {
            let Some((ride_arrival, ride)) =
                self.buffers.ride_arrival_by_station[station.as_usize()].take()
            else {
                continue;
            };
            for footpath in self.bundle.footpaths_from(station) {
                let arrival = ride_arrival + u32::from(footpath.duration_minutes);
                let origin = LabelOrigin::TrainRideAndFootpath {
                    ride,
                    alighting_station: station,
                };
                self.improve(round, footpath.to_station, Label { arrival, origin });
            }
        }
        self.buffers.stations_reached_by_rides = reached;
        self.buffers.stations_reached_by_rides.clear();
    }

    /// Returns the best arrival at the arrival place within a number of rides, with the
    /// arrival candidate station it goes through.
    fn best_target_arrival(&self, round: usize) -> Option<(u32, BundleStationIndex)> {
        self.query
            .arrival_candidates
            .iter()
            .filter_map(|candidate| {
                let label = self.label(round, candidate.station)?;
                Some((
                    label.arrival + u32::from(candidate.time_to_place_minutes),
                    candidate.station,
                ))
            })
            .min_by_key(|(arrival, _)| *arrival)
    }

    /// Follows the labels back from an arrival candidate to a departure candidate.
    fn reconstruct(
        &self,
        round: usize,
        target: BundleStationIndex,
        arrival: u32,
    ) -> Option<RaptorJourney> {
        let arrival_candidate = *self
            .query
            .arrival_candidates
            .iter()
            .find(|candidate| candidate.station == target)?;
        let mut rides: SmallVec<[RaptorRide; 4]> = SmallVec::new();
        let mut current_round = round;
        let mut station = target;
        // Every step lowers the round by one or ends the walk, so it takes at most round + 1
        // steps.
        for _ in 0..=round {
            let label = self.label(current_round, station)?;
            let (ride, alighting_station) = match label.origin {
                LabelOrigin::DepartureCandidate { candidate } => {
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
                LabelOrigin::TrainRide(ride) => (ride, station),
                LabelOrigin::TrainRideAndFootpath {
                    ride,
                    alighting_station,
                } => (ride, alighting_station),
            };
            let raptor_ride = self.raptor_ride(ride, alighting_station)?;
            station = raptor_ride.boarding_station;
            rides.push(raptor_ride);
            current_round = current_round.checked_sub(1)?;
        }
        None
    }

    /// Turns a recorded ride into its boarding and alighting stops and times.
    fn raptor_ride(
        &self,
        ride: RideOrigin,
        alighting_station: BundleStationIndex,
    ) -> Option<RaptorRide> {
        let route = self.bundle.route(ride.raptor_route);
        let boarding_station = self
            .bundle
            .pattern_stop(&route, ride.boarding_position)
            .station();
        let alighting_position =
            (ride.boarding_position + 1..route.stop_count).find(|&position| {
                self.bundle.pattern_stop(&route, position).station() == alighting_station
            })?;
        let day_start = u32::from(ride.service_day) * MINUTES_PER_DAY;
        let departure = self
            .bundle
            .stop_event(&route, ride.trip_offset, ride.boarding_position)
            .departure
            .0;
        let arrival = self
            .bundle
            .stop_event(&route, ride.trip_offset, alighting_position)
            .arrival
            .0;
        Some(RaptorRide {
            raptor_route: ride.raptor_route,
            trip: BundleTripIndex(route.first_trip_index + ride.trip_offset),
            service_day: ride.service_day,
            boarding_station,
            boarding_position: ride.boarding_position,
            alighting_station,
            alighting_position,
            departure: MinutesSinceBundleStart(day_start + u32::from(departure)),
            arrival: MinutesSinceBundleStart(day_start + u32::from(arrival)),
        })
    }
}

/// Returns every departure from the departure place within the window, from the latest to the
/// earliest, as the time the traveller leaves the place for a train that leaves then, with the
/// candidate station the train leaves from.
fn collect_departure_times(
    bundle: &LoadedTimetableBundle,
    query: &RaptorLegQuery<'_>,
) -> Vec<(u32, u16)> {
    let mut departure_times = Vec::new();
    let window_start = query.departure_window.start.0;
    let window_end = query.departure_window.end.0;
    let last_day = u32::from(bundle.validity_days()).saturating_sub(1);
    for (candidate_index, candidate) in (0_u16..).zip(query.departure_candidates) {
        let access = u32::from(candidate.time_to_place_minutes);
        let (earliest, latest) = (window_start + access, window_end + access);
        for route_stop in bundle.route_stops_at(candidate.station) {
            let route = bundle.route(route_stop.raptor_route);
            let position = route_stop.stop_position;
            if !bundle.pattern_stop(&route, position).boarding_allowed() {
                continue;
            }
            let first_day =
                (earliest / MINUTES_PER_DAY).saturating_sub(u32::from(route.longest_trip_days));
            for day in first_day..=(latest / MINUTES_PER_DAY).min(last_day) {
                let day_start = day * MINUTES_PER_DAY;
                let service_day = u16::try_from(day).unwrap_or(u16::MAX);
                let earliest_minute =
                    u16::try_from(earliest.saturating_sub(day_start)).unwrap_or(u16::MAX);
                let mut offset = partition_point(route.trip_count, |trip_offset| {
                    bundle.stop_event(&route, trip_offset, position).departure.0 < earliest_minute
                });
                while offset < route.trip_count {
                    let departure = day_start
                        + u32::from(bundle.stop_event(&route, offset, position).departure.0);
                    if departure >= latest {
                        break;
                    }
                    let trip = BundleTripIndex(route.first_trip_index + offset);
                    if bundle.trip_service_days(trip).runs_on(service_day) {
                        departure_times.push((departure - access, candidate_index));
                    }
                    offset += 1;
                }
            }
        }
    }
    departure_times.sort_unstable_by(|left, right| right.cmp(left));
    departure_times.dedup();
    departure_times
}

/// Returns the first trip offset for which `is_before` is false. Trips of a route are sorted by
/// departure at every stop, so `is_before` is true for a prefix of the offsets.
fn partition_point(trip_count: u32, is_before: impl Fn(u32) -> bool) -> u32 {
    let (mut low, mut high) = (0, trip_count);
    while low < high {
        let middle = low + (high - low) / 2;
        if is_before(middle) {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    low
}

/// Removes dominated journeys and sorts the rest by arrival.
fn keep_pareto_optimal(set: &mut ParetoJourneySet) {
    let journeys = std::mem::take(&mut set.journeys);
    for (index, journey) in journeys.iter().enumerate() {
        let dominated = journeys.iter().enumerate().any(|(other_index, other)| {
            other_index != index
                && (other.dominates(journey) || (other == journey && other_index < index))
        });
        if !dominated {
            set.journeys.push(journey.clone());
        }
    }
    set.journeys.sort_by_key(|journey| {
        (
            journey.arrival,
            journey.ride_count(),
            std::cmp::Reverse(journey.departure),
        )
    });
}
