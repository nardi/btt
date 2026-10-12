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
    RaptorLegQuery, RaptorRide, RideOrigin, WindowDeparture, collect_departures, earliest_trip,
    raptor_ride,
};

/// Marks a route that is not queued for scanning in the current round.
const NOT_QUEUED: u16 = u16::MAX;

/// Marks a station and round without a bag, or a label without a parent.
const NONE: u32 = u32::MAX;

/// Marks a journey that uses no travel day yet. A real day stays far below this, since a
/// bundle runs for a few months.
const NO_DAY: u16 = u16::MAX;

/// The most travel days a journey may use when the model counts them. The search drops a
/// journey that would use more.
pub const MAXIMUM_TRAVEL_DAYS: u8 = 3;

/// Number of travel day counts the search keeps arrivals and cost limits apart for, from zero
/// to `MAXIMUM_TRAVEL_DAYS`.
const TRAVEL_DAY_COUNTS: usize = MAXIMUM_TRAVEL_DAYS as usize + 1;

/// Marks a station from which no ride that lowers the cost can be reached.
const NO_CREDIT: i32 = i32::MIN;

/// The ride before the one whose cost the model returns, which lets the model charge a ride
/// that continues a ticket run of the same operator as the extra over the ride before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviousRide {
    /// Route of the previous ride.
    pub raptor_route: RaptorRouteIndex,
    /// Position where the traveller boarded it.
    pub boarding_position: u16,
    /// Position where the traveller left it.
    pub alighting_position: u16,
}

/// Costs that a search adds up along a journey, in a unit the model chooses.
///
/// The search relies on three properties. `elapsed_cost` never falls when the arrival moves
/// later or the departure moves earlier. `ride_cost` of boarding the same trip at an earlier
/// stop rises by no more than `boarding_extension` says, which the boarding shortcuts compare.
/// And `remaining_cost_lower_bound` never exceeds the cost still to come.
pub trait JourneyCostModel {
    /// Returns the added cost of getting from the departure place to a station.
    fn access_cost(&self, candidate: &CandidateStation) -> i32;

    /// Returns the added cost of a ride on a trip from its departure to its arrival. The
    /// traveller boards at `boarding_position` and leaves at `alighting_position`, and
    /// `previous` is the ride before this one, or `None` when this is the first.
    #[allow(clippy::too_many_arguments)]
    fn ride_cost(
        &self,
        raptor_route: RaptorRouteIndex,
        trip: BundleTripIndex,
        boarding_position: u16,
        alighting_position: u16,
        departure: MinutesSinceBundleStart,
        arrival: MinutesSinceBundleStart,
        previous: Option<PreviousRide>,
    ) -> i32;

    /// Returns the most that riding a route from the stop at `earlier` can cost on top of riding
    /// it from the stop at `later`, whichever stop after both the traveller leaves at, so the
    /// boarding shortcuts know what boarding earlier has to pay for. Returns `i32::MAX` when
    /// the model knows no such bound, which turns the shortcuts off for the pair. The default
    /// is nothing, for a model whose ride cost does not depend on where the ride begins.
    fn boarding_extension(&self, raptor_route: RaptorRouteIndex, earlier: u16, later: u16) -> i32 {
        let _ = (raptor_route, earlier, later);
        0
    }

    /// Returns the day a ride counts as a travel day on, as days since the start of the
    /// bundle, or `None` when the ride counts none, such as on single tickets or when a pass
    /// does not take the train. Journeys then keep a small criterion of how many travel days
    /// they use, and a label only drops another that uses no more. A journey uses at most
    /// `MAXIMUM_TRAVEL_DAYS`. The default counts none.
    fn travel_day_of(
        &self,
        raptor_route: RaptorRouteIndex,
        trip: BundleTripIndex,
        boarding_position: u16,
        departure: MinutesSinceBundleStart,
    ) -> Option<u16> {
        let _ = (raptor_route, trip, boarding_position, departure);
        None
    }

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
    /// arrival place before `earliest_arrival`. `credit_minutes_cap` is the most credit minutes
    /// that rides from this station at `moment` can still give, and is only given when
    /// `credit_window` is.
    fn remaining_cost_lower_bound(
        &self,
        departure: MinutesSinceBundleStart,
        moment: MinutesSinceBundleStart,
        earliest_arrival: MinutesSinceBundleStart,
        credit_minutes_cap: Option<u32>,
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

    /// Returns the moments during which a ride can lower the cost, such as the night, or `None`
    /// when no ride can. With a window, the search finds how much credit rides that leave
    /// before the window ends can still give a traveller at each station, and passes that to
    /// `remaining_cost_lower_bound`. The default has no window.
    fn credit_window(&self) -> Option<std::ops::Range<u32>> {
        None
    }

    /// Returns the most minutes of credit that a trip gives a traveller who boards at
    /// `departure` and rides to its last stop, which it reaches at `trip_end`. Only trips of
    /// routes whose `ride_cost_depends_on_time` is `true` are asked. The default gives none.
    fn ride_credit_minutes(
        &self,
        raptor_route: RaptorRouteIndex,
        trip: BundleTripIndex,
        departure: MinutesSinceBundleStart,
        trip_end: MinutesSinceBundleStart,
    ) -> u32 {
        let _ = (raptor_route, trip, departure, trip_end);
        0
    }
}

/// A journey with its cost under the model that found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostedJourney {
    /// The journey.
    pub journey: RaptorJourney,
    /// The elapsed cost plus every added cost, in the model's unit.
    pub cost: i32,
    /// How many travel days the journey uses. Zero when the model counts none.
    pub days: u8,
}

impl CostedJourney {
    /// Returns whether this journey is at least as good as another in departure, arrival,
    /// rides, travel days and cost, and better in at least one.
    fn dominates(&self, other: &Self) -> bool {
        let (own, theirs) = (&self.journey, &other.journey);
        let at_least_as_good = own.departure >= theirs.departure
            && own.arrival <= theirs.arrival
            && own.ride_count() <= theirs.ride_count()
            && self.days <= other.days
            && self.cost <= other.cost;
        let better = own.departure > theirs.departure
            || own.arrival < theirs.arrival
            || own.ride_count() < theirs.ride_count()
            || self.days < other.days
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

/// The travel days a journey uses so far, for models that count them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TravelDays {
    /// How many travel days the journey uses. Zero when the model counts none.
    count: u8,
    /// The day of the last ride that counted as a travel day, as days since the start of the
    /// bundle. `NO_DAY` when the journey uses none.
    last_day: u16,
}

impl TravelDays {
    /// The days of a journey that has ridden no train yet.
    const NONE: Self = Self {
        count: 0,
        last_day: NO_DAY,
    };

    /// Returns the travel days after a ride that counts on `day`, or the same days when the
    /// ride counts none. Rides come in order of time, so a ride on another day than the last
    /// one takes a new travel day.
    fn after_ride(self, day: Option<u16>) -> Self {
        match day {
            Some(day) if day != NO_DAY && day != self.last_day => Self {
                count: self.count.saturating_add(1),
                last_day: day,
            },
            _ => self,
        }
    }

    /// Returns whether a journey with these days ends up with no more travel days than one
    /// with the other days, after any rides both go on with. Fewer days are enough, because
    /// the rides add at most one day more to this one, and after the first counted ride both
    /// have the same last day. Equal counts need the same last day, because a ride on the
    /// other's last day would add a day to this one only.
    fn covers(self, other: Self) -> bool {
        self.count < other.count || self == other
    }

    /// Returns the position of the count among the counts the search keeps apart.
    fn index(self) -> usize {
        usize::from(self.count).min(TRAVEL_DAY_COUNTS - 1)
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
    TrainRide {
        ride: RideOrigin,
        /// Position where the traveller left the trip.
        alighting_position: u16,
    },
    /// The traveller rode a trip to another station and walked from there.
    TrainRideAndFootpath {
        ride: RideOrigin,
        /// Position where the traveller left the trip.
        alighting_position: u16,
        alighting_station: BundleStationIndex,
        footpath_minutes: u16,
    },
}

impl CostLabelOrigin {
    /// Returns whether the label is the walk from the departure place.
    fn is_walk(&self) -> bool {
        matches!(self, Self::DepartureCandidate { .. })
    }
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
    /// The travel days the journey so far uses.
    days: TravelDays,
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
            && self.days.covers(other.days)
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
    /// The travel days the journey uses with this ride, which counts on the day it is
    /// boarded.
    days: TravelDays,
}

impl RouteLabel {
    /// Returns whether riding this is at least as good as riding another at every later stop.
    ///
    /// On the same trip, boarding earlier is when the cost before the ride plus the price of
    /// the stops between the boardings reaches no more than the other's cost before the ride,
    /// because the earlier boarding pays for the extra distance. On another trip, only when
    /// the ride cost doesn't depend on the trip's times, and the trip runs earlier on the same
    /// service day, so it reaches every later stop no later. `extension` gives the price of the
    /// stops between this boarding and the other's. It is never negative, and the model prices
    /// it with a logarithm and an exponential, so it is only called once every cheaper
    /// condition holds. Either way the ride must use no more travel days.
    #[inline]
    fn covers(
        &self,
        other: &Self,
        ride_cost_depends_on_time: bool,
        extension: impl FnOnce() -> i32,
    ) -> bool {
        if self.cost_before_ride > other.cost_before_ride
            || (!self.is_first_ride && other.is_first_ride)
            || !self.days.covers(other.days)
        {
            return false;
        }
        let same_trip =
            self.trip_offset == other.trip_offset && self.service_day == other.service_day;
        let timing_covers = if same_trip {
            self.departure <= other.departure
        } else {
            !ride_cost_depends_on_time
                && self.service_day == other.service_day
                && self.trip_offset < other.trip_offset
        };
        timing_covers && self.cost_before_ride.saturating_add(extension()) <= other.cost_before_ride
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

/// A hasher for keys that are already well mixed integers, such as a trip number with a day.
#[derive(Debug, Default)]
struct TripKeyHasher(u64);

impl std::hash::Hasher for TripKeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn write_u64(&mut self, value: u64) {
        let mixed = (value ^ (value >> 32)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        self.0 = mixed ^ (mixed >> 29);
    }
}

/// A way a trip was boarded earlier in the search. Another boarding of the same trip at the same
/// stop or later, with no lower cost and no fewer rides, reaches nothing that this one did not.
#[derive(Debug, Clone, Copy)]
struct BoardedBefore {
    /// Position of the boarding stop in the route's stop sequence.
    position: u16,
    /// Added cost before the ride.
    cost: i32,
    /// Whether the ride was the first of its journey, which has no change penalty.
    is_first_ride: bool,
    /// The round of the ride.
    round: u8,
    /// The travel days the journey uses with the ride.
    days: TravelDays,
}

impl BoardedBefore {
    /// Returns whether this boarding reaches everything that another one does, at no more cost,
    /// no more rides and no more travel days. `extension` gives the price of the stops between
    /// this boarding and the other's, which the earlier boarding pays for. It is never
    /// negative, and only called once every cheaper condition holds.
    fn covers(self, other: Self, extension: impl FnOnce() -> i32) -> bool {
        self.position <= other.position
            && self.cost <= other.cost
            && (self.is_first_ride || !other.is_first_ride)
            && self.round <= other.round
            && self.days.covers(other.days)
            && self.cost.saturating_add(extension()) <= other.cost
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
    /// Position where the traveller left the trip.
    alighting_position: u16,
    /// The travel days the journey so far uses.
    days: TravelDays,
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
    /// Arrivals at the arrival place found per round and travel day count, flattened as
    /// `round * TRAVEL_DAY_COUNTS + count`. Labels that can't beat them are pruned. An arrival
    /// prunes only labels with at least its travel days.
    targets_by_round: Vec<ArrivalStaircase>,
    /// Highest cost worth searching for per travel day count, from the model and the cheapest
    /// arrival with that many days found so far.
    cost_limit_by_days: [i32; TRAVEL_DAY_COUNTS],
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
    /// Stations with labels from the previous round, which are the only ones that can board.
    boardable_stations: FixedBitSet,
    /// How each trip was boarded in earlier runs and rounds, by trip number and service day.
    boarded_before: std::collections::HashMap<
        u64,
        SmallVec<[BoardedBefore; 2]>,
        std::hash::BuildHasherDefault<TripKeyHasher>,
    >,
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
    /// For each tier of cost-lowering rides, from where it can still be reached.
    credit_reach: CreditReach,
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
            .resize_with(rounds * TRAVEL_DAY_COUNTS, ArrivalStaircase::default);
        self.cost_limit_by_days = [i32::MAX; TRAVEL_DAY_COUNTS];
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
        self.boardable_stations.clear();
        self.boardable_stations.grow(station_count);
        self.route_labels.clear();
        self.boarded_before.clear();
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

/// Number of tiers of cost-lowering rides, by how many minutes of credit they give.
const CREDIT_TIERS: usize = 4;

/// For each tier of ride, the latest moment at every station from which a ride of the tier can
/// still be reached, and the credit minutes the tiers start at.
#[derive(Debug, Default)]
struct CreditReach {
    /// Whether the model has a credit window, which makes the rest valid.
    enabled: bool,
    /// Minutes of credit a ride needs to belong to tier `k`, and the most credit any ride gives.
    thresholds: [u32; CREDIT_TIERS],
    /// The most credit minutes any ride gives.
    maximum_credit: u32,
    /// The latest moment at each station per tier, or `NO_CREDIT`.
    latest_by_tier: [Vec<i32>; CREDIT_TIERS],
    /// Priority queue of the search, latest moment first.
    queue: BinaryHeap<(i32, u32)>,
}

impl CreditReach {
    /// Returns an upper bound on the credit minutes that a traveller at a station at a moment
    /// can still earn. A tier that can't be reached means the credit stays below its threshold.
    fn credit_cap(&self, station: BundleStationIndex, moment: u32) -> Option<u32> {
        if !self.enabled {
            return None;
        }
        let moment = i32::try_from(moment).unwrap_or(i32::MAX);
        let mut cap = 0;
        for tier in (0..CREDIT_TIERS).rev() {
            if self.latest_by_tier[tier][station.as_usize()] >= moment {
                cap = match self.thresholds.get(tier + 1) {
                    Some(&next) => next.saturating_sub(1),
                    None => self.maximum_credit,
                };
                break;
            }
        }
        Some(cap)
    }

    /// Finds the latest moments for a model. A ride counts when it departs before the window
    /// ends and gives credit. Travel between stations takes at least the fastest time of the
    /// lower bound graph, which makes each moment an upper bound.
    fn compute<M: JourneyCostModel>(
        &mut self,
        bundle: &LoadedTimetableBundle,
        index: &SearchIndex,
        model: &M,
    ) {
        let Some(window) = model.credit_window() else {
            self.enabled = false;
            return;
        };
        self.enabled = true;
        let mut boardings: Vec<(u32, i32, u32)> = Vec::new();
        let last_day = (window.end / MINUTES_PER_DAY).min(u32::from(bundle.validity_days()));
        for route_index in 0..bundle.route_count() {
            let raptor_route = RaptorRouteIndex(u32::try_from(route_index).unwrap_or(u32::MAX));
            if !model.ride_cost_depends_on_time(raptor_route) {
                continue;
            }
            let route = bundle.route(raptor_route);
            let first_day = (window.start / MINUTES_PER_DAY)
                .saturating_sub(u32::from(route.longest_trip_days) + 1);
            for trip_offset in 0..route.trip_count {
                let trip = BundleTripIndex(route.first_trip_index + trip_offset);
                let service_days = bundle.trip_service_days(trip);
                let end_stop = bundle.stop_event(&route, trip_offset, route.stop_count - 1);
                for day in first_day..=last_day {
                    if !service_days.runs_on(u16::try_from(day).unwrap_or(u16::MAX)) {
                        continue;
                    }
                    let day_start = day * MINUTES_PER_DAY;
                    let trip_end = day_start + u32::from(end_stop.arrival.0);
                    for position in 0..route.stop_count {
                        let pattern_stop = bundle.pattern_stop(&route, position);
                        if !pattern_stop.boarding_allowed() {
                            continue;
                        }
                        let departure = day_start
                            + u32::from(
                                bundle.stop_event(&route, trip_offset, position).departure.0,
                            );
                        let credit = model.ride_credit_minutes(
                            raptor_route,
                            trip,
                            MinutesSinceBundleStart(departure),
                            MinutesSinceBundleStart(trip_end),
                        );
                        if credit > 0 && departure < window.end {
                            boardings.push((
                                pattern_stop.station().0,
                                i32::try_from(departure).unwrap_or(i32::MAX),
                                credit,
                            ));
                        }
                    }
                }
            }
        }
        self.maximum_credit = boardings
            .iter()
            .map(|&(_, _, credit)| credit)
            .max()
            .unwrap_or(0);
        let top = self.maximum_credit.max(1);
        self.thresholds = [1, (top / 4).max(2), (top / 2).max(3), (top * 3 / 4).max(4)];
        for tier in 0..CREDIT_TIERS {
            let latest = &mut self.latest_by_tier[tier];
            latest.clear();
            latest.resize(bundle.station_count(), NO_CREDIT);
            for &(station, departure, credit) in &boardings {
                if credit >= self.thresholds[tier] {
                    let slot = &mut latest[station as usize];
                    *slot = (*slot).max(departure);
                }
            }
            self.queue.clear();
            for (station, &moment) in latest.iter().enumerate() {
                if moment != NO_CREDIT {
                    self.queue
                        .push((moment, u32::try_from(station).unwrap_or(u32::MAX)));
                }
            }
            while let Some((moment, station)) = self.queue.pop() {
                if moment < latest[station as usize] {
                    continue;
                }
                for &(from, minutes) in index.lower_bounds.incoming(station as usize) {
                    let earlier = moment - i32::from(minutes);
                    let slot = &mut latest[from.as_usize()];
                    if earlier > *slot {
                        *slot = earlier;
                        self.queue.push((earlier, from.0));
                    }
                }
            }
        }
    }
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
    buffers.credit_reach.compute(bundle, index, model);
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
    let window_departures = collect_departures(bundle, query);
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
    for departures in window_departures.chunk_by(|left, right| left.time == right.time) {
        if should_stop() {
            break;
        }
        search.departure = departures[0].time;
        let departing_candidates: SmallVec<[u16; 4]> = departures
            .iter()
            .map(|departure| departure.candidate)
            .collect();
        search.run_rounds(&departing_candidates, departures);
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
    /// cost means beating it on every criterion. Only arrivals with no more travel days than
    /// the label count, because the label's days can only grow.
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
                self.buffers.credit_reach.credit_cap(station, label.arrival),
            )
            .saturating_add(label.cost);
        let counts = 0..=label.days.index();
        if self.buffers.cost_limit_by_days[counts.clone()]
            .iter()
            .any(|&limit| cost_bound > limit)
        {
            return false;
        }
        !(1..=round).any(|earlier_round| {
            let start = earlier_round * TRAVEL_DAY_COUNTS;
            self.buffers.targets_by_round[start..=start + label.days.index()]
                .iter()
                .any(|targets| targets.beats(earliest_arrival, cost_bound))
        })
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
        // A walk from the departure place at a station that is also an arrival station is not a
        // journey to it, so it can't keep a ride from arriving there.
        let is_ride_to_arrival_station =
            !label.origin.is_walk() && self.arrival_candidate(label.station).is_some();
        let labels = &self.buffers.labels;
        let existing_bag = self.buffers.bag_by_slot[slot];
        if existing_bag != NONE {
            let entries = &mut self.buffers.bags[existing_bag as usize];
            if entries.iter().any(|&other| {
                let other = &labels[other as usize];
                other.covers(&label) && !(is_ride_to_arrival_station && other.origin.is_walk())
            }) {
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
        // A walk from the departure place that a later round copies is not a journey, even when
        // the arrival place shares the station.
        if !label.origin.is_walk()
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
            let count = label.days.index();
            if self.buffers.targets_by_round[round * TRAVEL_DAY_COUNTS + count]
                .insert(arrival, cost)
            {
                let limit = &mut self.buffers.cost_limit_by_days[count];
                *limit = (*limit).min(self.model.cost_limit(cost));
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
    fn run_rounds(&mut self, departing_candidates: &[u16], departures: &[WindowDeparture]) {
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
                days: TravelDays::NONE,
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
            // The first ride boards a train that leaves when the traveller is at the station.
            // A train that leaves later is reached by the run for its own departure time, which
            // leaves later and so beats this one, so the first round scans only the routes of
            // the trains that leave now.
            let has_routes = if round == 1 {
                self.queue_departing_routes(departures);
                !self.buffers.queued_routes.is_empty()
            } else if self.buffers.stations_created_before.is_empty() {
                false
            } else {
                self.queue_routes(round);
                true
            };
            if has_routes {
                for &station in &self.buffers.stations_created_before {
                    self.buffers.boardable_stations.insert(station.as_usize());
                }
                let queued = std::mem::take(&mut self.buffers.queued_routes);
                for &raptor_route in &queued {
                    let start = self.buffers.scan_start_by_route[raptor_route.as_usize()];
                    self.buffers.scan_start_by_route[raptor_route.as_usize()] = NOT_QUEUED;
                    self.scan_route(round, raptor_route, start);
                }
                self.buffers.queued_routes = queued;
                self.buffers.queued_routes.clear();
                for &station in &self.buffers.stations_created_before {
                    self.buffers
                        .boardable_stations
                        .set(station.as_usize(), false);
                }
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

    /// Returns whether a boarding repeats one from an earlier run or round, and remembers it
    /// when it doesn't. Labels of the earlier boarding are in the bags already, so riding on
    /// from the later one can only produce labels they cover.
    fn repeats_boarding(
        &mut self,
        raptor_route: RaptorRouteIndex,
        trip: BundleTripIndex,
        route_label: &RouteLabel,
        round: usize,
    ) -> bool {
        let boarding = BoardedBefore {
            position: route_label.boarding_position,
            cost: route_label.cost_before_ride,
            is_first_ride: route_label.is_first_ride,
            round: u8::try_from(round).unwrap_or(u8::MAX),
            days: route_label.days,
        };
        let key = (u64::from(trip.0) << 16) | u64::from(route_label.service_day);
        // The boardings are copied out of the map, so the model can be asked for the price of
        // the stops between two of them while the map is not borrowed. Each earlier boarding
        // has its own stops in between.
        let known: SmallVec<[BoardedBefore; 2]> = self
            .buffers
            .boarded_before
            .get(&key)
            .cloned()
            .unwrap_or_default();
        if known.iter().any(|earlier| {
            earlier.covers(boarding, || {
                self.board_extension_cost(raptor_route, earlier.position, boarding.position)
            })
        }) {
            return true;
        }
        let mut kept: SmallVec<[BoardedBefore; 2]> = known
            .into_iter()
            .filter(|earlier| {
                !boarding.covers(*earlier, || {
                    self.board_extension_cost(raptor_route, boarding.position, earlier.position)
                })
            })
            .collect();
        kept.push(boarding);
        self.buffers.boarded_before.insert(key, kept);
        false
    }

    /// Returns what the earlier of two boardings of a route pays on top of its cost before the
    /// ride, as the model bounds it. A boarding no earlier than the other pays for no more
    /// distance, so it adds nothing.
    fn board_extension_cost(
        &self,
        raptor_route: RaptorRouteIndex,
        earlier: u16,
        later: u16,
    ) -> i32 {
        if earlier >= later {
            return 0;
        }
        self.model.boarding_extension(raptor_route, earlier, later)
    }

    /// Returns the ride before the one a route label boards, or `None` when it boards the
    /// first ride of a journey.
    fn previous_ride(&self, route_label: &RouteLabel) -> Option<PreviousRide> {
        let parent = self.buffers.labels.get(route_label.parent as usize)?;
        match parent.origin {
            CostLabelOrigin::DepartureCandidate { .. } => None,
            CostLabelOrigin::TrainRide {
                ride,
                alighting_position,
            }
            | CostLabelOrigin::TrainRideAndFootpath {
                ride,
                alighting_position,
                ..
            } => Some(PreviousRide {
                raptor_route: ride.raptor_route,
                boarding_position: ride.boarding_position,
                alighting_position,
            }),
        }
    }

    /// Queues the routes of the trains that leave now, from the stop where each is boarded.
    fn queue_departing_routes(&mut self, departures: &[WindowDeparture]) {
        for departure in departures {
            self.queue_route_stop(departure.route, departure.position);
        }
    }

    /// Queues every route that stops at a station with labels created in the previous round,
    /// from the earliest such stop. A route is left out when it runs on none of the days on
    /// which a train could be boarded, because the scan would find no trip.
    fn queue_routes(&mut self, round: usize) {
        let previous_round = u8::try_from(round - 1).unwrap_or(u8::MAX);
        for index in 0..self.buffers.stations_created_before.len() {
            let station = self.buffers.stations_created_before[index];
            let mut ready_range: Option<(u32, u32)> = None;
            for &position in self.bag(round - 1, station) {
                let label = &self.buffers.labels[position as usize];
                if label.run == self.run && label.round == previous_round {
                    ready_range = Some(
                        ready_range.map_or((label.ready, label.ready), |(low, high)| {
                            (low.min(label.ready), high.max(label.ready))
                        }),
                    );
                }
            }
            let Some((earliest_ready, latest_ready)) = ready_range else {
                continue;
            };
            let last_day = u16::try_from(latest_ready / MINUTES_PER_DAY + 1).unwrap_or(u16::MAX);
            for route_stop in self.bundle.route_stops_at(station) {
                let route_days =
                    self.index.service_days_by_route[route_stop.raptor_route.as_usize()];
                let first_day = u16::try_from((earliest_ready / MINUTES_PER_DAY).saturating_sub(
                    u32::from(self.bundle.route(route_stop.raptor_route).longest_trip_days),
                ))
                .unwrap_or(u16::MAX);
                if (first_day..=last_day).any(|day| route_days.runs_on(day)) {
                    self.queue_route_stop(route_stop.raptor_route, route_stop.stop_position);
                }
            }
        }
    }

    /// Queues a route to be scanned from a stop position, or from an earlier one it has.
    fn queue_route_stop(&mut self, raptor_route: RaptorRouteIndex, position: u16) {
        let slot = &mut self.buffers.scan_start_by_route[raptor_route.as_usize()];
        if *slot == NOT_QUEUED {
            self.buffers.queued_routes.push(raptor_route);
            *slot = position;
        } else {
            *slot = (*slot).min(position);
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
                    let trip = BundleTripIndex(route.first_trip_index + route_label.trip_offset);
                    let ride_cost = self.model.ride_cost(
                        raptor_route,
                        trip,
                        route_label.boarding_position,
                        position,
                        MinutesSinceBundleStart(route_label.departure),
                        MinutesSinceBundleStart(arrival),
                        self.previous_ride(route_label),
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
                        days: route_label.days,
                        origin: CostLabelOrigin::TrainRide {
                            ride,
                            alighting_position: position,
                        },
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
                            alighting_position: position,
                            days: route_label.days,
                        });
                    }
                    self.insert(round, label, None);
                }
            }
            // Only stations with labels from the previous round can board.
            if pattern_stop.boarding_allowed()
                && self.buffers.boardable_stations.contains(station.as_usize())
            {
                self.board_route(
                    round,
                    raptor_route,
                    &route,
                    position,
                    &mut route_labels,
                    ride_cost_depends_on_time,
                );
            }
        }
        self.buffers.route_labels = route_labels;
    }

    /// Lets each label the previous round created at a stop of a route board the earliest
    /// trip it can reach there, and adds the rides to the route's labels unless one there
    /// covers them.
    fn board_route(
        &mut self,
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
            let trip_index = BundleTripIndex(route.first_trip_index + trip.trip_offset);
            // The ride counts as a travel day on the day it is boarded, and a journey that
            // would use more travel days than any pass gives is no option.
            let days = label.days.after_ride(self.model.travel_day_of(
                raptor_route,
                trip_index,
                trip.boarding_position,
                MinutesSinceBundleStart(trip.departure),
            ));
            if days.count > MAXIMUM_TRAVEL_DAYS {
                continue;
            }
            let route_label = RouteLabel {
                trip_offset: trip.trip_offset,
                service_day: trip.service_day,
                boarding_position: trip.boarding_position,
                departure: trip.departure,
                parent,
                cost_before_ride: label.cost,
                is_first_ride: matches!(label.origin, CostLabelOrigin::DepartureCandidate { .. }),
                days,
            };
            if self.repeats_boarding(raptor_route, trip_index, &route_label, round) {
                continue;
            }
            if route_labels.iter().any(|other| {
                other.covers(&route_label, ride_cost_depends_on_time, || {
                    self.board_extension_cost(
                        raptor_route,
                        other.boarding_position,
                        route_label.boarding_position,
                    )
                })
            }) {
                continue;
            }
            route_labels.retain(|other| {
                !route_label.covers(other, ride_cost_depends_on_time, || {
                    self.board_extension_cost(
                        raptor_route,
                        route_label.boarding_position,
                        other.boarding_position,
                    )
                })
            });
            route_labels.push(route_label);
        }
    }

    /// Walks footpaths from the train arrivals of this round. Footpaths are one hop, so
    /// arrivals on foot are not walked on. A walk between stations adds no cost of its own
    /// until it ends at an arrival candidate.
    fn relax_footpaths(&mut self, round: usize) {
        let mut arrivals = std::mem::take(&mut self.buffers.ride_arrivals);
        // Walking on from an arrival that another at the same station beats on time, cost and
        // travel days leads nowhere new, so in order of arrival an arrival walks unless the
        // cheapest one before it covers it.
        arrivals.sort_unstable_by_key(|ride_arrival| {
            (
                ride_arrival.station,
                ride_arrival.arrival,
                ride_arrival.cost,
            )
        });
        let mut cheapest_before: Option<(BundleStationIndex, i32, TravelDays)> = None;
        for ride_arrival in &arrivals {
            match cheapest_before {
                Some((station, cost, days))
                    if station == ride_arrival.station
                        && cost <= ride_arrival.cost
                        && days.covers(ride_arrival.days) =>
                {
                    continue;
                }
                _ => {
                    cheapest_before =
                        Some((ride_arrival.station, ride_arrival.cost, ride_arrival.days));
                }
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
                    days: ride_arrival.days,
                    origin: CostLabelOrigin::TrainRideAndFootpath {
                        ride: ride_arrival.ride,
                        alighting_position: ride_arrival.alighting_position,
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
                    journeys.push(CostedJourney {
                        journey,
                        cost,
                        days: label.days.count,
                    });
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
                CostLabelOrigin::TrainRide {
                    ride,
                    alighting_position: _,
                } => (ride, label.station),
                CostLabelOrigin::TrainRideAndFootpath {
                    ride,
                    alighting_position: _,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A boarding of a trip that leaves at 480 and was reached for 100 cost units.
    fn route_label(boarding_position: u16, cost_before_ride: i32) -> RouteLabel {
        RouteLabel {
            trip_offset: 0,
            service_day: 0,
            boarding_position,
            departure: 480,
            parent: 0,
            cost_before_ride,
            is_first_ride: true,
            days: TravelDays::NONE,
        }
    }

    #[test]
    fn an_earlier_boarding_covers_only_what_its_price_pays_for() {
        let earlier = route_label(0, 100);
        let later = route_label(2, 100);
        // Boarding two stops earlier costs the price of the two stops on top, so with the same
        // cost before the ride the earlier boarding no longer covers the later one.
        assert!(!earlier.covers(&later, false, || 40));
        // A cheaper boarding covers when its saving pays for the stops in between.
        let cheaper = route_label(0, 60);
        assert!(cheaper.covers(&later, false, || 40));
        // Without prices the old rule holds.
        assert!(earlier.covers(&later, false, || 0));
    }

    #[test]
    fn a_boarding_remembered_from_earlier_covers_only_with_its_price() {
        let earlier = BoardedBefore {
            position: 1,
            cost: 100,
            is_first_ride: true,
            round: 2,
            days: TravelDays::NONE,
        };
        let later = BoardedBefore {
            position: 3,
            cost: 100,
            is_first_ride: true,
            round: 2,
            days: TravelDays::NONE,
        };
        // Boarding the same trip two stops earlier costs the price of the stops in between.
        assert!(!earlier.covers(later, || 40));
        assert!(earlier.covers(later, || 0));
        let cheaper = BoardedBefore {
            position: 1,
            cost: 60,
            is_first_ride: true,
            round: 2,
            days: TravelDays::NONE,
        };
        assert!(cheaper.covers(later, || 40));
    }

    #[test]
    fn fewer_travel_days_or_the_same_last_day_cover() {
        let first_day = TravelDays::NONE.after_ride(Some(4));
        let second_day = TravelDays::NONE.after_ride(Some(5));
        // A ride on the day already used adds no day, and a ride that counts none neither.
        assert_eq!(first_day.after_ride(Some(4)), first_day);
        assert_eq!(first_day.after_ride(None), first_day);
        assert_eq!(first_day.after_ride(Some(5)).count, 2);
        // One day each, but a ride on day 5 next would add a day to the first only.
        assert!(!first_day.covers(second_day));
        assert!(!second_day.covers(first_day));
        assert!(first_day.covers(first_day));
        // No day at all covers any day.
        assert!(TravelDays::NONE.covers(second_day));
        assert!(!second_day.covers(TravelDays::NONE));
    }

    #[test]
    fn a_boarding_on_more_travel_days_covers_nothing() {
        let mut fewer_days = route_label(0, 100);
        let mut more_days = route_label(0, 100);
        fewer_days.days = TravelDays::NONE.after_ride(Some(1));
        more_days.days = fewer_days.days.after_ride(Some(2));
        // The same trip from the same stop at the same cost, but one journey already used
        // another travel day.
        assert!(fewer_days.covers(&more_days, false, || 0));
        assert!(!more_days.covers(&fewer_days, false, || 0));
        let earlier = BoardedBefore {
            position: 0,
            cost: 100,
            is_first_ride: true,
            round: 1,
            days: more_days.days,
        };
        let later = BoardedBefore {
            days: fewer_days.days,
            ..earlier
        };
        assert!(!earlier.covers(later, || 0));
        assert!(later.covers(earlier, || 0));
    }
}
