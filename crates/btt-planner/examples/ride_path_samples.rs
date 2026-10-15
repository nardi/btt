//! Writes the rides of real legs together with the stations the desktop map draws them through,
//! for the tests of the web viewer.
//!
//! Run with `cargo run --release -p btt-planner --example ride_path_samples -- <bundle> <file>`.
//! The file is a JSON list with one entry per distinct ride of the chosen journey and the
//! alternatives of each leg below: the category of the train, the positions of the boarding and
//! the alighting station, and the positions of the points of the line, as latitude and longitude
//! in degrees. The viewer finds its lines from the two stations and the category alone, and its
//! tests compare them with the lines in this file.

use std::sync::Arc;

use btt_geo::TimeZoneFinder;
use btt_model::{
    DailyTimeWindow, DurationMinutes, GeoCoordinates, InterrailPassPolicy, LegPlanStationIndex,
    LegPlanningResult, LegTravelMode, LocalTimeOfDay, PriceWeight, StationSearchRadius,
    TransferAversion,
};
use btt_planner::{LegEndpoint, LegPlanningInput, NoPrice, TimetableBundleRouter};
use chrono::NaiveDate;

/// A leg as the latitude and longitude of its departure place and of its arrival place.
type Leg = (f64, f64, f64, f64);

/// Legs across Europe, between cities that long-distance trains connect, and a few regional ones.
const LEGS: [Leg; 28] = [
    // Amsterdam to Berlin, Utrecht to München, Köln to Wien, Hamburg to Salzburg.
    (52.3789, 4.9003, 52.5251, 13.3694),
    (52.0894, 5.1101, 48.1402, 11.5586),
    (50.9430, 6.9589, 48.1852, 16.3761),
    (53.5528, 10.0067, 47.8129, 13.0456),
    // Paris to Barcelona, Madrid to Sevilla, Roma to Milano, Zürich to Wien.
    (48.8809, 2.3553, 41.3795, 2.1404),
    (40.4065, -3.6892, 37.3922, -5.9755),
    (41.9009, 12.5017, 45.4862, 9.2046),
    (47.3779, 8.5403, 48.1852, 16.3761),
    // Stockholm to København, Warszawa to Praha, Budapest to Wien, Brussels to Paris.
    (59.3303, 18.0586, 55.6727, 12.5648),
    (52.2289, 21.0034, 50.0833, 14.4353),
    (47.5003, 19.0839, 48.1852, 16.3761),
    (50.8359, 4.3361, 48.8809, 2.3553),
    // Lisboa to Porto, Bern to Interlaken, Oslo to Bergen, Venezia to Napoli.
    (38.7678, -9.1000, 41.1456, -8.5853),
    (46.9488, 7.4392, 46.6903, 7.8689),
    (59.9109, 10.7527, 60.3906, 5.3341),
    (45.4410, 12.3212, 40.8531, 14.2722),
    // Berlin to Praha, München to Venezia, Wien to Zagreb, Frankfurt to Basel.
    (52.5251, 13.3694, 50.0833, 14.4353),
    (48.1402, 11.5586, 45.4410, 12.3212),
    (48.1852, 16.3761, 45.8049, 15.9786),
    (50.1071, 8.6638, 47.5476, 7.5896),
    // Köln to Amsterdam, Hannover to Dresden, Stuttgart to Genève, Graz to Linz.
    (50.9430, 6.9589, 52.3789, 4.9003),
    (52.3765, 9.7410, 51.0402, 13.7320),
    (48.7840, 9.1816, 46.2102, 6.1424),
    (47.0722, 15.4170, 48.2900, 14.2917),
    // Köln to Düsseldorf, Wien to Salzburg, Utrecht to Arnhem, Torino to Firenze.
    (50.9430, 6.9589, 51.2200, 6.7940),
    (48.1852, 16.3761, 47.8129, 13.0456),
    (52.0894, 5.1101, 51.9850, 5.8980),
    (45.0703, 7.6869, 43.7764, 11.2478),
];

/// Returns a position as latitude and longitude in degrees.
fn degrees(coordinates: GeoCoordinates) -> [f64; 2] {
    [
        coordinates.latitude.degrees(),
        coordinates.longitude.degrees(),
    ]
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let bundle_path = arguments.next().expect("pass the bundle");
    let output_path = arguments.next().expect("pass the file to write");
    let router = TimetableBundleRouter::open(
        bundle_path.as_ref(),
        Arc::new(TimeZoneFinder::new()),
        NoPrice,
    )
    .expect("the bundle opens");
    let time = |hour| LocalTimeOfDay::from_hour_and_minute(hour, 0).expect("the hour is valid");
    let endpoint = |latitude, longitude| LegEndpoint {
        coordinates: GeoCoordinates::from_degrees(latitude, longitude)
            .expect("the coordinates are on Earth"),
        station_search_radius: StationSearchRadius::DEFAULT,
        time_window: DailyTimeWindow::from_start_and_end(time(7), time(20)),
    };
    let mut rides: Vec<serde_json::Value> = Vec::new();
    for (from_latitude, from_longitude, to_latitude, to_longitude) in LEGS {
        let input = LegPlanningInput {
            departure: endpoint(from_latitude, from_longitude),
            arrival: endpoint(to_latitude, to_longitude),
            departure_date: NaiveDate::from_ymd_opt(2026, 10, 20).expect("the date is valid"),
            travel_mode: LegTravelMode::Daytime,
            minimum_transfer_time: DurationMinutes(10),
            pass_policy: InterrailPassPolicy::PreferPass,
            transfer_aversion: TransferAversion::new(50).expect("the aversion is in range"),
            price_weight: PriceWeight::TIME_FIRST,
            chosen_journey: None,
        };
        let LegPlanningResult::Found(plan) = router.plan_leg_with_extras(&input, || false) else {
            continue;
        };
        let journeys = std::iter::once(&plan.chosen_journey).chain(&plan.alternative_journeys);
        for ride in journeys.flat_map(btt_model::Journey::rides) {
            let station =
                |index: LegPlanStationIndex| plan.station_table[usize::from(index.0)].coordinates;
            let entry = serde_json::json!({
                "category": ride.train_label.category,
                "from": degrees(station(ride.boarding_station)),
                "to": degrees(station(ride.alighting_station)),
                "path": ride.path_coordinates.iter().copied().map(degrees).collect::<Vec<_>>(),
            });
            if !rides.contains(&entry) {
                rides.push(entry);
            }
        }
    }
    std::fs::write(
        &output_path,
        serde_json::to_string(&rides).expect("the rides are JSON") + "\n",
    )
    .expect("the rides are written");
    println!("{} rides", rides.len());
}
