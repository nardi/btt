//! The journey cost of `scoring` as a cost model that the router adds up along a journey.

use std::ops::Range;

use btt_router::{CandidateStation, JourneyCostModel, MinutesSinceBundleStart};
use btt_timetable::{BundleTripIndex, LoadedTimetableBundle, RaptorRouteIndex};

use crate::scoring::ScoringWeights;

/// Hundredths of a minute per minute. The model counts in hundredths so that the weights in
/// percent need no rounding.
const HUNDREDTHS: i32 = 100;

/// The cost of `journey_cost` split into the parts that each step of a journey adds, in
/// hundredths of a minute of train travel.
///
/// The two costs differ only in rounding: `journey_cost` rounds the extra weight of the ways
/// to stations down once for the whole journey, while this model doesn't round.
pub(crate) struct LegCostModel<'a> {
    /// The bundle that says which trips have berths.
    pub(crate) bundle: &'a LoadedTimetableBundle,
    /// The weights of the cost.
    pub(crate) weights: ScoringWeights,
    /// Minutes one change of trains costs at the traveller's transfer aversion.
    pub(crate) transfer_cost_minutes: u32,
    /// The arrival window in bundle minutes, which starts no earlier than the wake-up time on
    /// an overnight leg.
    pub(crate) arrival_window: Range<u32>,
    /// The night from bedtime to wake-up in bundle minutes, on an overnight leg.
    pub(crate) night: Option<Range<u32>>,
    /// Journeys that cost more than this many times the cheapest one found are not searched
    /// for, because the leg plan wouldn't show them.
    pub(crate) cost_limit_factor: u32,
    /// Whether each route has a trip with berths.
    pub(crate) routes_with_berths: &'a [bool],
}

/// Converts a minute count or a percentage to the model's signed arithmetic.
fn signed(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

impl LegCostModel<'_> {
    /// Returns the minutes of the night that lie within a range of moments.
    fn night_minutes_within(&self, moments: Range<u32>) -> i32 {
        self.night.as_ref().map_or(0, |night| {
            signed(
                moments
                    .end
                    .min(night.end)
                    .saturating_sub(moments.start.max(night.start)),
            )
        })
    }

    /// Returns the extra weight of a minute on the way to or from a station, in hundredths.
    fn way_extra_per_minute(&self) -> i32 {
        signed(self.weights.way_to_station_percent.saturating_sub(100))
    }

    /// Returns what a minute asleep in a berth takes off: the minute of travel it is part of,
    /// and the minute of the night it no longer spends awake.
    fn sleep_per_minute(&self) -> i32 {
        HUNDREDTHS + signed(self.weights.night_awake_percent)
    }
}

impl JourneyCostModel for LegCostModel<'_> {
    fn access_cost(&self, candidate: &CandidateStation) -> i32 {
        i32::from(candidate.time_to_place_minutes) * self.way_extra_per_minute()
    }

    fn ride_cost(
        &self,
        _: RaptorRouteIndex,
        trip: BundleTripIndex,
        departure: MinutesSinceBundleStart,
        arrival: MinutesSinceBundleStart,
        is_first_ride: bool,
    ) -> i32 {
        let change = if is_first_ride {
            0
        } else {
            signed(self.transfer_cost_minutes) * HUNDREDTHS
        };
        let sleep = if self.bundle.trip_offers_berths(trip) {
            self.night_minutes_within(departure.0..arrival.0) * self.sleep_per_minute()
        } else {
            0
        };
        change - sleep
    }

    fn egress_cost(
        &self,
        candidate: &CandidateStation,
        final_footpath_minutes: Option<u16>,
    ) -> i32 {
        // A walk between stations at the end counts as a change, and as part of the way to the
        // place.
        let footpath = final_footpath_minutes.map_or(0, |minutes| {
            signed(self.transfer_cost_minutes) * HUNDREDTHS
                + i32::from(minutes) * self.way_extra_per_minute()
        });
        i32::from(candidate.time_to_place_minutes) * self.way_extra_per_minute() + footpath
    }

    fn elapsed_cost(
        &self,
        departure: MinutesSinceBundleStart,
        arrival: MinutesSinceBundleStart,
    ) -> i32 {
        let arrival = arrival.0;
        let travel = signed(arrival.saturating_sub(departure.0)) * HUNDREDTHS;
        let before_window = signed(self.arrival_window.start.saturating_sub(arrival))
            * signed(self.weights.arrival_before_window_percent);
        let after_window = signed(arrival.saturating_sub(self.arrival_window.end))
            * signed(self.weights.arrival_after_window_percent);
        let night_until_arrival = self
            .night
            .as_ref()
            .map_or(0, |night| self.night_minutes_within(night.start..arrival));
        travel
            + before_window
            + after_window
            + night_until_arrival * signed(self.weights.night_awake_percent)
    }

    /// Sleep before the earliest arrival can take off at most the full weight of a minute
    /// asleep. After it, a minute asleep also moves the arrival a minute later, which costs at
    /// least the minute of travel minus the weight of arriving before the window. The bound
    /// takes both at their largest.
    fn remaining_cost_lower_bound(
        &self,
        departure: MinutesSinceBundleStart,
        moment: MinutesSinceBundleStart,
        earliest_arrival: MinutesSinceBundleStart,
    ) -> i32 {
        let elapsed = self.elapsed_cost(departure, earliest_arrival);
        let Some(night) = &self.night else {
            return elapsed;
        };
        let sleep_before = self.night_minutes_within(moment.0..earliest_arrival.0);
        let sleep_after = self.night_minutes_within(moment.0.max(earliest_arrival.0)..night.end);
        let later_arrival_saving = signed(self.weights.arrival_before_window_percent.min(100));
        elapsed - sleep_before * self.sleep_per_minute() - sleep_after * later_arrival_saving
    }

    /// A ride costs a change penalty, which doesn't depend on time, and takes off the night
    /// minutes on a train with berths, which does. Without a night or berths, time doesn't
    /// matter.
    fn ride_cost_depends_on_time(&self, raptor_route: RaptorRouteIndex) -> bool {
        self.night.is_some()
            && self
                .routes_with_berths
                .get(raptor_route.as_usize())
                .is_none_or(|&has_berths| has_berths)
    }
    /// The extra minute covers the rounding in which this cost and the ranking's differ.
    fn cost_limit(&self, best_cost: i32) -> i32 {
        best_cost
            .max(0)
            .saturating_mul(signed(self.cost_limit_factor))
            .saturating_add(HUNDREDTHS)
    }
}

#[cfg(test)]
mod tests {
    use btt_model::{GeoCoordinates, TransferAversion};
    use btt_router::CandidateStation;
    use btt_timetable::{
        BundleBuilder, BundleStationIndex, BundleTripIndex, LoadedTimetableBundle,
        RaptorRouteIndex, ServiceDayBitset, StationInput, TrainLabelInput, TripInput,
        TripStopInput,
    };
    use chrono::NaiveDate;

    use super::*;
    use crate::scoring::{JourneyCostInputs, journey_cost, transfer_cost_minutes};

    /// A bundle with a night train with berths from station 0 to 1 and a seated train from 1
    /// to 2, which gives the model a trip of each kind. The model reads only whether a trip has
    /// berths, and takes the times of a ride as arguments.
    fn bundle() -> LoadedTimetableBundle {
        let mut builder =
            BundleBuilder::new(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(), 2, 1).unwrap();
        for index in 0..3 {
            builder
                .add_station(StationInput {
                    name: format!("S{index}"),
                    coordinates: GeoCoordinates::from_degrees(50.0, 8.0).unwrap(),
                    time_zone: "UTC".to_owned(),
                    trainline_station_id: None,
                })
                .unwrap();
        }
        let mut first_day = ServiceDayBitset::default();
        first_day.insert(0);
        for (from, to, departure, arrival, offers_berths) in
            [(0, 1, 1010, 1500, true), (1, 2, 70, 160, false)]
        {
            let stop = |station, time| TripStopInput {
                station: BundleStationIndex(station),
                arrival: time,
                departure: time,
                boarding_allowed: true,
                alighting_allowed: true,
            };
            builder.add_trip(TripInput {
                stops: vec![stop(from, departure), stop(to, arrival)],
                service_days: first_day,
                train_label: TrainLabelInput {
                    category: String::new(),
                    number: String::new(),
                    agency: String::new(),
                    offers_berths,
                },
            });
        }
        LoadedTimetableBundle::from_bundle(&builder.build().unwrap()).unwrap()
    }

    #[test]
    fn the_steps_of_a_journey_add_up_to_its_ranking_cost() {
        let bundle = bundle();
        let aversion = TransferAversion::new(50).unwrap();
        let weights = ScoringWeights::DEFAULT;
        let model = LegCostModel {
            bundle: &bundle,
            weights,
            transfer_cost_minutes: transfer_cost_minutes(aversion, weights),
            arrival_window: 1800..1900,
            night: Some(1320..1800),
            cost_limit_factor: 2,
            routes_with_berths: &[],
        };
        let (berth_trip, seated_trip) = if bundle.trip_offers_berths(BundleTripIndex(0)) {
            (BundleTripIndex(0), BundleTripIndex(1))
        } else {
            (BundleTripIndex(1), BundleTripIndex(0))
        };
        let minutes = MinutesSinceBundleStart;
        let candidate = |time_to_place_minutes| CandidateStation {
            station: BundleStationIndex(0),
            time_to_place_minutes,
        };
        // Leave at 1000 with 10 minutes to the station, sleep on the first train, change, and
        // walk 6 minutes to a station 4 minutes from the place, arriving at 1610.
        let steps = [
            model.access_cost(&candidate(10)),
            model.ride_cost(
                RaptorRouteIndex(0),
                berth_trip,
                minutes(1010),
                minutes(1500),
                true,
            ),
            model.ride_cost(
                RaptorRouteIndex(0),
                seated_trip,
                minutes(1510),
                minutes(1600),
                false,
            ),
            model.egress_cost(&candidate(4), Some(6)),
            model.elapsed_cost(minutes(1000), minutes(1610)),
        ];
        let inputs = JourneyCostInputs {
            door_to_door_minutes: 610,
            way_to_station_minutes: 20,
            transfer_count: 2,
            minutes_before_arrival_window: 190,
            minutes_after_arrival_window: 0,
            sleeping_minutes: 180,
            night_minutes_awake: 110,
        };
        let ranking_cost = journey_cost(inputs, aversion, weights);
        assert_eq!(steps.iter().sum::<i32>(), signed(ranking_cost) * HUNDREDTHS);
        // The bound from the start never exceeds the journey's cost.
        let bound = model.remaining_cost_lower_bound(minutes(1000), minutes(1010), minutes(1600))
            + steps[0];
        assert!(bound <= steps.iter().sum::<i32>());
    }
}
