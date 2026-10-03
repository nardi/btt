//! The rides of a trip as the optimizer sees them.

use btt_fares::country_of_time_zone;
use btt_model::{
    GeoCoordinates, Journey, LegPlanStation, TrainLabel, TrainRide, TripPlaceId, UnixTimeMinutes,
};
use chrono::{DateTime, NaiveDate};
use chrono_tz::Tz;
use smol_str::SmolStr;

/// One train of the trip, with what the optimizer needs to price and cover it.
#[derive(Debug, Clone, PartialEq)]
pub struct RideInput {
    /// The place the ride's leg arrives at.
    pub leg_arrival_place_id: TripPlaceId,
    /// How the train is shown.
    pub train_label: TrainLabel,
    /// Name of the boarding station.
    pub from_name: SmolStr,
    /// Name of the alighting station.
    pub to_name: SmolStr,
    /// Position of the boarding station.
    pub from_coordinates: GeoCoordinates,
    /// Position of the alighting station.
    pub to_coordinates: GeoCoordinates,
    /// Departure.
    pub departure_time: UnixTimeMinutes,
    /// Arrival.
    pub arrival_time: UnixTimeMinutes,
    /// IANA time zone of the boarding station.
    pub time_zone: SmolStr,
    /// Local date of the departure, which is the ride's travel day.
    pub departure_date: NaiveDate,
    /// Local date of the arrival.
    pub arrival_date: NaiveDate,
    /// Country of the boarding station, if known.
    pub from_country: Option<SmolStr>,
    /// Country of the alighting station, if known.
    pub to_country: Option<SmolStr>,
    /// Length of the ride along its line, in kilometres.
    pub distance_kilometres: f64,
    /// Whether the train has berths.
    pub offers_berths: bool,
}

/// Returns the local date of a moment in a time zone, or `None` for an unknown zone.
fn local_date(moment: UnixTimeMinutes, time_zone: &str) -> Option<NaiveDate> {
    let zone: Tz = time_zone.parse().ok()?;
    let utc = DateTime::from_timestamp(i64::from(moment.0) * 60, 0)?;
    Some(utc.with_timezone(&zone).date_naive())
}

/// Returns the length of a ride along its line in kilometres, from the points of the line or
/// else from the stops.
fn ride_distance_kilometres(ride: &TrainRide, stations: &[LegPlanStation]) -> f64 {
    let station = |index: btt_model::LegPlanStationIndex| stations.get(usize::from(index.0));
    let points: Vec<_> = if ride.path_coordinates.len() >= 2 {
        ride.path_coordinates.clone()
    } else {
        station(ride.boarding_station)
            .map(|station| station.coordinates)
            .into_iter()
            .chain(ride.intermediate_station_coordinates.iter().copied())
            .chain(station(ride.alighting_station).map(|station| station.coordinates))
            .collect()
    };
    points
        .windows(2)
        .map(|pair| pair[0].haversine_distance_meters(pair[1]))
        .sum::<f64>()
        / 1000.0
}

/// Builds the rides of one leg's chosen journey. A ride whose stations the table does not hold
/// or whose time zone is unknown is left out.
pub fn rides_of_journey(
    leg_arrival_place_id: TripPlaceId,
    journey: &Journey,
    stations: &[LegPlanStation],
) -> Vec<RideInput> {
    journey
        .rides()
        .filter_map(|ride| {
            let from = stations.get(usize::from(ride.boarding_station.0))?;
            let to = stations.get(usize::from(ride.alighting_station.0))?;
            Some(RideInput {
                leg_arrival_place_id,
                train_label: ride.train_label.clone(),
                from_name: from.name.clone(),
                to_name: to.name.clone(),
                from_coordinates: from.coordinates,
                to_coordinates: to.coordinates,
                departure_time: ride.departure_time,
                arrival_time: ride.arrival_time,
                time_zone: from.time_zone.clone(),
                departure_date: local_date(ride.departure_time, &from.time_zone)?,
                arrival_date: local_date(ride.arrival_time, &to.time_zone)?,
                from_country: country_of_time_zone(&from.time_zone).map(Into::into),
                to_country: country_of_time_zone(&to.time_zone).map(Into::into),
                distance_kilometres: ride_distance_kilometres(ride, stations),
                offers_berths: ride.offers_berths,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moment_has_the_date_of_its_zone() {
        // 2026-10-10 23:30 UTC is already the 11th in Vienna.
        let moment = UnixTimeMinutes(29_861_250);
        let date = |year, month, day| NaiveDate::from_ymd_opt(year, month, day).unwrap();
        assert_eq!(
            local_date(moment, "Europe/Vienna"),
            Some(date(2026, 10, 11))
        );
        assert_eq!(
            local_date(moment, "Europe/London"),
            Some(date(2026, 10, 11))
        );
        assert_eq!(
            local_date(moment, "America/New_York"),
            Some(date(2026, 10, 10))
        );
        assert_eq!(local_date(moment, "Nowhere/Land"), None);
    }
}
