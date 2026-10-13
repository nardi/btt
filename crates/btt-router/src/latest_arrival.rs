//! The latest moment at every station from which the arrival place can still be reached by a
//! deadline, found by a search that runs backwards in time from the arrival place.
//!
//! This is the backward pruning search of Bounded McRAPTOR (Delling, Dibbelt and Pajor, "Fast
//! and Exact Public Transit Routing with Restricted Pareto Sets", ALENEX 2019). A forward search
//! that knows a good journey turns its cost into a deadline, and drops every label that arrives
//! at a station after the latest moment found here. Unlike the static lower bounds of
//! `lower_bound`, these moments follow the timetable, so a station whose last useful train has
//! left is known to be a dead end.
//!
//! The search goes route by route like RAPTOR, one more train per round. From a station that a
//! traveller can still leave by some moment, it rides each route backwards: the latest trip that
//! reaches the station in time sets the latest departure at every stop before it.
//!
//! The search keeps only moments that a forward label can reach: not before the departure
//! window starts, plus the fewest minutes from the departure place by the static lower bound
//! graph. Without that, it would run back through the whole timetable before the window.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use btt_timetable::{BundleStationIndex, LoadedTimetableBundle, RaptorRoute, RaptorRouteIndex};
use fixedbitset::FixedBitSet;

use crate::index::SearchIndex;
use crate::search::{MINUTES_PER_DAY, RaptorLegQuery};

/// The latest moments at every station, in minutes since the bundle starts.
#[derive(Debug, Default)]
pub(crate) struct LatestArrivals {
    /// The latest moment a traveller who arrived by train can be at each station, which is
    /// before the latest departure by the minimum transfer time. `None` when the arrival place
    /// can't be reached from the station by the deadline.
    by_train: Vec<Option<u32>>,
    /// The latest moment a traveller can be at each station and still board, which also holds
    /// for one who arrived on foot and so needs no transfer time.
    ready: Vec<Option<u32>>,
    /// Stations whose `by_train` moment grew in the current round, and a mark per station.
    changed: Vec<BundleStationIndex>,
    changed_marks: FixedBitSet,
    /// The stations of the round before.
    changed_before: Vec<BundleStationIndex>,
    /// The highest stop position to scan each route from, with the routes in the order queued.
    queued: Vec<(RaptorRouteIndex, u16)>,
    /// Position of each route in `queued`, or `u32::MAX`.
    queue_slot_by_route: Vec<u32>,
    /// The start of the departure window.
    earliest: u32,
    /// The fewest minutes from the departure place to each station, from the static lower
    /// bound graph. No label of the forward search is at a station before `earliest` plus
    /// these minutes, so the search drops every moment before that.
    minutes_from_departure: Vec<u32>,
    /// The queue of the search for `minutes_from_departure`.
    queue: BinaryHeap<Reverse<(u32, u32)>>,
}

impl LatestArrivals {
    /// Returns the latest moment at which a label can arrive at a station and still reach the
    /// arrival place by the deadline, or `None` when no moment can.
    pub(crate) fn latest(&self, station: BundleStationIndex) -> Option<u32> {
        let index = station.as_usize();
        match (self.by_train[index], self.ready[index]) {
            (Some(by_train), Some(ready)) => Some(by_train.max(ready)),
            (by_train, ready) => by_train.or(ready),
        }
    }

    /// Finds the latest moments for a query and a deadline at the arrival place.
    pub(crate) fn compute(
        &mut self,
        bundle: &LoadedTimetableBundle,
        index: &SearchIndex,
        query: &RaptorLegQuery<'_>,
        deadline: u32,
    ) {
        let station_count = bundle.station_count();
        self.by_train.clear();
        self.by_train.resize(station_count, None);
        self.ready.clear();
        self.ready.resize(station_count, None);
        self.changed.clear();
        self.changed_marks.clear();
        self.changed_marks.grow(station_count);
        self.queue_slot_by_route.clear();
        self.queue_slot_by_route
            .resize(bundle.route_count(), u32::MAX);
        self.earliest = query.departure_window.start.0;
        // A station the departure place can't reach before the deadline is of no use, which
        // keeps the search near the stations between the two places.
        index.lower_bounds.compute_bounds_from(
            query.departure_candidates,
            deadline.saturating_sub(self.earliest),
            &mut self.minutes_from_departure,
            &mut self.queue,
        );
        let minimum_transfer = u32::from(query.minimum_transfer_minutes);
        for candidate in query.arrival_candidates {
            if let Some(moment) = deadline.checked_sub(u32::from(candidate.time_to_place_minutes)) {
                // A traveller who arrives here by train goes on to the arrival place without
                // the transfer time.
                self.raise_ready(index, candidate.station, moment, minimum_transfer);
                self.raise_by_train(candidate.station, moment);
            }
        }
        // One more train each round, and one round for the walk to the first train.
        for _ in 0..=query.maximum_train_rides.get() {
            if self.changed.is_empty() {
                break;
            }
            std::mem::swap(&mut self.changed, &mut self.changed_before);
            for &station in &self.changed_before {
                self.changed_marks.set(station.as_usize(), false);
            }
            self.changed.clear();
            self.queue_routes(bundle);
            let queued = std::mem::take(&mut self.queued);
            for &(raptor_route, last_position) in &queued {
                self.queue_slot_by_route[raptor_route.as_usize()] = u32::MAX;
                self.scan_route(bundle, index, raptor_route, last_position, minimum_transfer);
            }
            self.queued = queued;
            self.queued.clear();
        }
    }

    /// Queues every route that a station of the round before lets the traveller leave a train
    /// at, from the latest such stop.
    fn queue_routes(&mut self, bundle: &LoadedTimetableBundle) {
        for &station in &self.changed_before {
            for route_stop in bundle.route_stops_at(station) {
                let route = bundle.route(route_stop.raptor_route);
                if !bundle
                    .pattern_stop(&route, route_stop.stop_position)
                    .alighting_allowed()
                {
                    continue;
                }
                let slot = &mut self.queue_slot_by_route[route_stop.raptor_route.as_usize()];
                if *slot == u32::MAX {
                    *slot = u32::try_from(self.queued.len()).unwrap_or(u32::MAX);
                    self.queued
                        .push((route_stop.raptor_route, route_stop.stop_position));
                } else if let Some(queued) = self.queued.get_mut(*slot as usize) {
                    queued.1 = queued.1.max(route_stop.stop_position);
                }
            }
        }
    }

    /// Rides a route backwards from a stop position. At each stop where a traveller can leave
    /// the train in time, the latest trip that gets there in time becomes the one to ride, and
    /// at each earlier stop its departure is a moment from which the traveller still makes it.
    fn scan_route(
        &mut self,
        bundle: &LoadedTimetableBundle,
        index: &SearchIndex,
        raptor_route: RaptorRouteIndex,
        last_position: u16,
        minimum_transfer: u32,
    ) {
        let route = bundle.route(raptor_route);
        // The trip to ride, as its offset and service day.
        let mut riding: Option<(u32, u16)> = None;
        for position in (0..=last_position).rev() {
            let pattern_stop = bundle.pattern_stop(&route, position);
            let station = pattern_stop.station();
            if let Some((trip_offset, service_day)) = riding
                && pattern_stop.boarding_allowed()
            {
                let departure = u32::from(service_day) * MINUTES_PER_DAY
                    + u32::from(bundle.stop_event(&route, trip_offset, position).departure.0);
                self.raise_ready(index, station, departure, minimum_transfer);
            }
            if pattern_stop.alighting_allowed()
                && let Some(latest) = self.by_train[station.as_usize()]
                && let Some(found) =
                    latest_trip_arriving_by(bundle, index, raptor_route, &route, position, latest)
            {
                // Trips of a route never overtake, so the trip that arrives here latest also
                // leaves every earlier stop latest.
                let arrival_of = |(trip_offset, service_day): (u32, u16)| {
                    u32::from(service_day) * MINUTES_PER_DAY
                        + u32::from(bundle.stop_event(&route, trip_offset, position).arrival.0)
                };
                if riding.is_none_or(|current| arrival_of(found) > arrival_of(current)) {
                    riding = Some(found);
                }
            }
        }
    }

    /// Records that a traveller can be ready at a station by a moment. A traveller who arrives
    /// by train needs the minimum transfer time on top, and one who walks there from another
    /// station after a train needs the walk.
    fn raise_ready(
        &mut self,
        index: &SearchIndex,
        station: BundleStationIndex,
        moment: u32,
        minimum_transfer: u32,
    ) {
        if !self.can_be_reached_by(station, moment)
            || !raise(&mut self.ready[station.as_usize()], moment)
        {
            return;
        }
        if let Some(by_train) = moment.checked_sub(minimum_transfer) {
            self.raise_by_train(station, by_train);
        }
        for &(from_station, minutes) in index.incoming_footpaths(station) {
            if let Some(by_train) = moment.checked_sub(u32::from(minutes)) {
                self.raise_by_train(from_station, by_train);
            }
        }
    }

    /// Records that a traveller arriving by train at a station by a moment still makes it, and
    /// marks the station for the next round when that is later than before.
    fn raise_by_train(&mut self, station: BundleStationIndex, moment: u32) {
        if self.can_be_reached_by(station, moment)
            && raise(&mut self.by_train[station.as_usize()], moment)
            && !self.changed_marks.put(station.as_usize())
        {
            self.changed.push(station);
        }
    }

    /// Returns whether a label of the forward search can be at a station by a moment.
    fn can_be_reached_by(&self, station: BundleStationIndex, moment: u32) -> bool {
        self.minutes_from_departure[station.as_usize()]
            .checked_add(self.earliest)
            .is_some_and(|earliest| earliest <= moment)
    }
}

/// Sets a moment to a later one. Returns whether it changed.
fn raise(slot: &mut Option<u32>, moment: u32) -> bool {
    if slot.is_some_and(|known| known >= moment) {
        return false;
    }
    *slot = Some(moment);
    true
}

/// Returns the trip of a route that arrives at a stop latest but no later than a moment, as
/// its offset and service day, or `None` when none does.
fn latest_trip_arriving_by(
    bundle: &LoadedTimetableBundle,
    index: &SearchIndex,
    raptor_route: RaptorRouteIndex,
    route: &RaptorRoute,
    position: u16,
    latest: u32,
) -> Option<(u32, u16)> {
    let route_days = index.service_days_by_route[raptor_route.as_usize()];
    let arrival_at =
        |trip_offset: u32| u32::from(bundle.stop_event(route, trip_offset, position).arrival.0);
    let last_day =
        (latest / MINUTES_PER_DAY).min(u32::from(bundle.validity_days()).saturating_sub(1));
    let first_day = (latest / MINUTES_PER_DAY).saturating_sub(u32::from(route.longest_trip_days));
    let mut best: Option<(u32, (u32, u16))> = None;
    for day in (first_day..=last_day).rev() {
        let service_day = u16::try_from(day).unwrap_or(u16::MAX);
        if !route_days.runs_on(service_day) {
            continue;
        }
        let day_start = day * MINUTES_PER_DAY;
        let Some(limit) = latest.checked_sub(day_start) else {
            continue;
        };
        // Arrivals at the stop are sorted, because no trip of a route overtakes another.
        let mut offset = crate::search::partition_point(route.trip_count, |trip_offset| {
            arrival_at(trip_offset) <= limit
        });
        while offset > 0 {
            offset -= 1;
            let arrival = day_start + arrival_at(offset);
            if best.is_some_and(|(known, _)| arrival <= known) {
                break;
            }
            let trip = btt_timetable::BundleTripIndex(route.first_trip_index + offset);
            if bundle.trip_service_days(trip).runs_on(service_day) {
                best = Some((arrival, (offset, service_day)));
                break;
            }
        }
    }
    best.map(|(_, found)| found)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU8;

    use btt_model::GeoCoordinates;
    use btt_timetable::{
        BundleBuilder, ServiceDayBitset, StationInput, TrainLabelInput, TripInput, TripStopInput,
    };
    use chrono::NaiveDate;
    use proptest::prelude::*;

    use super::*;
    use crate::search::{
        AllowEveryRide, CandidateStation, MinutesSinceBundleStart, RaptorSearchBuffers,
        search_leg_journeys,
    };

    const STATION_COUNT: u32 = 5;
    const MINIMUM_TRANSFER_MINUTES: u16 = 5;
    const MAXIMUM_TRAIN_RIDES: u8 = 4;

    /// A trip as its stations, its first departure and the minutes between its stops.
    type RandomTrip = (Vec<u32>, u16, Vec<u16>);

    fn random_trip() -> impl Strategy<Value = RandomTrip> {
        (2..=4usize).prop_flat_map(|stop_count| {
            (
                Just((0..STATION_COUNT).collect::<Vec<_>>())
                    .prop_shuffle()
                    .prop_map(move |stations| stations[..stop_count].to_vec()),
                0..300u16,
                proptest::collection::vec(5..60u16, stop_count - 1),
            )
        })
    }

    fn random_footpath() -> impl Strategy<Value = (u32, u32, u16)> {
        (0..STATION_COUNT, 0..STATION_COUNT, 1..20u16)
            .prop_filter("distinct ends", |(from, to, _)| from != to)
    }

    /// Builds a bundle of the trips on its first day and the footpaths.
    fn bundle(trips: &[RandomTrip], footpaths: &[(u32, u32, u16)]) -> LoadedTimetableBundle {
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
        for (stations, first_departure, segments) in trips {
            let mut time = *first_departure;
            let mut stops = vec![TripStopInput {
                station: BundleStationIndex(stations[0]),
                arrival: time,
                departure: time,
                boarding_allowed: true,
                alighting_allowed: true,
            }];
            for (station, minutes) in stations[1..].iter().zip(segments) {
                time += minutes;
                stops.push(TripStopInput {
                    station: BundleStationIndex(*station),
                    arrival: time,
                    departure: time,
                    boarding_allowed: true,
                    alighting_allowed: true,
                });
            }
            builder.add_trip(TripInput {
                stops,
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

    /// Returns the latest moment a traveller can leave from the departure candidates and still
    /// reach the arrival place by the deadline. The forward router finds it, and another test
    /// checks that router against a connection scan.
    fn latest_by_forward_search(
        bundle: &LoadedTimetableBundle,
        index: &SearchIndex,
        departure_candidates: &[CandidateStation],
        arrival_candidates: &[CandidateStation],
        deadline: u32,
    ) -> Option<u32> {
        let query = RaptorLegQuery {
            departure_candidates,
            arrival_candidates,
            departure_window: MinutesSinceBundleStart(0)..MinutesSinceBundleStart(deadline + 1),
            minimum_transfer_minutes: MINIMUM_TRANSFER_MINUTES,
            maximum_train_rides: NonZeroU8::new(MAXIMUM_TRAIN_RIDES).unwrap(),
        };
        search_leg_journeys(
            bundle,
            index,
            &query,
            &AllowEveryRide,
            &mut RaptorSearchBuffers::new(),
            || false,
        )
        .journeys
        .iter()
        .filter(|journey| journey.arrival.0 <= deadline)
        .map(|journey| journey.departure.0)
        .max()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn no_station_is_given_up_while_the_deadline_can_be_met(
            trips in proptest::collection::vec(random_trip(), 1..10),
            footpaths in proptest::collection::vec(random_footpath(), 0..4),
            time_to_place in 0..10u16,
            deadline in 50..500u32,
        ) {
            // A traveller who can still reach the arrival place by the deadline from a station
            // must never arrive there later than the latest moment says, or the forward search
            // would drop a journey it needs. That holds for a traveller who is ready at the
            // station, and for one who arrives on a train and needs the transfer time or a walk.
            let bundle = bundle(&trips, &footpaths);
            let index = SearchIndex::new(&bundle);
            let target = BundleStationIndex(STATION_COUNT - 1);
            let arrival_candidates =
                [CandidateStation { station: target, time_to_place_minutes: time_to_place }];
            // Every station is a departure candidate, so none is out of reach of the departure
            // place.
            let every_station: Vec<CandidateStation> = (0..STATION_COUNT)
                .map(|station| CandidateStation {
                    station: BundleStationIndex(station),
                    time_to_place_minutes: 0,
                })
                .collect();
            let query = RaptorLegQuery {
                departure_candidates: &every_station,
                arrival_candidates: &arrival_candidates,
                departure_window: MinutesSinceBundleStart(0)..MinutesSinceBundleStart(1),
                minimum_transfer_minutes: MINIMUM_TRANSFER_MINUTES,
                maximum_train_rides: NonZeroU8::new(MAXIMUM_TRAIN_RIDES).unwrap(),
            };
            let mut latest = LatestArrivals::default();
            latest.compute(&bundle, &index, &query, deadline);
            if let Some(moment) = deadline.checked_sub(u32::from(time_to_place)) {
                prop_assert!(latest.latest(target).is_some_and(|known| known >= moment));
            }
            for station in (0..STATION_COUNT - 1).map(BundleStationIndex) {
                let ready = [CandidateStation { station, time_to_place_minutes: 0 }];
                let by_train: Vec<CandidateStation> = std::iter::once(CandidateStation {
                    station,
                    time_to_place_minutes: MINIMUM_TRANSFER_MINUTES,
                })
                .chain(bundle.footpaths_from(station).map(|footpath| CandidateStation {
                    station: footpath.to_station,
                    time_to_place_minutes: footpath.duration_minutes,
                }))
                .collect();
                for candidates in [&ready[..], &by_train[..]] {
                    if let Some(needed) = latest_by_forward_search(
                        &bundle,
                        &index,
                        candidates,
                        &arrival_candidates,
                        deadline,
                    ) {
                        prop_assert!(
                            latest.latest(station).is_some_and(|known| known >= needed),
                            "station {} must allow {needed}, has {:?}",
                            station.0,
                            latest.latest(station)
                        );
                    }
                }
            }
        }
    }
}
