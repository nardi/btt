//! Plans a few real legs on a timetable bundle and prints the journeys with search times.
//!
//! Run with `cargo run --release -p btt-planner --example plan_on_bundle -- <bundle> <date>`,
//! where the date is a travel date inside the bundle's validity window. Five more arguments plan
//! one custom leg instead: the departure latitude and longitude, the arrival latitude and
//! longitude, and the station search radius in meters.

use std::sync::Arc;
use std::time::Instant;

use btt_geo::TimeZoneFinder;
use btt_model::{
    DailyTimeWindow, DurationMinutes, GeoCoordinates, InterrailPassPolicy, LegPlan,
    LegPlanStationIndex, LegPlanningResult, LegTravelMode, LocalTimeOfDay, StationSearchRadius,
    TransferAversion,
};
use btt_planner::{LegEndpoint, LegPlanningInput, TimetableBundleRouter};
use chrono::NaiveDate;

/// Legs to plan, as names and coordinates of both ends.
const LEGS: [(&str, f64, f64, &str, f64, f64); 5] = [
    ("Amsterdam", 52.3789, 4.9003, "Berlin", 52.5251, 13.3694),
    ("Utrecht", 52.0894, 5.1101, "München", 48.1402, 11.5586),
    ("Köln", 50.9430, 6.9589, "Wien", 48.1852, 16.3761),
    ("Hamburg", 53.5528, 10.0067, "Salzburg", 47.8129, 13.0456),
    ("Groningen", 53.2105, 6.5641, "Innsbruck", 47.2632, 11.4009),
];

fn main() {
    let mut arguments = std::env::args().skip(1);
    let bundle_path = arguments.next().expect("pass the bundle path");
    let date: NaiveDate = arguments
        .next()
        .expect("pass a travel date")
        .parse()
        .expect("date as YYYY-MM-DD");

    let custom_arguments: Vec<f64> = arguments
        .map(|argument| argument.parse().expect("coordinates and radius as numbers"))
        .collect();
    let (legs, radius) = match custom_arguments[..] {
        [] => (LEGS.to_vec(), StationSearchRadius::DEFAULT),
        [
            from_latitude,
            from_longitude,
            to_latitude,
            to_longitude,
            radius_meters,
        ] => (
            vec![(
                "custom departure",
                from_latitude,
                from_longitude,
                "custom arrival",
                to_latitude,
                to_longitude,
            )],
            // The bounded constructor rejects radii outside 100 m to 50 km.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            StationSearchRadius::new(radius_meters as u16).expect("radius from 100 m to 50 km"),
        ),
        _ => panic!("pass no custom leg or all five values of one"),
    };

    let started = Instant::now();
    let finder = Arc::new(TimeZoneFinder::new());
    let router = TimetableBundleRouter::open(bundle_path.as_ref(), finder).expect("bundle loads");
    println!(
        "loaded bundle {} in {:?}",
        router.bundle_version(),
        started.elapsed()
    );

    let time = |hour| LocalTimeOfDay::from_hour_and_minute(hour, 0).unwrap();
    for (from_name, from_latitude, from_longitude, to_name, to_latitude, to_longitude) in legs {
        let endpoint = |latitude, longitude| LegEndpoint {
            coordinates: GeoCoordinates::from_degrees(latitude, longitude).unwrap(),
            station_search_radius: radius,
            time_window: DailyTimeWindow::from_start_and_end(time(7), time(20)),
        };
        let input = LegPlanningInput {
            departure: endpoint(from_latitude, from_longitude),
            arrival: endpoint(to_latitude, to_longitude),
            departure_date: date,
            travel_mode: LegTravelMode::Daytime,
            minimum_transfer_time: DurationMinutes(10),
            pass_policy: InterrailPassPolicy::PreferPass,
            transfer_aversion: TransferAversion::new(50).unwrap(),
            chosen_journey: None,
        };
        let started = Instant::now();
        let result = router.plan_leg(&input, || false);
        let elapsed = started.elapsed();
        println!(
            "\n{from_name} to {to_name}, leaving between 07:00 and 20:00, searched in {elapsed:?}"
        );
        match result {
            LegPlanningResult::NoJourneyFound(reason) => println!("  no journey: {reason:?}"),
            LegPlanningResult::Found(plan) => print_plan(&plan),
        }
    }
}

/// Prints the chosen journey of a leg plan, marked with a star, and its alternatives.
fn print_plan(plan: &LegPlan) {
    let station =
        |index: LegPlanStationIndex| plan.station_table[usize::from(index.0)].name.as_str();
    let clock = |minutes: u32| format!("{:02}:{:02}", (minutes / 60) % 24, minutes % 60);
    let journeys = std::iter::once(&plan.chosen_journey).chain(&plan.alternative_journeys);
    for (index, journey) in journeys.enumerate() {
        let rides: Vec<String> = journey
            .rides()
            .map(|ride| {
                format!(
                    "{} {} {} {} {} {}",
                    clock(ride.departure_time.0),
                    station(ride.boarding_station),
                    ride.train_label.category,
                    ride.train_label.number,
                    clock(ride.arrival_time.0),
                    station(ride.alighting_station),
                )
            })
            .collect();
        let marker = if index == 0 { "*" } else { " " };
        let footpath = journey.arrival_footpath.map_or(String::new(), |footpath| {
            format!(
                "  >  {} min to {}",
                footpath.way.duration.0,
                station(footpath.to_station)
            )
        });
        println!(
            "  {marker} {} min access  >  {}{footpath}  >  {} min egress (times in UTC)",
            journey.access.duration.0,
            rides.join("  >  "),
            journey.egress.duration.0,
        );
    }
}
