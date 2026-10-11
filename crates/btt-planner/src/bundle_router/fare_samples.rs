//! Single-train rides picked across the bundle, for collecting the live fares that seed the
//! price model.

use btt_model::{LegPlanStation, TrainRide};
use btt_router::{MinutesSinceBundleStart, RaptorRide};
use btt_timetable::{BundleTripIndex, RaptorRouteIndex};
use chrono::NaiveDate;
use rustc_hash::{FxHashMap, FxHashSet};

use super::{LegPlanStations, MINUTES_PER_DAY, TimetableBundleRouter};
use crate::pricing::JourneyPricer;

/// How many picks a sample may try per ride it asks for. Picks fail on days a trip does not
/// run and on stops where boarding or alighting is not allowed.
const ATTEMPTS_PER_RIDE: usize = 30;

/// A ride picked for its fare, with the stations it refers to.
pub struct FareSampleRide {
    /// The ride, with stations that index `stations`.
    pub ride: TrainRide,
    /// The stations of the ride.
    pub stations: Vec<LegPlanStation>,
}

/// A small generator of pseudo-random numbers by the `SplitMix64` method, so the same seed
/// picks the same rides from the same bundle.
struct SplitMixGenerator(u64);

impl SplitMixGenerator {
    /// Returns the next number.
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut mixed = self.0;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        mixed ^ (mixed >> 31)
    }

    /// Returns a number below `bound`, which must not be zero.
    fn below(&mut self, bound: usize) -> usize {
        // The remainder is below `bound`, so it fits in `usize`.
        #[expect(clippy::cast_possible_truncation)]
        let index = (self.next() % bound as u64) as usize;
        index
    }
}

impl<P: JourneyPricer> TimetableBundleRouter<P> {
    /// Picks up to `per_operator` rides of each operator's trains, each on a random trip, from
    /// a random stop to a later one, on one of `days`. The rides span distances from one stop
    /// to the whole route. A day outside the validity of the bundle is never picked.
    pub fn fare_sample_rides(
        &self,
        operators: &[&str],
        per_operator: usize,
        days: &[NaiveDate],
        seed: u64,
    ) -> Vec<FareSampleRide> {
        let validity_start = self.bundle.validity_start();
        let service_days: Vec<u16> = days
            .iter()
            .filter_map(|day| u16::try_from((*day - validity_start).num_days()).ok())
            .filter(|&day| day < self.bundle.validity_days())
            .collect();
        if service_days.is_empty() || per_operator == 0 {
            return Vec::new();
        }
        // The routes of each operator, by the operator of their first trip.
        let mut routes_by_operator: FxHashMap<&str, Vec<RaptorRouteIndex>> = FxHashMap::default();
        for route_index in 0..self.bundle.route_count() {
            let index = RaptorRouteIndex(u32::try_from(route_index).unwrap_or(u32::MAX));
            let route = self.bundle.route(index);
            if route.trip_count == 0 || route.stop_count < 2 {
                continue;
            }
            let (category, _, agency) = self
                .bundle
                .trip_train_label(BundleTripIndex(route.first_trip_index));
            let operator = self.operators.operator_of(agency, category);
            if let Some(&wanted) = operators.iter().find(|&&wanted| wanted == operator) {
                routes_by_operator.entry(wanted).or_default().push(index);
            }
        }
        let mut random = SplitMixGenerator(seed);
        let mut picked = FxHashSet::default();
        let mut samples = Vec::new();
        for operator in operators {
            let Some(routes) = routes_by_operator.get(operator) else {
                continue;
            };
            let mut found = 0;
            for _ in 0..per_operator * ATTEMPTS_PER_RIDE {
                if found == per_operator {
                    break;
                }
                let route_index = routes[random.below(routes.len())];
                let route = self.bundle.route(route_index);
                let trip_offset =
                    u32::try_from(random.below(route.trip_count as usize)).unwrap_or(0);
                let trip = BundleTripIndex(route.first_trip_index + trip_offset);
                let service_day = service_days[random.below(service_days.len())];
                let stop_count = usize::from(route.stop_count);
                let boarding = u16::try_from(random.below(stop_count - 1)).unwrap_or(0);
                let alighting = boarding
                    + 1
                    + u16::try_from(random.below(stop_count - 1 - usize::from(boarding)))
                        .unwrap_or(0);
                let (boarding_stop, alighting_stop) = (
                    self.bundle.pattern_stop(&route, boarding),
                    self.bundle.pattern_stop(&route, alighting),
                );
                let usable = self.bundle.trip_service_days(trip).runs_on(service_day)
                    && boarding_stop.boarding_allowed()
                    && alighting_stop.alighting_allowed()
                    && picked.insert((trip, boarding, alighting, service_day));
                if !usable {
                    continue;
                }
                let day_start = u32::from(service_day) * MINUTES_PER_DAY;
                let raptor_ride = RaptorRide {
                    raptor_route: route_index,
                    trip,
                    service_day,
                    boarding_station: boarding_stop.station(),
                    boarding_position: boarding,
                    alighting_station: alighting_stop.station(),
                    alighting_position: alighting,
                    departure: MinutesSinceBundleStart(
                        day_start
                            + u32::from(
                                self.bundle
                                    .stop_event(&route, trip_offset, boarding)
                                    .departure
                                    .0,
                            ),
                    ),
                    arrival: MinutesSinceBundleStart(
                        day_start
                            + u32::from(
                                self.bundle
                                    .stop_event(&route, trip_offset, alighting)
                                    .arrival
                                    .0,
                            ),
                    ),
                };
                let mut stations = LegPlanStations::default();
                if let Some(ride) = self.train_ride(&raptor_ride, &mut stations) {
                    samples.push(FareSampleRide {
                        ride,
                        stations: stations.table.into_vec(),
                    });
                    found += 1;
                }
            }
        }
        samples
    }
}
