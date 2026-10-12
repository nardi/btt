//! Compares the router with a connection scan on random small networks.
//!
//! The connection scan looks at every consecutive pair of stops of every trip in order of
//! departure, which is simple enough to trust. Both must find the same earliest arrival. A second
//! test checks that searching a departure window in two parts and merging the results gives the
//! same journeys as one search over the whole window.

use std::num::NonZeroU8;

use btt_model::GeoCoordinates;
use btt_router::{
    AllowEveryRide, CandidateStation, CostSearchBuffers, JourneyCostModel, MAXIMUM_TRAVEL_DAYS,
    MinutesSinceBundleStart, ParetoJourneySet, PreviousRide, RaptorLegQuery, RaptorSearchBuffers,
    SearchIndex, search_leg_journeys, search_leg_journeys_by_cost,
};
use btt_timetable::{
    BundleBuilder, BundleStationIndex, BundleTripIndex, LoadedTimetableBundle, RaptorRouteIndex,
    ServiceDayBitset, StationInput, TrainLabelInput, TripInput, TripStopInput,
};
use chrono::NaiveDate;
use proptest::prelude::*;

const STATION_COUNT: u32 = 5;
const MINIMUM_TRANSFER_MINUTES: u16 = 5;

/// A random trip as stations and times.
#[derive(Debug, Clone)]
struct RandomTrip {
    stations: Vec<u32>,
    first_departure: u16,
    segment_minutes: Vec<u16>,
    dwell_minutes: Vec<u16>,
}

impl RandomTrip {
    /// Returns the stops as station, arrival and departure.
    fn stops(&self) -> Vec<(u32, u16, u16)> {
        let mut time = self.first_departure;
        let mut stops = vec![(self.stations[0], time, time)];
        for index in 1..self.stations.len() {
            let arrival = time + self.segment_minutes[index - 1];
            let departure = arrival + self.dwell_minutes[index - 1];
            stops.push((self.stations[index], arrival, departure));
            time = departure;
        }
        stops
    }
}

fn random_trip() -> impl Strategy<Value = RandomTrip> {
    (2..=4usize)
        .prop_flat_map(|stop_count| {
            (
                Just((0..STATION_COUNT).collect::<Vec<_>>())
                    .prop_shuffle()
                    .prop_map(move |stations| stations[..stop_count].to_vec()),
                0..300u16,
                proptest::collection::vec(5..60u16, stop_count - 1),
                proptest::collection::vec(0..5u16, stop_count - 1),
            )
        })
        .prop_map(
            |(stations, first_departure, segment_minutes, dwell_minutes)| RandomTrip {
                stations,
                first_departure,
                segment_minutes,
                dwell_minutes,
            },
        )
}

fn random_footpath() -> impl Strategy<Value = (u32, u32, u16)> {
    (0..STATION_COUNT, 0..STATION_COUNT, 1..20u16)
        .prop_filter("distinct ends", |(from, to, _)| from != to)
}

/// Earliest arrival at `target` when leaving the place at `start`, by connection scan. Each
/// origin is a station with the minutes it takes to walk there.
fn connection_scan(
    trips: &[RandomTrip],
    footpaths: &[(u32, u32, u16)],
    origins: &[(u32, u16)],
    target: u32,
    start: u32,
) -> Option<u32> {
    let mut connections = Vec::new();
    for (trip_index, trip) in trips.iter().enumerate() {
        for pair in trip.stops().windows(2) {
            connections.push((
                u32::from(pair[0].2),
                u32::from(pair[1].1),
                pair[0].0,
                pair[1].0,
                trip_index,
            ));
        }
    }
    connections.sort_by_key(|connection| connection.0);
    let stations = STATION_COUNT as usize;
    let mut arrival_by_walking = vec![u32::MAX; stations];
    let mut arrival_by_train = vec![u32::MAX; stations];
    let mut trip_boarded = vec![false; trips.len()];
    for &(origin, access) in origins {
        let slot = &mut arrival_by_walking[origin as usize];
        *slot = (*slot).min(start + u32::from(access));
    }
    for (departure, arrival, from, to, trip_index) in connections {
        let ready = arrival_by_walking[from as usize].min(
            arrival_by_train[from as usize].saturating_add(u32::from(MINIMUM_TRANSFER_MINUTES)),
        );
        if trip_boarded[trip_index] || ready <= departure {
            trip_boarded[trip_index] = true;
            if arrival < arrival_by_train[to as usize] {
                arrival_by_train[to as usize] = arrival;
                for &(footpath_from, footpath_to, minutes) in footpaths {
                    if footpath_from == to {
                        let walked = arrival + u32::from(minutes);
                        let slot = &mut arrival_by_walking[footpath_to as usize];
                        *slot = (*slot).min(walked);
                    }
                }
            }
        }
    }
    let best = arrival_by_train[target as usize].min(arrival_by_walking[target as usize]);
    (best != u32::MAX).then_some(best)
}

/// Earliest arrival at `target` when leaving the place at `start` or later, by the router.
fn router(
    trips: &[RandomTrip],
    footpaths: &[(u32, u32, u16)],
    origins: &[(u32, u16)],
    target: u32,
    start: u32,
) -> Option<u32> {
    router_journeys(trips, footpaths, origins, target, &[(start, 1440)])
        .into_iter()
        .map(|(_, arrival, _)| arrival)
        .min()
}

/// Builds a bundle with the trips on the first day and the footpaths.
fn build_bundle(trips: &[RandomTrip], footpaths: &[(u32, u32, u16)]) -> LoadedTimetableBundle {
    let mut builder =
        BundleBuilder::new(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(), 2, 1).unwrap();
    for index in 0..STATION_COUNT {
        builder
            .add_station(StationInput {
                name: format!("S{index}"),
                coordinates: GeoCoordinates::from_degrees(50.0, 8.0).unwrap(),
                time_zone: "UTC".to_owned(),
                trainline_station_id: None,
            })
            .unwrap();
    }
    for &(from, to, minutes) in footpaths {
        builder.add_footpath(BundleStationIndex(from), BundleStationIndex(to), minutes);
    }
    let mut first_day = ServiceDayBitset::default();
    first_day.insert(0);
    for trip in trips {
        builder.add_trip(TripInput {
            stops: trip
                .stops()
                .into_iter()
                .map(|(station, arrival, departure)| TripStopInput {
                    station: BundleStationIndex(station),
                    arrival,
                    departure,
                    boarding_allowed: true,
                    alighting_allowed: true,
                })
                .collect(),
            service_days: first_day,
            train_label: TrainLabelInput {
                category: String::new(),
                number: String::new(),
                agency: String::new(),
                offers_berths: false,
            },
        });
    }
    LoadedTimetableBundle::from_bundle(&builder.build().unwrap()).unwrap()
}

/// Returns the candidates of the departure place and the arrival place.
fn candidates(
    origins: &[(u32, u16)],
    target: u32,
) -> (Vec<CandidateStation>, [CandidateStation; 1]) {
    let departure_candidates = origins
        .iter()
        .map(|&(origin, access)| CandidateStation {
            station: BundleStationIndex(origin),
            time_to_place_minutes: access,
        })
        .collect();
    let arrival_candidates = [CandidateStation {
        station: BundleStationIndex(target),
        time_to_place_minutes: 0,
    }];
    (departure_candidates, arrival_candidates)
}

/// Searches each departure window, given as start and end, merges the results and returns every
/// journey as its departure, arrival and number of trains.
fn router_journeys(
    trips: &[RandomTrip],
    footpaths: &[(u32, u32, u16)],
    origins: &[(u32, u16)],
    target: u32,
    windows: &[(u32, u32)],
) -> Vec<(u32, u32, usize)> {
    let bundle = build_bundle(trips, footpaths);
    let departure_candidates: Vec<_> = origins
        .iter()
        .map(|&(origin, access)| CandidateStation {
            station: BundleStationIndex(origin),
            time_to_place_minutes: access,
        })
        .collect();
    let arrival_candidates = [CandidateStation {
        station: BundleStationIndex(target),
        time_to_place_minutes: 0,
    }];
    let index = SearchIndex::new(&bundle);
    let found = windows.iter().map(|&(window_start, window_end)| {
        let query = RaptorLegQuery {
            departure_candidates: &departure_candidates,
            arrival_candidates: &arrival_candidates,
            departure_window: MinutesSinceBundleStart(window_start)
                ..MinutesSinceBundleStart(window_end),
            minimum_transfer_minutes: MINIMUM_TRANSFER_MINUTES,
            maximum_train_rides: NonZeroU8::new(u8::try_from(trips.len().max(1)).unwrap()).unwrap(),
        };
        search_leg_journeys(
            &bundle,
            &index,
            &query,
            &AllowEveryRide,
            &mut RaptorSearchBuffers::new(),
            || false,
        )
    });
    ParetoJourneySet::merge(found)
        .journeys
        .iter()
        .map(|journey| (journey.departure.0, journey.arrival.0, journey.ride_count()))
        .collect()
}

/// Cost of a change of trains, and of a walk between stations at the end, in the test model.
const TRANSFER_PENALTY: i32 = 30;

/// Largest number of trains in the cost tests, which keeps the brute force small.
const MAXIMUM_COSTED_RIDES: usize = 3;

/// A cost model with the shape of the planner's, without the night terms. Ways to and from
/// stations count one and a half times, changes add a penalty, and the arrival window costs a
/// minute per minute before it and two per minute after it.
struct WindowCostModel {
    window_start: u32,
    window_end: u32,
}

impl JourneyCostModel for WindowCostModel {
    fn access_cost(&self, candidate: &CandidateStation) -> i32 {
        i32::from(candidate.time_to_place_minutes) / 2
    }

    fn ride_cost(
        &self,
        _: RaptorRouteIndex,
        _: BundleTripIndex,
        _: u16,
        _: u16,
        _: MinutesSinceBundleStart,
        _: MinutesSinceBundleStart,
        previous: Option<PreviousRide>,
    ) -> i32 {
        if previous.is_none() {
            0
        } else {
            TRANSFER_PENALTY
        }
    }

    fn egress_cost(
        &self,
        candidate: &CandidateStation,
        final_footpath_minutes: Option<u16>,
    ) -> i32 {
        i32::from(candidate.time_to_place_minutes) / 2
            + final_footpath_minutes.map_or(0, |minutes| TRANSFER_PENALTY + i32::from(minutes) / 2)
    }

    fn elapsed_cost(
        &self,
        departure: MinutesSinceBundleStart,
        arrival: MinutesSinceBundleStart,
    ) -> i32 {
        let arrival = arrival.0;
        let waited_until = arrival.max(self.window_start);
        let late = arrival.saturating_sub(self.window_end);
        i32::try_from(waited_until - departure.0 + 2 * late).unwrap()
    }

    fn remaining_cost_lower_bound(
        &self,
        departure: MinutesSinceBundleStart,
        _: MinutesSinceBundleStart,
        earliest_arrival: MinutesSinceBundleStart,
        _: Option<u32>,
    ) -> i32 {
        self.elapsed_cost(departure, earliest_arrival)
    }
    fn ride_cost_depends_on_time(&self, _: RaptorRouteIndex) -> bool {
        false
    }
}

/// A journey as departure, arrival, number of trains, number of travel days and cost.
type CostedTuple = (u32, u32, usize, u8, i32);

/// Every journey with no two alike and none that another beats on all five criteria.
fn pareto_front(mut journeys: Vec<CostedTuple>) -> Vec<CostedTuple> {
    journeys.sort_unstable();
    journeys.dedup();
    let dominated = |journey: &CostedTuple| {
        journeys.iter().any(|other| {
            other != journey
                && other.0 >= journey.0
                && other.1 <= journey.1
                && other.2 <= journey.2
                && other.3 <= journey.3
                && other.4 <= journey.4
        })
    };
    journeys
        .iter()
        .copied()
        .filter(|journey| !dominated(journey))
        .collect()
}

/// Lists every journey with at most `MAXIMUM_COSTED_RIDES` trains that leaves the place within
/// the window, as departure, arrival, number of trains, travel days and cost, and keeps the
/// Pareto front. Every ride asks the model, so prices that depend on the stops of the ride
/// enter the cost, and the travel days are the distinct days the model counts rides on.
#[allow(clippy::too_many_lines)]
fn brute_force_costs<M: JourneyCostModel>(
    trips: &[RandomTrip],
    footpaths: &[(u32, u32, u16)],
    origins: &[(u32, u16)],
    target: u32,
    window: (u32, u32),
    model: &M,
    routes_of_trips: &[RaptorRouteIndex],
) -> Vec<CostedTuple> {
    /// The state of a journey on board a trip.
    struct OnBoard {
        trip: usize,
        route: RaptorRouteIndex,
        boarding_position: usize,
        departure: u32,
        ride_departure: u32,
        cost_before_ride: i32,
        rides: usize,
        previous: Option<PreviousRide>,
        /// The days the model counted the rides so far on, this one included.
        days: Vec<u16>,
    }

    /// Adds the day a ride counts on to the days of the rides before it.
    fn with_day(mut days: Vec<u16>, day: Option<u16>) -> Vec<u16> {
        if let Some(day) = day
            && !days.contains(&day)
        {
            days.push(day);
        }
        days
    }

    let stops: Vec<Vec<(u32, u16, u16)>> = trips.iter().map(RandomTrip::stops).collect();
    let (_, arrival_candidates) = candidates(origins, target);
    let arrival_candidate = arrival_candidates[0];
    let mut found = Vec::new();
    let mut stack = Vec::new();
    for &(origin, access) in origins {
        let departure_candidate = CandidateStation {
            station: BundleStationIndex(origin),
            time_to_place_minutes: access,
        };
        for (trip, trip_stops) in stops.iter().enumerate() {
            for (position, &(station, _, departure)) in trip_stops.iter().enumerate() {
                let Some(leave) = u32::from(departure).checked_sub(u32::from(access)) else {
                    continue;
                };
                if station == origin && leave >= window.0 && leave < window.1 {
                    let day = model.travel_day_of(
                        routes_of_trips[trip],
                        BundleTripIndex(u32::try_from(trip).unwrap_or(u32::MAX)),
                        u16::try_from(position).unwrap_or(u16::MAX),
                        MinutesSinceBundleStart(u32::from(departure)),
                    );
                    stack.push(OnBoard {
                        trip,
                        route: routes_of_trips[trip],
                        boarding_position: position,
                        departure: leave,
                        ride_departure: u32::from(departure),
                        cost_before_ride: model.access_cost(&departure_candidate),
                        rides: 1,
                        previous: None,
                        days: with_day(Vec::new(), day),
                    });
                }
            }
        }
    }
    let boardings_after = |station: u32,
                           ready: u32,
                           state: &OnBoard,
                           cost: i32,
                           previous: PreviousRide|
     -> Vec<OnBoard> {
        let mut next = Vec::new();
        if state.rides == MAXIMUM_COSTED_RIDES {
            return next;
        }
        for (trip, trip_stops) in stops.iter().enumerate() {
            for (position, &(stop_station, _, departure)) in trip_stops.iter().enumerate() {
                if stop_station == station && u32::from(departure) >= ready {
                    let day = model.travel_day_of(
                        routes_of_trips[trip],
                        BundleTripIndex(u32::try_from(trip).unwrap_or(u32::MAX)),
                        u16::try_from(position).unwrap_or(u16::MAX),
                        MinutesSinceBundleStart(u32::from(departure)),
                    );
                    next.push(OnBoard {
                        trip,
                        route: routes_of_trips[trip],
                        boarding_position: position,
                        departure: state.departure,
                        ride_departure: u32::from(departure),
                        cost_before_ride: cost,
                        rides: state.rides + 1,
                        previous: Some(previous),
                        days: with_day(state.days.clone(), day),
                    });
                }
            }
        }
        next
    };
    while let Some(state) = stack.pop() {
        // A journey on more travel days than the search keeps is no option.
        let days = u8::try_from(state.days.len()).unwrap_or(u8::MAX);
        if days > MAXIMUM_TRAVEL_DAYS {
            continue;
        }
        for (offset, &(station, arrival, _)) in stops[state.trip][state.boarding_position + 1..]
            .iter()
            .enumerate()
        {
            let alighting_position = state.boarding_position + 1 + offset;
            let arrival = u32::from(arrival);
            let ride_cost = model.ride_cost(
                state.route,
                BundleTripIndex(u32::try_from(state.trip).unwrap_or(u32::MAX)),
                u16::try_from(state.boarding_position).unwrap_or(u16::MAX),
                u16::try_from(alighting_position).unwrap_or(u16::MAX),
                MinutesSinceBundleStart(state.ride_departure),
                MinutesSinceBundleStart(arrival),
                state.previous,
            );
            let cost = state.cost_before_ride + ride_cost;
            let ride = PreviousRide {
                raptor_route: state.route,
                boarding_position: u16::try_from(state.boarding_position).unwrap_or(u16::MAX),
                alighting_position: u16::try_from(alighting_position).unwrap_or(u16::MAX),
            };
            let total = |arrival: u32, footpath: Option<u16>| {
                model.elapsed_cost(
                    MinutesSinceBundleStart(state.departure),
                    MinutesSinceBundleStart(arrival),
                ) + cost
                    + model.egress_cost(&arrival_candidate, footpath)
            };
            if station == target {
                found.push((
                    state.departure,
                    arrival,
                    state.rides,
                    days,
                    total(arrival, None),
                ));
            }
            let ready = arrival + u32::from(MINIMUM_TRANSFER_MINUTES);
            stack.extend(boardings_after(station, ready, &state, cost, ride));
            for &(from, to, minutes) in footpaths {
                if from != station {
                    continue;
                }
                let walked = arrival + u32::from(minutes);
                if to == target {
                    found.push((
                        state.departure,
                        walked,
                        state.rides,
                        days,
                        total(walked, Some(minutes)),
                    ));
                }
                stack.extend(boardings_after(to, walked, &state, cost, ride));
            }
        }
    }
    pareto_front(found)
}

/// Returns the route of each trip, found by matching the trip's stops in the bundle. Trips of
/// one stop pattern share a route, and the times tell the trips of one route apart.
fn routes_of_trips(bundle: &LoadedTimetableBundle, trips: &[RandomTrip]) -> Vec<RaptorRouteIndex> {
    trips
        .iter()
        .map(|trip| {
            let wanted = trip.stops();
            for route_index in 0..bundle.route_count() {
                let route_index = RaptorRouteIndex(u32::try_from(route_index).unwrap_or(u32::MAX));
                let route = bundle.route(route_index);
                if usize::from(route.stop_count) != wanted.len() {
                    continue;
                }
                for trip_offset in 0..route.trip_count {
                    let matches = wanted.iter().enumerate().all(
                        |(position, &(station, arrival, departure))| {
                            let position = u16::try_from(position).unwrap_or(u16::MAX);
                            let stop = bundle.pattern_stop(&route, position);
                            let event = bundle.stop_event(&route, trip_offset, position);
                            stop.station().0 == station
                                && event.arrival.0 == arrival
                                && event.departure.0 == departure
                        },
                    );
                    if matches {
                        return route_index;
                    }
                }
            }
            panic!("a trip of the test is missing from the bundle");
        })
        .collect()
}

/// A cost model whose rides cost a price that grows with the number of stops between boarding
/// and alighting and falls per stop on longer rides, which is what makes boarding earlier cost
/// more. One operator runs every route, so a ride after another continues its ticket run and
/// pays the extra over the ride before, the way the planner's search prices a run.
struct PricedCostModel {
    window_start: u32,
    window_end: u32,
}

impl PricedCostModel {
    /// The price of a number of stops, in the model's cost units.
    fn price(units: u16) -> i32 {
        let kilometres = f64::from(units) * 15.0;
        let distance = ((kilometres + 20.0) / 120.0).ln();
        let euro = (1.2 + 0.7 * distance).exp();
        // Prices stay in a small range, so the cast loses nothing.
        #[expect(clippy::cast_possible_truncation)]
        let cents = (euro * 100.0).round() as i32;
        cents
    }
}

impl JourneyCostModel for PricedCostModel {
    fn access_cost(&self, candidate: &CandidateStation) -> i32 {
        i32::from(candidate.time_to_place_minutes) / 2
    }

    fn ride_cost(
        &self,
        _: RaptorRouteIndex,
        _: BundleTripIndex,
        boarding_position: u16,
        alighting_position: u16,
        _: MinutesSinceBundleStart,
        _: MinutesSinceBundleStart,
        previous: Option<PreviousRide>,
    ) -> i32 {
        let change = if previous.is_some() {
            TRANSFER_PENALTY
        } else {
            0
        };
        let units = alighting_position.saturating_sub(boarding_position);
        // One operator runs everything, so every following ride continues the run.
        let price = match previous {
            Some(previous) => {
                let previous_units = previous
                    .alighting_position
                    .saturating_sub(previous.boarding_position);
                Self::price(previous_units + units) - Self::price(previous_units)
            }
            None => Self::price(units),
        };
        change + price
    }

    fn ride_price(
        &self,
        _: RaptorRouteIndex,
        boarding_position: u16,
        alighting_position: u16,
    ) -> i32 {
        Self::price(alighting_position.saturating_sub(boarding_position))
    }

    fn egress_cost(
        &self,
        candidate: &CandidateStation,
        final_footpath_minutes: Option<u16>,
    ) -> i32 {
        i32::from(candidate.time_to_place_minutes) / 2
            + final_footpath_minutes.map_or(0, |minutes| TRANSFER_PENALTY + i32::from(minutes) / 2)
    }

    fn elapsed_cost(
        &self,
        departure: MinutesSinceBundleStart,
        arrival: MinutesSinceBundleStart,
    ) -> i32 {
        let arrival = arrival.0;
        let waited_until = arrival.max(self.window_start);
        let late = arrival.saturating_sub(self.window_end);
        i32::try_from(waited_until - departure.0 + 2 * late).unwrap()
    }

    fn remaining_cost_lower_bound(
        &self,
        departure: MinutesSinceBundleStart,
        _: MinutesSinceBundleStart,
        earliest_arrival: MinutesSinceBundleStart,
        _: Option<u32>,
    ) -> i32 {
        self.elapsed_cost(departure, earliest_arrival)
    }
    fn ride_cost_depends_on_time(&self, _: RaptorRouteIndex) -> bool {
        false
    }
}

/// Length of a travel day in the pass model. It is short, so that the trips of a test, which
/// run within a few hours, span several days.
const PASS_DAY_MINUTES: u32 = 90;

/// Fixed price of a ride the pass takes, such as a reservation.
const PASS_RESERVATION: i32 = 50;

/// A cost model with the prices of `PricedCostModel` on the travel days of a pass. The pass
/// takes the routes with an even number, for a fixed price and one travel day per day it is
/// ridden on. A ride on another route is a ticket of its own and counts no travel day.
struct PassCostModel {
    priced: PricedCostModel,
}

impl PassCostModel {
    /// Returns whether the pass takes the trains of a route.
    fn takes(raptor_route: RaptorRouteIndex) -> bool {
        raptor_route.0.is_multiple_of(2)
    }
}

impl JourneyCostModel for PassCostModel {
    fn access_cost(&self, candidate: &CandidateStation) -> i32 {
        self.priced.access_cost(candidate)
    }

    fn ride_cost(
        &self,
        raptor_route: RaptorRouteIndex,
        _: BundleTripIndex,
        boarding_position: u16,
        alighting_position: u16,
        _: MinutesSinceBundleStart,
        _: MinutesSinceBundleStart,
        previous: Option<PreviousRide>,
    ) -> i32 {
        let change = if previous.is_some() {
            TRANSFER_PENALTY
        } else {
            0
        };
        if Self::takes(raptor_route) {
            change + PASS_RESERVATION
        } else {
            change + PricedCostModel::price(alighting_position.saturating_sub(boarding_position))
        }
    }

    fn ride_price(
        &self,
        raptor_route: RaptorRouteIndex,
        boarding_position: u16,
        alighting_position: u16,
    ) -> i32 {
        if Self::takes(raptor_route) {
            0
        } else {
            PricedCostModel::price(alighting_position.saturating_sub(boarding_position))
        }
    }

    fn travel_day_of(
        &self,
        raptor_route: RaptorRouteIndex,
        _: BundleTripIndex,
        _: u16,
        departure: MinutesSinceBundleStart,
    ) -> Option<u16> {
        Self::takes(raptor_route).then(|| u16::try_from(departure.0 / PASS_DAY_MINUTES).unwrap())
    }

    fn egress_cost(
        &self,
        candidate: &CandidateStation,
        final_footpath_minutes: Option<u16>,
    ) -> i32 {
        self.priced.egress_cost(candidate, final_footpath_minutes)
    }

    fn elapsed_cost(
        &self,
        departure: MinutesSinceBundleStart,
        arrival: MinutesSinceBundleStart,
    ) -> i32 {
        self.priced.elapsed_cost(departure, arrival)
    }

    fn remaining_cost_lower_bound(
        &self,
        departure: MinutesSinceBundleStart,
        moment: MinutesSinceBundleStart,
        earliest_arrival: MinutesSinceBundleStart,
        credit_minutes_cap: Option<u32>,
    ) -> i32 {
        self.priced.remaining_cost_lower_bound(
            departure,
            moment,
            earliest_arrival,
            credit_minutes_cap,
        )
    }

    fn ride_cost_depends_on_time(&self, _: RaptorRouteIndex) -> bool {
        false
    }
}

/// Searches with the cost model and returns the Pareto front like `brute_force_costs`.
fn cost_router<M: JourneyCostModel>(
    trips: &[RandomTrip],
    footpaths: &[(u32, u32, u16)],
    origins: &[(u32, u16)],
    target: u32,
    window: (u32, u32),
    model: &M,
) -> Vec<CostedTuple> {
    let bundle = build_bundle(trips, footpaths);
    let (departure_candidates, arrival_candidates) = candidates(origins, target);
    let query = RaptorLegQuery {
        departure_candidates: &departure_candidates,
        arrival_candidates: &arrival_candidates,
        departure_window: MinutesSinceBundleStart(window.0)..MinutesSinceBundleStart(window.1),
        minimum_transfer_minutes: MINIMUM_TRANSFER_MINUTES,
        maximum_train_rides: NonZeroU8::new(u8::try_from(MAXIMUM_COSTED_RIDES).unwrap()).unwrap(),
    };
    let found = search_leg_journeys_by_cost(
        &bundle,
        &SearchIndex::new(&bundle),
        &query,
        model,
        &mut CostSearchBuffers::new(),
        || false,
    );
    pareto_front(
        found
            .journeys
            .iter()
            .map(|costed| {
                (
                    costed.journey.departure.0,
                    costed.journey.arrival.0,
                    costed.journey.ride_count(),
                    costed.days,
                    costed.cost,
                )
            })
            .collect(),
    )
}

proptest! {
    #[test]
    fn cost_search_matches_brute_force(
        trips in proptest::collection::vec(random_trip(), 1..10),
        footpaths in proptest::collection::vec(random_footpath(), 0..4),
        start in 0..300u32,
        first_access in 0..30u16,
        second_access in 0..30u16,
        window_start in 0..500u32,
    ) {
        let origins = [(0, first_access), (1, second_access)];
        let model = WindowCostModel { window_start, window_end: window_start + 60 };
        let target = STATION_COUNT - 1;
        let routes = routes_of_trips(&build_bundle(&trips, &footpaths), &trips);
        let expected =
            brute_force_costs(&trips, &footpaths, &origins, target, (start, 1440), &model, &routes);
        let actual = cost_router(&trips, &footpaths, &origins, target, (start, 1440), &model);
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn cost_search_over_split_windows_gives_the_same_journeys(
        trips in proptest::collection::vec(random_trip(), 1..10),
        footpaths in proptest::collection::vec(random_footpath(), 0..4),
        start in 0..300u32,
        split in 0..600u32,
        first_access in 0..30u16,
        second_access in 0..30u16,
        window_start in 0..500u32,
    ) {
        let origins = [(0, first_access), (1, second_access)];
        let model = WindowCostModel { window_start, window_end: window_start + 60 };
        let target = STATION_COUNT - 1;
        let middle = start + split;
        let whole = cost_router(&trips, &footpaths, &origins, target, (start, 1440), &model);
        let mut parts = cost_router(&trips, &footpaths, &origins, target, (start, middle), &model);
        parts.extend(cost_router(&trips, &footpaths, &origins, target, (middle, 1440), &model));
        prop_assert_eq!(pareto_front(parts), whole);
    }

    #[test]
    fn cost_search_matches_brute_force_when_both_places_use_a_station(
        trips in proptest::collection::vec(random_trip(), 1..10),
        footpaths in proptest::collection::vec(random_footpath(), 0..4),
        start in 0..300u32,
        first_access in 0..30u16,
        second_access in 0..30u16,
        window_start in 0..500u32,
        target in 0..2u32,
    ) {
        let origins = [(0, first_access), (1, second_access)];
        let model = WindowCostModel { window_start, window_end: window_start + 60 };
        let routes = routes_of_trips(&build_bundle(&trips, &footpaths), &trips);
        let expected =
            brute_force_costs(&trips, &footpaths, &origins, target, (start, 1440), &model, &routes);
        let actual = cost_router(&trips, &footpaths, &origins, target, (start, 1440), &model);
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn priced_cost_search_matches_brute_force(
        trips in proptest::collection::vec(random_trip(), 1..10),
        footpaths in proptest::collection::vec(random_footpath(), 0..4),
        start in 0..300u32,
        first_access in 0..30u16,
        second_access in 0..30u16,
        window_start in 0..500u32,
    ) {
        // With prices that grow with the distance and a discount for rides that continue a run,
        // the shortcuts that let an earlier boarding stand for a later one have to count the
        // price of the stops in between. A journey the search drops is one the brute force
        // finds beaten, which is what the property asserts.
        let origins = [(0, first_access), (1, second_access)];
        let model = PricedCostModel { window_start, window_end: window_start + 60 };
        let target = STATION_COUNT - 1;
        let routes = routes_of_trips(&build_bundle(&trips, &footpaths), &trips);
        let expected =
            brute_force_costs(&trips, &footpaths, &origins, target, (start, 1440), &model, &routes);
        let actual = cost_router(&trips, &footpaths, &origins, target, (start, 1440), &model);
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn cost_search_on_pass_days_matches_brute_force(
        trips in proptest::collection::vec(random_trip(), 1..10),
        footpaths in proptest::collection::vec(random_footpath(), 0..4),
        start in 0..300u32,
        first_access in 0..30u16,
        second_access in 0..30u16,
        window_start in 0..500u32,
    ) {
        // Travel days are a criterion of their own, so a journey on fewer days stays beside a
        // cheaper one on more, and the shortcuts that drop a boarding or a label have to keep
        // it. The brute force counts the distinct days by listing them.
        let origins = [(0, first_access), (1, second_access)];
        let model = PassCostModel {
            priced: PricedCostModel { window_start, window_end: window_start + 60 },
        };
        let target = STATION_COUNT - 1;
        let routes = routes_of_trips(&build_bundle(&trips, &footpaths), &trips);
        let expected =
            brute_force_costs(&trips, &footpaths, &origins, target, (start, 1440), &model, &routes);
        let actual = cost_router(&trips, &footpaths, &origins, target, (start, 1440), &model);
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn router_matches_connection_scan(
        trips in proptest::collection::vec(random_trip(), 1..12),
        footpaths in proptest::collection::vec(random_footpath(), 0..4),
        start in 0..300u32,
        first_access in 0..30u16,
        second_access in 0..30u16,
    ) {
        let origins = [(0, first_access), (1, second_access)];
        let expected = connection_scan(&trips, &footpaths, &origins, STATION_COUNT - 1, start);
        let actual = router(&trips, &footpaths, &origins, STATION_COUNT - 1, start);
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn split_windows_give_the_same_journeys(
        trips in proptest::collection::vec(random_trip(), 1..12),
        footpaths in proptest::collection::vec(random_footpath(), 0..4),
        start in 0..300u32,
        split in 0..600u32,
        first_access in 0..30u16,
        second_access in 0..30u16,
    ) {
        let origins = [(0, first_access), (1, second_access)];
        let middle = start + split;
        let target = STATION_COUNT - 1;
        let whole = router_journeys(&trips, &footpaths, &origins, target, &[(start, 1440)]);
        let halves = router_journeys(
            &trips,
            &footpaths,
            &origins,
            target,
            &[(start, middle), (middle, 1440)],
        );
        prop_assert_eq!(halves, whole);
    }
}
