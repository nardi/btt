//! Journey searches on small hand-made timetables.

use std::num::NonZeroU8;

use btt_model::GeoCoordinates;
use btt_router::{
    AllowEveryRide, CandidateStation, CostSearchBuffers, JourneyCostModel, MinutesSinceBundleStart,
    RaptorLegQuery, RaptorSearchBuffers, RideFilter, SearchIndex, search_leg_journeys,
    search_leg_journeys_by_cost,
};
use btt_timetable::{
    BundleBuilder, BundleStationIndex, BundleTripIndex, LoadedTimetableBundle, RaptorRouteIndex,
    ServiceDayBitset, StationInput, TrainLabelInput, TripInput, TripStopInput,
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
                offers_berths: false,
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
    search_with_filter(bundle, from, to, window_start, window_end, &AllowEveryRide)
}

/// Searches like `search`, with a filter on boarding and alighting.
fn search_with_filter(
    bundle: &LoadedTimetableBundle,
    from: &[CandidateStation],
    to: &[CandidateStation],
    window_start: u32,
    window_end: u32,
    filter: &impl RideFilter,
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
        filter,
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

/// Forbids leaving any train before a moment, except where the train ends.
struct NoAlightingBefore(u32);

impl RideFilter for NoAlightingBefore {
    fn may_board(
        &self,
        _: RaptorRouteIndex,
        _: BundleTripIndex,
        _: MinutesSinceBundleStart,
    ) -> bool {
        true
    }

    fn may_alight(
        &self,
        _: RaptorRouteIndex,
        _: BundleTripIndex,
        arrival: MinutesSinceBundleStart,
        is_last_stop: bool,
    ) -> bool {
        is_last_stop || arrival.0 >= self.0
    }
}

#[test]
fn a_forbidden_stop_keeps_the_traveller_on_the_train() {
    let mut network = Network::new();
    let a = network.station("A");
    let b = network.station("B");
    let c = network.station("C");
    let d = network.station("D");
    network.trip("1", &[(a, 600, 600), (b, 700, 702), (c, 800, 800)]);
    network.trip("2", &[(b, 720, 720), (d, 780, 780)]);
    network.trip("3", &[(c, 810, 810), (d, 900, 900)]);
    let bundle = network.build();
    let from = [candidate(a, 0)];
    let to = [candidate(d, 0)];

    let free = search(&bundle, &from, &to, 590, 610);
    assert_eq!(free[0].3, ["1", "2"]);

    // Leaving train 1 at B at minute 700 is forbidden, so the traveller rides on to its end
    // at C and changes there.
    let kept_on = search_with_filter(&bundle, &from, &to, 590, 610, &NoAlightingBefore(750));
    assert_eq!(kept_on.len(), 1);
    assert_eq!(kept_on[0].3, ["1", "3"]);
    assert_eq!(kept_on[0].1, 900);
}

/// Night from 22:00 to 07:00 the next morning, in minutes since the start of day 0.
const NIGHT: std::ops::Range<u32> = 1320..1860;

/// A cost model with the planner's night terms. A minute of the night before arriving costs
/// three minutes, a minute of it on the train with berths takes four off, and a change costs 65
/// minutes.
struct NightCostModel<'a> {
    bundle: &'a LoadedTimetableBundle,
}

impl NightCostModel<'_> {
    fn night_minutes_until(moment: u32) -> i32 {
        i32::try_from(moment.clamp(NIGHT.start, NIGHT.end) - NIGHT.start).unwrap()
    }
}

impl JourneyCostModel for NightCostModel<'_> {
    fn access_cost(&self, _: &CandidateStation) -> i32 {
        0
    }

    fn ride_cost(
        &self,
        _: RaptorRouteIndex,
        trip: BundleTripIndex,
        departure: MinutesSinceBundleStart,
        arrival: MinutesSinceBundleStart,
        is_first_ride: bool,
    ) -> i32 {
        let sleep = if self.bundle.trip_train_label(trip).1 == "night" {
            Self::night_minutes_until(arrival.0) - Self::night_minutes_until(departure.0)
        } else {
            0
        };
        let change = if is_first_ride { 0 } else { 65 };
        change - 4 * sleep
    }

    fn egress_cost(&self, _: &CandidateStation, _: Option<u16>) -> i32 {
        0
    }

    fn elapsed_cost(
        &self,
        departure: MinutesSinceBundleStart,
        arrival: MinutesSinceBundleStart,
    ) -> i32 {
        i32::try_from(arrival.0 - departure.0).unwrap() + 3 * Self::night_minutes_until(arrival.0)
    }

    fn remaining_cost_lower_bound(
        &self,
        departure: MinutesSinceBundleStart,
        moment: MinutesSinceBundleStart,
        earliest_arrival: MinutesSinceBundleStart,
    ) -> i32 {
        let sleep_still_possible =
            Self::night_minutes_until(NIGHT.end) - Self::night_minutes_until(moment.0);
        self.elapsed_cost(departure, earliest_arrival) - 4 * sleep_still_possible
    }
}

#[test]
fn the_cost_search_stays_on_a_night_train_to_sleep_longer() {
    let mut network = Network::new();
    let amsterdam = network.station("Amsterdam");
    let basel = network.station("Basel");
    let zurich = network.station("Zürich");
    let bern = network.station("Bern");
    network.trip(
        "night",
        &[
            (amsterdam, 1141, 1141),
            (basel, 1701, 1705),
            (zurich, 1805, 1805),
        ],
    );
    // The connecting trains run on day 1, at 04:58 from Basel and 06:32 from Zürich.
    network.trip_with_flags(
        "early",
        &[(basel, 298, 298, true, true), (bern, 356, 356, true, true)],
        &[1],
    );
    network.trip_with_flags(
        "late",
        &[(zurich, 392, 392, true, true), (bern, 448, 448, true, true)],
        &[1],
    );
    let bundle = network.build();
    let from = [candidate(amsterdam, 0)];
    let to = [candidate(bern, 0)];

    // The plain search only keeps the change in Basel, which arrives earlier.
    let plain = search(&bundle, &from, &to, 1100, 1200);
    assert_eq!(plain.len(), 1);
    assert_eq!(plain[0].3, ["night", "early"]);

    let query = RaptorLegQuery {
        departure_candidates: &from,
        arrival_candidates: &to,
        departure_window: MinutesSinceBundleStart(1100)..MinutesSinceBundleStart(1200),
        minimum_transfer_minutes: 10,
        maximum_train_rides: NonZeroU8::new(5).unwrap(),
    };
    let found = search_leg_journeys_by_cost(
        &bundle,
        &SearchIndex::new(&bundle),
        &query,
        &NightCostModel { bundle: &bundle },
        &mut CostSearchBuffers::new(),
        || false,
    );
    let trains_and_costs: Vec<(Vec<&str>, i32)> = found
        .journeys
        .iter()
        .map(|costed| {
            let trains = costed
                .journey
                .rides
                .iter()
                .map(|ride| bundle.trip_train_label(ride.trip).1)
                .collect();
            (trains, costed.cost)
        })
        .collect();
    // Staying on to Zürich sleeps 104 minutes longer, which outweighs arriving 92 minutes later.
    assert_eq!(
        trains_and_costs,
        [(vec!["night", "early"], 624), (vec!["night", "late"], 492)]
    );
}
