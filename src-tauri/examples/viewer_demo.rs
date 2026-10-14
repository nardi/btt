//! Plans a demo trip on a timetable bundle and writes what the web viewer needs to show it: the
//! link, the trip file and the list of feeds for its credits page.
//!
//! Run with `cargo run --release -p btt-app --example viewer_demo -- <bundle> <output folder>
//! [start date] [viewer address]`. The start date defaults to a week after the bundle's
//! planning day, and the viewer address to the preview server of the viewer.

use std::num::{NonZeroU8, NonZeroU32};
use std::path::Path;
use std::sync::Arc;

use btt_app_lib::pricing::PriceBookJourneyPricer;
use btt_fares::{FareData, PriceBook};
use btt_geo::TimeZoneFinder;
use btt_model::{
    GeoCoordinates, Itinerary, ItineraryRoute, LegPlanningResult, LegTravelMode, PlaceStay,
    PriceWeight, SNAPSHOT_SCHEMA_VERSION, SnapshotLeg, StationSearchRadius, StoredSnapshot, Trip,
    TripPlace, TripPlaceId, TripSnapshot, ViaPlace, encode_snapshot_link,
};
use btt_planner::{TimetableBundleRouter, leg_planning_inputs};
use btt_timetable::LoadedTimetableBundle;
use chrono::NaiveDate;

/// Address the preview server of the viewer serves the built site at.
const DEFAULT_VIEWER_ADDRESS: &str = "http://127.0.0.1:4173/btt/";

/// Returns a place at a position, which the planner looks for stations around.
fn place(id: u32, name: &str, latitude: f64, longitude: f64) -> TripPlace {
    TripPlace {
        id: TripPlaceId(NonZeroU32::new(id).expect("place IDs are not zero")),
        name: name.into(),
        coordinates: GeoCoordinates::from_degrees(latitude, longitude)
            .expect("the coordinates are on Earth"),
        station_search_radius: StationSearchRadius::DEFAULT,
        window_override: None,
    }
}

/// Returns the demo trip: Amsterdam, a night in Zürich, two in Vienna and a night train to Rome.
fn demo_trip(start_date: NaiveDate) -> Trip {
    let nights = |count: u8| PlaceStay::Nights(NonZeroU8::new(count).expect("at least one night"));
    let mut trip = Trip::new("Amsterdam to Rome".into(), start_date);
    trip.preferences.price_weight = PriceWeight::BALANCED;
    trip.itinerary = Itinerary::Route(ItineraryRoute {
        origin: place(1, "Amsterdam", 52.3789, 4.9003),
        via_places: vec![
            ViaPlace {
                arriving_leg_travel_mode: LegTravelMode::Daytime,
                place: place(2, "Zürich", 47.3779, 8.5403),
                stay: nights(1),
            },
            ViaPlace {
                arriving_leg_travel_mode: LegTravelMode::Daytime,
                place: place(3, "Wien", 48.1852, 16.3761),
                stay: nights(2),
            },
        ],
        final_leg_travel_mode: LegTravelMode::Overnight(btt_model::OvernightLegOptions::DEFAULT),
        destination: place(4, "Roma", 41.901, 12.5007),
    });
    trip.next_place_id = NonZeroU32::new(5).expect("five is not zero");
    trip
}

/// Writes the feeds of a bundle as the JSON the credits page reads.
fn write_credits(bundle_path: &Path, output: &Path) {
    let bundle = LoadedTimetableBundle::open(bundle_path).expect("the bundle opens");
    let mut feeds: Vec<serde_json::Value> = bundle
        .feed_attributions()
        .map(|(name, license, url)| {
            serde_json::json!({ "name": name, "license": license, "url": url })
        })
        .collect();
    feeds.sort_by_key(|feed| feed["name"].as_str().unwrap_or_default().to_owned());
    let document = serde_json::json!({
        "bundle_version": bundle.bundle_version(),
        "feeds": feeds,
    });
    let text = serde_json::to_string_pretty(&document).expect("the credits are JSON");
    std::fs::write(output.join("credits.json"), text + "\n").expect("the credits are written");
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let bundle_path = arguments.next().expect("pass the bundle");
    let output = arguments.next().expect("pass the output folder");
    let start_date: NaiveDate = arguments
        .next()
        .map_or_else(|| "2026-10-20".parse(), |date| date.parse())
        .expect("the start date is YYYY-MM-DD");
    let viewer_address = arguments
        .next()
        .unwrap_or_else(|| DEFAULT_VIEWER_ADDRESS.to_owned());
    let output = Path::new(&output);
    std::fs::create_dir_all(output).expect("the output folder exists");

    let data = Arc::new(FareData::bundled().expect("the bundled fare data is valid"));
    let book = Arc::new(PriceBook::from_data(&data));
    let pricer = PriceBookJourneyPricer::new(data, book);
    let router = TimetableBundleRouter::open(
        bundle_path.as_ref(),
        Arc::new(TimeZoneFinder::new()),
        pricer,
    )
    .expect("the bundle opens");

    let trip = demo_trip(start_date);
    let mut legs = Vec::new();
    for leg in leg_planning_inputs(&trip) {
        let result = router.plan_leg_with_extras(&leg.input, || false);
        match &result {
            LegPlanningResult::Found(plan) => println!(
                "leg to place {}: {} alternatives, {} stations",
                leg.arrival_place_id.0,
                plan.alternative_journeys.len(),
                plan.station_table.len()
            ),
            LegPlanningResult::NoJourneyFound(reason) => {
                println!(
                    "leg to place {}: no journey, {reason:?}",
                    leg.arrival_place_id.0
                );
            }
        }
        legs.push(SnapshotLeg {
            arrival_place_id: leg.arrival_place_id,
            result,
        });
    }

    let planned_at = chrono::Local::now().date_naive();
    let snapshot = TripSnapshot::new(&trip, &legs, planned_at, router.bundle_version());
    let link = encode_snapshot_link(&snapshot).expect("the demo trip fits in a link");
    println!(
        "link: {} characters from {} compressed bytes",
        link.payload.len(),
        link.compressed_bytes
    );
    std::fs::write(
        output.join("demo-link.txt"),
        format!("{viewer_address}#{}\n", link.payload),
    )
    .expect("the link is written");
    let document = StoredSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        snapshot,
    };
    std::fs::write(
        output.join("demo.btt"),
        serde_json::to_string(&document).expect("the snapshot is JSON"),
    )
    .expect("the trip file is written");
    write_credits(bundle_path.as_ref(), output);
    println!(
        "wrote the demo link, the trip file and the credits to {}",
        output.display()
    );
}
