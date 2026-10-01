//! Compares the router with a connection scan on random small networks.
//!
//! The connection scan looks at every consecutive pair of stops of every trip in order of
//! departure, which is simple enough to trust. Both must find the same earliest arrival. A second
//! test checks that searching a departure window in two parts and merging the results gives the
//! same journeys as one search over the whole window.

use std::num::NonZeroU8;

use btt_model::GeoCoordinates;
use btt_router::{
    AllowEveryRide, CandidateStation, CostSearchBuffers, JourneyCostModel, MinutesSinceBundleStart,
    ParetoJourneySet, RaptorLegQuery, RaptorSearchBuffers, SearchIndex, search_leg_journeys,
    search_leg_journeys_by_cost,
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
        _: MinutesSinceBundleStart,
        _: MinutesSinceBundleStart,
        is_first_ride: bool,
    ) -> i32 {
        if is_first_ride { 0 } else { TRANSFER_PENALTY }
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

/// Every journey as departure, arrival, number of trains and cost, with no two alike and none
/// that another beats on all four.
fn pareto_front(mut journeys: Vec<(u32, u32, usize, i32)>) -> Vec<(u32, u32, usize, i32)> {
    journeys.sort_unstable();
    journeys.dedup();
    let dominated = |journey: &(u32, u32, usize, i32)| {
        journeys.iter().any(|other| {
            other != journey
                && other.0 >= journey.0
                && other.1 <= journey.1
                && other.2 <= journey.2
                && other.3 <= journey.3
        })
    };
    journeys
        .iter()
        .copied()
        .filter(|journey| !dominated(journey))
        .collect()
}

/// Lists every journey with at most `MAXIMUM_COSTED_RIDES` trains that leaves the place within
/// the window, as departure, arrival, number of trains and cost, and keeps the Pareto front.
fn brute_force_costs(
    trips: &[RandomTrip],
    footpaths: &[(u32, u32, u16)],
    origins: &[(u32, u16)],
    target: u32,
    window: (u32, u32),
    model: &WindowCostModel,
) -> Vec<(u32, u32, usize, i32)> {
    /// The state of a journey on board a trip.
    struct OnBoard {
        trip: usize,
        position: usize,
        departure: u32,
        cost_before_ride: i32,
        rides: usize,
        is_first_ride: bool,
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
                    stack.push(OnBoard {
                        trip,
                        position,
                        departure: leave,
                        cost_before_ride: model.access_cost(&departure_candidate),
                        rides: 1,
                        is_first_ride: true,
                    });
                }
            }
        }
    }
    let ride_cost = |is_first_ride| if is_first_ride { 0 } else { TRANSFER_PENALTY };
    let boardings_after = |station: u32, ready: u32, state: &OnBoard, cost: i32| {
        let mut next = Vec::new();
        if state.rides == MAXIMUM_COSTED_RIDES {
            return next;
        }
        for (trip, trip_stops) in stops.iter().enumerate() {
            for (position, &(stop_station, _, departure)) in trip_stops.iter().enumerate() {
                if stop_station == station && u32::from(departure) >= ready {
                    next.push(OnBoard {
                        trip,
                        position,
                        departure: state.departure,
                        cost_before_ride: cost,
                        rides: state.rides + 1,
                        is_first_ride: false,
                    });
                }
            }
        }
        next
    };
    while let Some(state) = stack.pop() {
        let cost = state.cost_before_ride + ride_cost(state.is_first_ride);
        for &(station, arrival, _) in &stops[state.trip][state.position + 1..] {
            let arrival = u32::from(arrival);
            let total = |arrival: u32, footpath: Option<u16>| {
                model.elapsed_cost(
                    MinutesSinceBundleStart(state.departure),
                    MinutesSinceBundleStart(arrival),
                ) + cost
                    + model.egress_cost(&arrival_candidate, footpath)
            };
            if station == target {
                found.push((state.departure, arrival, state.rides, total(arrival, None)));
            }
            let ready = arrival + u32::from(MINIMUM_TRANSFER_MINUTES);
            stack.extend(boardings_after(station, ready, &state, cost));
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
                        total(walked, Some(minutes)),
                    ));
                }
                stack.extend(boardings_after(to, walked, &state, cost));
            }
        }
    }
    pareto_front(found)
}

/// Searches with the cost model and returns the Pareto front like `brute_force_costs`.
fn cost_router(
    trips: &[RandomTrip],
    footpaths: &[(u32, u32, u16)],
    origins: &[(u32, u16)],
    target: u32,
    window: (u32, u32),
    model: &WindowCostModel,
) -> Vec<(u32, u32, usize, i32)> {
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
        let expected =
            brute_force_costs(&trips, &footpaths, &origins, target, (start, 1440), &model);
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
        let expected =
            brute_force_costs(&trips, &footpaths, &origins, target, (start, 1440), &model);
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
