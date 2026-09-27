//! Compares the router with a connection scan on random small networks.
//!
//! The connection scan looks at every consecutive pair of stops of every trip in order of
//! departure, which is simple enough to trust. Both must find the same earliest arrival.

use std::num::NonZeroU8;

use btt_model::GeoCoordinates;
use btt_router::{
    BoardAnyTrip, CandidateStation, MinutesSinceBundleStart, RaptorLegQuery, RaptorSearchBuffers,
    search_leg_journeys,
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

/// Earliest arrival at `target` when leaving `origin` at `start`, by connection scan.
fn connection_scan(
    trips: &[RandomTrip],
    footpaths: &[(u32, u32, u16)],
    origin: u32,
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
    arrival_by_walking[origin as usize] = start;
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

/// Earliest arrival at `target` when leaving `origin` at `start` or later, by the router.
fn router(
    trips: &[RandomTrip],
    footpaths: &[(u32, u32, u16)],
    origin: u32,
    target: u32,
    start: u32,
) -> Option<u32> {
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
            },
        });
    }
    let bundle = LoadedTimetableBundle::from_bundle(&builder.build().unwrap()).unwrap();
    let departure_candidates = [CandidateStation {
        station: BundleStationIndex(origin),
        time_to_place_minutes: 0,
    }];
    let arrival_candidates = [CandidateStation {
        station: BundleStationIndex(target),
        time_to_place_minutes: 0,
    }];
    let query = RaptorLegQuery {
        departure_candidates: &departure_candidates,
        arrival_candidates: &arrival_candidates,
        departure_window: MinutesSinceBundleStart(start)..MinutesSinceBundleStart(1440),
        minimum_transfer_minutes: MINIMUM_TRANSFER_MINUTES,
        maximum_train_rides: NonZeroU8::new(u8::try_from(trips.len().max(1)).unwrap()).unwrap(),
    };
    let found = search_leg_journeys(
        &bundle,
        &query,
        &BoardAnyTrip,
        &mut RaptorSearchBuffers::new(),
        || false,
    );
    found.journeys.iter().map(|journey| journey.arrival.0).min()
}

proptest! {
    #[test]
    fn router_matches_connection_scan(
        trips in proptest::collection::vec(random_trip(), 1..12),
        footpaths in proptest::collection::vec(random_footpath(), 0..4),
        start in 0..300u32,
    ) {
        let expected = connection_scan(&trips, &footpaths, 0, STATION_COUNT - 1, start);
        let actual = router(&trips, &footpaths, 0, STATION_COUNT - 1, start);
        prop_assert_eq!(actual, expected);
    }
}
