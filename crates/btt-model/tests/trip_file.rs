//! Tests of the trip in a `.btt` file, which the app opens as a new trip to edit.

mod common;

use btt_model::{
    Itinerary, LegPlanningResult, SNAPSHOT_SCHEMA_VERSION, StoredSnapshot, TravellerId, Trip,
    TripFileError,
};
use common::{demo_snapshot, stress_snapshot};

/// Returns the demo trip as the app holds it, with an ID and a picked journey.
fn demo_trip() -> Trip {
    let snapshot = StoredSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        snapshot: demo_snapshot(),
        editable_trip: None,
    };
    snapshot.into_editable_trip().unwrap()
}

#[test]
fn a_file_with_the_editable_trip_opens_as_that_trip_with_a_new_id() {
    let trip = demo_trip();
    let document = StoredSnapshot::of_trip(demo_snapshot(), &trip);
    let json = serde_json::to_string(&document).unwrap();
    let parsed: StoredSnapshot = serde_json::from_str(&json).unwrap();
    let opened = parsed.into_editable_trip().unwrap();

    assert_ne!(opened.id, trip.id);
    assert!(!opened.id.0.is_nil());
    assert_eq!(opened.itinerary, trip.itinerary);
    assert_eq!(opened.preferences, trip.preferences);
    assert_eq!(opened.chosen_journeys, trip.chosen_journeys);
}

#[test]
fn the_file_holds_no_trip_id_and_no_travellers() {
    let mut trip = demo_trip();
    trip.preferences
        .traveller_ids
        .push(TravellerId(uuid::Uuid::max()));
    let document = StoredSnapshot::of_trip(demo_snapshot(), &trip);
    let editable_trip = document.editable_trip.unwrap();
    assert!(editable_trip.id.0.is_nil());
    assert!(editable_trip.preferences.traveller_ids.is_empty());
}

#[test]
fn a_file_without_the_editable_trip_picks_the_planned_journeys() {
    let snapshot = demo_snapshot();
    let found_leg_count = snapshot
        .legs
        .iter()
        .filter(|leg| matches!(leg.result, LegPlanningResult::Found(_)))
        .count();
    assert!(found_leg_count > 0);

    let trip = demo_trip();
    assert_eq!(trip.chosen_journeys.len(), found_leg_count);
    assert_eq!(trip.itinerary, snapshot.trip.itinerary);
}

#[test]
fn a_long_trip_opens_with_every_place() {
    let snapshot = stress_snapshot();
    let place_count = snapshot.trip.itinerary.trip_places().count();
    let document = StoredSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        snapshot,
        editable_trip: None,
    };
    let trip = document.into_editable_trip().unwrap();
    assert_eq!(trip.itinerary.trip_places().count(), place_count);
}

#[test]
fn a_file_from_a_newer_version_is_refused() {
    let document = StoredSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION + 1,
        snapshot: demo_snapshot(),
        editable_trip: None,
    };
    assert_eq!(
        document.into_editable_trip(),
        Err(TripFileError::NewerSchema(SNAPSHOT_SCHEMA_VERSION + 1))
    );
}

#[test]
fn a_file_with_two_places_of_the_same_id_is_refused() {
    let mut snapshot = demo_snapshot();
    let Itinerary::Route(route) = &mut snapshot.trip.itinerary else {
        panic!("The demo trip has a route.");
    };
    route.via_places[0].place.id = route.origin.id;
    let document = StoredSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        snapshot,
        editable_trip: None,
    };
    assert!(matches!(
        document.into_editable_trip(),
        Err(TripFileError::InvalidTrip(_))
    ));
}

#[test]
fn a_new_place_does_not_reuse_the_id_of_a_place_in_the_file() {
    let mut snapshot = demo_snapshot();
    snapshot.trip.next_place_id = std::num::NonZeroU32::MIN;
    let document = StoredSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        snapshot,
        editable_trip: None,
    };
    let trip = document.into_editable_trip().unwrap();
    assert!(
        trip.itinerary
            .trip_places()
            .all(|place| place.id.0 < trip.next_place_id)
    );
}
