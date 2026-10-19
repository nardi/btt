//! The rides of a trip as the optimizer sees them.

use btt_fares::country_of_time_zone;
use btt_model::{
    GeoCoordinates, Journey, LayoutLink, LegPlanStation, TrainLabel, TrainRide, TripPlaceId,
    UnixTimeMinutes,
};
use chrono::{DateTime, NaiveDate, NaiveTime};
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
    /// Local time of day of the departure.
    pub departure_clock: NaiveTime,
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
    /// Pages that show the layout of the train.
    pub layout_links: Vec<LayoutLink>,
}

/// Returns the local date and time of day of a moment in a time zone, or `None` for an unknown
/// zone.
fn local_moment(moment: UnixTimeMinutes, time_zone: &str) -> Option<(NaiveDate, NaiveTime)> {
    let zone: Tz = time_zone.parse().ok()?;
    let utc = DateTime::from_timestamp(i64::from(moment.0) * 60, 0)?;
    let local = utc.with_timezone(&zone);
    Some((local.date_naive(), local.time()))
}

/// Returns the local date of a moment in a time zone, or `None` for an unknown zone.
fn local_date(moment: UnixTimeMinutes, time_zone: &str) -> Option<NaiveDate> {
    local_moment(moment, time_zone).map(|(date, _)| date)
}

/// Returns the length of a ride along its line in kilometres, from the boarding station through
/// the points of the line, or else through the stops, to the alighting station.
fn ride_distance_kilometres(ride: &TrainRide, stations: &[LegPlanStation]) -> f64 {
    let station = |index: btt_model::LegPlanStationIndex| stations.get(usize::from(index.0));
    // The points of the line lie between the two stations and leave both out.
    let between = if ride.path_coordinates.is_empty() {
        &ride.intermediate_station_coordinates[..]
    } else {
        &ride.path_coordinates[..]
    };
    let points: Vec<_> = station(ride.boarding_station)
        .map(|station| station.coordinates)
        .into_iter()
        .chain(between.iter().copied())
        .chain(station(ride.alighting_station).map(|station| station.coordinates))
        .collect();
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
        .filter_map(|ride| ride_input(leg_arrival_place_id, ride, stations))
        .collect()
}

/// Builds the rides of a journey for pricing, which reads no leg attribution. A ride whose
/// stations the table does not hold or whose time zone is unknown is left out, which the
/// caller can tell from the length.
pub fn pricing_rides_of_journey(journey: &Journey, stations: &[LegPlanStation]) -> Vec<RideInput> {
    journey
        .rides()
        .filter_map(|ride| ride_input(UNGROUPED_PLACE, ride, stations))
        .collect()
}

/// Attributes a ride to no leg, which only the pricing paths use because they never read the
/// attribution.
const UNGROUPED_PLACE: TripPlaceId = TripPlaceId(std::num::NonZeroU32::MIN);

/// Builds the optimizer's view of one train ride, or returns `None` when the table does not
/// hold its stations or its time zone is unknown.
pub fn ride_input(
    leg_arrival_place_id: TripPlaceId,
    ride: &TrainRide,
    stations: &[LegPlanStation],
) -> Option<RideInput> {
    let from = stations.get(usize::from(ride.boarding_station.0))?;
    let to = stations.get(usize::from(ride.alighting_station.0))?;
    let (departure_date, departure_clock) = local_moment(ride.departure_time, &from.time_zone)?;
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
        departure_date,
        departure_clock,
        arrival_date: local_date(ride.arrival_time, &to.time_zone)?,
        from_country: country_of_time_zone(&from.time_zone).map(Into::into),
        to_country: country_of_time_zone(&to.time_zone).map(Into::into),
        distance_kilometres: ride_distance_kilometres(ride, stations),
        offers_berths: ride.offers_berths,
        layout_links: ride.layout_links.clone(),
    })
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

    #[test]
    fn the_distance_runs_from_station_to_station_through_the_line() {
        let point = |longitude| GeoCoordinates::from_degrees(52.0, longitude).unwrap();
        let station = |longitude| LegPlanStation {
            name: "Station".into(),
            coordinates: point(longitude),
            time_zone: "Europe/Amsterdam".into(),
        };
        let stations = [station(5.0), station(5.4)];
        let mut ride = TrainRide {
            boarding_station: btt_model::LegPlanStationIndex(0),
            departure_time: UnixTimeMinutes(0),
            alighting_station: btt_model::LegPlanStationIndex(1),
            arrival_time: UnixTimeMinutes(30),
            train_label: TrainLabel {
                category: "IC".into(),
                number: "1".into(),
                operator: "ns".into(),
                agency: "NS".into(),
            },
            offers_berths: false,
            intermediate_station_coordinates: std::iter::empty().collect(),
            path_coordinates: vec![point(5.2), point(5.3)],
            layout_links: Vec::new(),
        };
        let whole = point(5.0).haversine_distance_meters(point(5.4)) / 1000.0;
        // The line covers only the middle, and the ends still count.
        assert!((ride_distance_kilometres(&ride, &stations) - whole).abs() < 0.01);
        ride.path_coordinates.clear();
        assert!((ride_distance_kilometres(&ride, &stations) - whole).abs() < 0.01);
    }
}
