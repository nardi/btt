//! Journey searches on small hand-made timetables.

use std::num::NonZeroU8;

use btt_model::GeoCoordinates;
use btt_router::{
    BoardAnyTrip, CandidateStation, MinutesSinceBundleStart, RaptorLegQuery, RaptorSearchBuffers,
    SearchIndex, search_leg_journeys,
};
use btt_timetable::{
    BundleBuilder, BundleStationIndex, LoadedTimetableBundle, ServiceDayBitset, StationInput,
    TrainLabelInput, TripInput, TripStopInput,
};
use chrono::NaiveDate;

/// A timetable under construction with named stations.
struct Network {
    builder: BundleBuilder,
}

impl Network {
    fn new() -> Self {
        Self {
            builder: BundleBuilder::new(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(), 7, 1)
                .unwrap(),
        }
    }

    fn station(&mut self, name: &str) -> BundleStationIndex {
        self.builder
            .add_station(StationInput {
                name: name.to_owned(),
                coordinates: GeoCoordinates::from_degrees(50.0, 8.0).unwrap(),
                time_zone: "UTC".to_owned(),
                trainline_station_id: None,
            })
            .unwrap()
    }

    /// Adds a trip that runs on day 0, from `(station, arrival, departure)` stops.
    fn trip(&mut self, number: &str, stops: &[(BundleStationIndex, u16, u16)]) {
        self.trip_with_flags(
            number,
            &stops
                .iter()
                .map(|&(station, arrival, departure)| (station, arrival, departure, true, true))
                .collect::<Vec<_>>(),
            &[0],
        );
    }

    fn trip_with_flags(
        &mut self,
        number: &str,
        stops: &[(BundleStationIndex, u16, u16, bool, bool)],
        days: &[u16],
    ) {
        let mut service_days = ServiceDayBitset::default();
        for &day in days {
            service_days.insert(day);
        }
        self.builder.add_trip(TripInput {
            stops: stops
                .iter()
                .map(
                    |&(station, arrival, departure, boarding_allowed, alighting_allowed)| {
                        TripStopInput {
                            station,
                            arrival,
                            departure,
                            boarding_allowed,
                            alighting_allowed,
                        }
                    },
                )
                .collect(),
            service_days,
            train_label: TrainLabelInput {
                category: "T".into(),
                number: number.into(),
                agency: String::new(),
            },
        });
    }

    fn build(self) -> LoadedTimetableBundle {
        LoadedTimetableBundle::from_bundle(&self.builder.build().unwrap()).unwrap()
    }
}

fn candidate(station: BundleStationIndex, minutes: u16) -> CandidateStation {
    CandidateStation {
        station,
        time_to_place_minutes: minutes,
    }
}

/// Searches from `from` to `to` leaving between `window_start` and `window_end`.
fn search(
    bundle: &LoadedTimetableBundle,
    from: &[CandidateStation],
    to: &[CandidateStation],
    window_start: u32,
    window_end: u32,
) -> Vec<(u32, u32, usize, Vec<String>)> {
    let query = RaptorLegQuery {
        departure_candidates: from,
        arrival_candidates: to,
        departure_window: MinutesSinceBundleStart(window_start)
            ..MinutesSinceBundleStart(window_end),
        minimum_transfer_minutes: 10,
        maximum_train_rides: NonZeroU8::new(5).unwrap(),
    };
    let result = search_leg_journeys(
        bundle,
        &SearchIndex::new(bundle),
        &query,
        &BoardAnyTrip,
        &mut RaptorSearchBuffers::new(),
        || false,
    );
    result
        .journeys
        .iter()
        .map(|journey| {
            let trains = journey
                .rides
                .iter()
                .map(|ride| bundle.trip_train_label(ride.trip).1.to_owned())
                .collect();
            (
                journey.departure.0,
                journey.arrival.0,
                journey.ride_count(),
                trains,
            )
        })
        .collect()
}

#[test]
fn finds_a_direct_train() {
    let mut network = Network::new();
    let a = network.station("A");
    let b = network.station("B");
    network.trip("1", &[(a, 600, 600), (b, 660, 660)]);
    let bundle = network.build();
    let journeys = search(&bundle, &[candidate(a, 5)], &[candidate(b, 5)], 500, 700);
    assert_eq!(journeys, [(595, 665, 1, vec!["1".to_owned()])]);
}

#[test]
fn respects_the_minimum_transfer_time() {
    let mut network = Network::new();
    let a = network.station("A");
    let b = network.station("B");
    let c = network.station("C");
    network.trip("1", &[(a, 600, 600), (b, 660, 660)]);
    // Leaves five minutes after the arrival, too soon for a ten-minute transfer.
    network.trip("2", &[(b, 665, 665), (c, 700, 700)]);
    network.trip("3", &[(b, 680, 680), (c, 720, 720)]);
    let bundle = network.build();
    let journeys = search(&bundle, &[candidate(a, 0)], &[candidate(c, 0)], 600, 601);
    assert_eq!(
        journeys,
        [(600, 720, 2, vec!["1".to_owned(), "3".to_owned()])]
    );
}

#[test]
fn walks_between_stations_after_a_ride() {
    let mut network = Network::new();
    let a = network.station("A");
    let north = network.station("North");
    let south = network.station("South");
    let c = network.station("C");
    network.builder.add_footpath(north, south, 15);
    network.trip("1", &[(a, 600, 600), (north, 660, 660)]);
    // The footpath takes the place of the minimum transfer time.
    network.trip("2", &[(south, 676, 676), (c, 700, 700)]);
    let bundle = network.build();
    let journeys = search(&bundle, &[candidate(a, 0)], &[candidate(c, 0)], 600, 601);
    assert_eq!(
        journeys,
        [(600, 700, 2, vec!["1".to_owned(), "2".to_owned()])]
    );
}

#[test]
fn takes_a_faster_train_that_overtakes() {
    let mut network = Network::new();
    let a = network.station("A");
    let b = network.station("B");
    network.trip("slow", &[(a, 600, 600), (b, 720, 720)]);
    network.trip("fast", &[(a, 610, 610), (b, 660, 660)]);
    let bundle = network.build();
    let journeys = search(&bundle, &[candidate(a, 0)], &[candidate(b, 0)], 590, 620);
    assert_eq!(journeys, [(610, 660, 1, vec!["fast".to_owned()])]);
}

#[test]
fn never_boards_where_boarding_is_not_allowed() {
    let mut network = Network::new();
    let a = network.station("A");
    let b = network.station("B");
    let c = network.station("C");
    network.trip_with_flags(
        "1",
        &[
            (a, 600, 600, true, true),
            (b, 630, 632, false, true),
            (c, 660, 660, true, true),
        ],
        &[0],
    );
    let bundle = network.build();
    assert!(search(&bundle, &[candidate(b, 0)], &[candidate(c, 0)], 600, 640).is_empty());
    assert_eq!(
        search(&bundle, &[candidate(a, 0)], &[candidate(b, 0)], 590, 610).len(),
        1
    );
}

#[test]
fn rides_trips_past_midnight_and_boards_them_on_the_next_day() {
    let mut network = Network::new();
    let a = network.station("A");
    let b = network.station("B");
    let c = network.station("C");
    // A night train with service day 0 that reaches B at 01:00 and C at 06:00 on day 1.
    network.trip_with_flags(
        "night",
        &[
            (a, 1320, 1320, true, true),
            (b, 1500, 1505, true, true),
            (c, 1800, 1800, true, true),
        ],
        &[0],
    );
    let bundle = network.build();
    let from_b = search(
        &bundle,
        &[candidate(b, 0)],
        &[candidate(c, 0)],
        1440 + 30,
        1440 + 120,
    );
    assert_eq!(from_b, [(1505, 1800, 1, vec!["night".to_owned()])]);
}

#[test]
fn range_search_keeps_later_departures_and_fewer_transfers() {
    let mut network = Network::new();
    let a = network.station("A");
    let b = network.station("B");
    let c = network.station("C");
    // Early: direct but slow. Later: with a change, arriving earlier.
    network.trip("direct", &[(a, 600, 600), (c, 780, 780)]);
    network.trip("feeder", &[(a, 620, 620), (b, 650, 650)]);
    network.trip("express", &[(b, 665, 665), (c, 720, 720)]);
    let bundle = network.build();
    let journeys = search(&bundle, &[candidate(a, 0)], &[candidate(c, 0)], 590, 640);
    assert_eq!(
        journeys,
        [
            (620, 720, 2, vec!["feeder".to_owned(), "express".to_owned()]),
            (600, 780, 1, vec!["direct".to_owned()]),
        ]
    );
}

#[test]
fn starts_from_the_best_of_several_departure_stations() {
    let mut network = Network::new();
    let near = network.station("Near");
    let far = network.station("Far");
    let target = network.station("Target");
    network.trip("slow", &[(near, 600, 600), (target, 800, 800)]);
    network.trip("fast", &[(far, 630, 630), (target, 700, 700)]);
    let bundle = network.build();
    let journeys = search(
        &bundle,
        &[candidate(near, 5), candidate(far, 40)],
        &[candidate(target, 0)],
        580,
        600,
    );
    // The slow train leaves the place later, so both journeys are Pareto-optimal.
    assert_eq!(
        journeys,
        [
            (590, 700, 1, vec!["fast".to_owned()]),
            (595, 800, 1, vec!["slow".to_owned()])
        ]
    );
}
