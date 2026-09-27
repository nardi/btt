//! A backend that invents one direct train per leg from the straight-line distance.
//!
//! It lets the whole pipeline from edit to plan run before any timetable data exists.

use std::sync::Arc;

use btt_geo::{TimeZoneFinder, local_time_to_unix_minutes};
use btt_model::{
    DurationMinutes, GeoCoordinates, Journey, LegPlan, LegPlanStation, LegPlanStationIndex,
    LegPlanningResult, LegTravelMode, TrainLabel, TrainRide, UnixTimeMinutes,
};
use smallvec::{SmallVec, smallvec};

use crate::input::LegPlanningInput;

/// Rail routes are longer than the straight line between their ends by roughly this factor.
const ROUTE_DETOUR_FACTOR: f64 = 1.3;

/// Average speed of the invented trains.
const STUB_TRAIN_SPEED_METERS_PER_MINUTE: f64 = 100_000.0 / 60.0;

/// Plans every leg as one invented direct train.
pub struct StubBackend {
    /// Resolves the time zone of the departure place, so the train leaves at the start of the
    /// local departure window.
    time_zone_finder: Arc<TimeZoneFinder>,
}

impl StubBackend {
    /// Creates a stub backend that shares a time zone finder.
    pub fn new(time_zone_finder: Arc<TimeZoneFinder>) -> Self {
        Self { time_zone_finder }
    }

    /// Invents a direct train that departs at the start of the departure window and runs at a
    /// constant speed along a slightly longer path than the straight line.
    pub fn plan_leg(&self, input: &LegPlanningInput) -> LegPlanningResult {
        let departure_coordinates = input.departure.coordinates;
        let arrival_coordinates = input.arrival.coordinates;
        let departure_time_zone = self.time_zone_finder.time_zone_at(departure_coordinates);
        let arrival_time_zone = self.time_zone_finder.time_zone_at(arrival_coordinates);
        // A night train leaves in the boarding window, which is usually later than the leg's
        // departure window.
        let departure_window_start = match input.travel_mode {
            LegTravelMode::Daytime => input.departure.time_window.start,
            LegTravelMode::Overnight(options) => options.boarding_window.start,
        };
        let departure_time = local_time_to_unix_minutes(
            input.departure_date,
            u32::from(departure_window_start.minutes_since_midnight()),
            departure_time_zone,
        );
        let ride_minutes = stub_ride_minutes(departure_coordinates, arrival_coordinates);
        let arrival_time =
            UnixTimeMinutes(departure_time.0.saturating_add(u32::from(ride_minutes)));
        let journey = Journey {
            access_duration: DurationMinutes::ZERO,
            first_ride: TrainRide {
                boarding_station: LegPlanStationIndex(0),
                departure_time,
                alighting_station: LegPlanStationIndex(1),
                arrival_time,
                train_label: TrainLabel {
                    category: "STUB".into(),
                    number: "1".into(),
                },
            },
            later_rides: SmallVec::new(),
            arrival_footpath: None,
            egress_duration: DurationMinutes::ZERO,
        };
        LegPlanningResult::Found(Box::new(LegPlan {
            chosen_journey: journey,
            alternative_journeys: SmallVec::new(),
            station_table: smallvec![
                LegPlanStation {
                    name: "Departure station".into(),
                    coordinates: departure_coordinates,
                    time_zone: departure_time_zone.name().into(),
                },
                LegPlanStation {
                    name: "Arrival station".into(),
                    coordinates: arrival_coordinates,
                    time_zone: arrival_time_zone.name().into(),
                },
            ],
        }))
    }
}

/// Returns the ride time of an invented train between two points, at least one minute.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn stub_ride_minutes(from: GeoCoordinates, to: GeoCoordinates) -> u16 {
    let route_meters = from.haversine_distance_meters(to) * ROUTE_DETOUR_FACTOR;
    let minutes = (route_meters / STUB_TRAIN_SPEED_METERS_PER_MINUTE).ceil();
    // The float is finite and non-negative, and `as` saturates at the largest `u16`.
    (minutes as u16).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ride_from_amsterdam_to_berlin_takes_about_seven_and_a_half_hours() {
        let amsterdam = GeoCoordinates::from_degrees(52.378_9, 4.900_3).unwrap();
        let berlin = GeoCoordinates::from_degrees(52.525_1, 13.369_4).unwrap();
        // 574 km times 1.3 at 100 km/h is about 448 minutes.
        assert!((440..460).contains(&stub_ride_minutes(amsterdam, berlin)));
        assert_eq!(stub_ride_minutes(amsterdam, amsterdam), 1);
    }
}
