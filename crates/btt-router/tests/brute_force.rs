//! Compares the router with a connection scan on random small networks.
//!
//! The connection scan looks at every consecutive pair of stops of every trip in order of
//! departure, which is simple enough to trust. Both must find the same earliest arrival. A second
//! test checks that searching a departure window in two parts and merging the results gives the
//! same journeys as one search over the whole window.

use std::num::NonZeroU8;

use btt_model::GeoCoordinates;
use btt_router::{
    AllowEveryRide, CandidateStation, MinutesSinceBundleStart, ParetoJourneySet, RaptorLegQuery,
    RaptorSearchBuffers, SearchIndex, search_leg_journeys,
};
use btt_timetable::{
    BundleBuilder, BundleStationIndex, LoadedTimetableBundle, ServiceDayBitset, StationInput,
    TrainLabelInput, TripInput, TripStopInput,
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

/// Searches each departure window, given as start and end, merges the results and returns every
/// journey as its departure, arrival and number of trains.
fn router_journeys(
    trips: &[RandomTrip],
    footpaths: &[(u32, u32, u16)],
    origins: &[(u32, u16)],
    target: u32,
    windows: &[(u32, u32)],
) -> Vec<(u32, u32, usize)> {
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
    let bundle = LoadedTimetableBundle::from_bundle(&builder.build().unwrap()).unwrap();
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

proptest! {
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
