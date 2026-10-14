//! Tests of the link that carries a snapshot: round trips, damaged links and sizes.

mod common;

use btt_model::{
    GeoCoordinates, LegPlanStation, LegPlanningResult, SNAPSHOT_SCHEMA_VERSION, SnapshotLinkError,
    StoredSnapshot, TripSnapshot, decode_snapshot_link, encode_snapshot_link,
};
use common::{demo_snapshot, stress_snapshot};

#[test]
fn a_snapshot_decodes_to_the_snapshot_that_was_encoded() {
    for snapshot in [demo_snapshot(), stress_snapshot()] {
        let link = encode_snapshot_link(&snapshot).unwrap();
        assert_eq!(decode_snapshot_link(&link.payload).unwrap(), snapshot);
    }
}

#[test]
fn the_fragment_may_start_with_the_hash_sign() {
    let snapshot = demo_snapshot();
    let link = encode_snapshot_link(&snapshot).unwrap();
    let with_hash = format!("#{}", link.payload);
    assert_eq!(decode_snapshot_link(&with_hash).unwrap(), snapshot);
}

#[test]
fn the_payload_uses_only_characters_a_fragment_allows() {
    let link = encode_snapshot_link(&demo_snapshot()).unwrap();
    let allowed = |character: char| {
        character.is_ascii_alphanumeric() || "-._~!$&()*+,;=:@".contains(character)
    };
    assert!(link.payload.chars().all(allowed), "{}", link.payload);
}

#[test]
fn a_snapshot_survives_the_json_of_the_file() {
    let snapshot = demo_snapshot();
    let stored = StoredSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        snapshot: snapshot.clone(),
    };
    let json = serde_json::to_string(&stored).unwrap();
    let parsed: StoredSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.snapshot, snapshot);
}

#[test]
fn the_snapshot_holds_nothing_about_travellers() {
    let snapshot = demo_snapshot();
    assert!(snapshot.trip.preferences.traveller_ids.is_empty());
    assert!(snapshot.trip.chosen_journeys.is_empty());
    assert!(snapshot.trip.id.0.is_nil());
}

#[test]
fn a_link_with_cut_characters_is_recognised_as_cut() {
    let link = encode_snapshot_link(&demo_snapshot()).unwrap().payload;
    for cut in 1..=4 {
        let shortened = &link[..link.len() - cut];
        assert_eq!(
            decode_snapshot_link(shortened),
            Err(SnapshotLinkError::Cut),
            "{cut} characters cut"
        );
    }
    // A messenger that treats the end of the link as punctuation drops it.
    for punctuation in [".", ",", ")", ";", ":", "!"] {
        let extended = format!("{link}{punctuation}");
        assert!(decode_snapshot_link(&extended).is_err());
    }
}

#[test]
fn a_changed_character_is_recognised() {
    let link = encode_snapshot_link(&demo_snapshot()).unwrap().payload;
    let mut changed: Vec<char> = link.chars().collect();
    let middle = changed.len() / 2;
    changed[middle] = if changed[middle] == 'x' { 'y' } else { 'x' };
    let changed: String = changed.into_iter().collect();
    assert_eq!(decode_snapshot_link(&changed), Err(SnapshotLinkError::Cut));
}

#[test]
fn text_that_is_no_payload_fails_without_panicking() {
    for text in [
        "",
        "a",
        "abc",
        "###",
        "ünïcode",
        "a~~~~~~~~~~~~",
        &"a".repeat(40_000),
    ] {
        assert!(decode_snapshot_link(text).is_err(), "{text:?}");
    }
}

/// Prints the sizes the plan sets its thresholds from. Run with `--nocapture` to read them.
#[test]
fn the_links_stay_small() {
    let demo = encode_snapshot_link(&demo_snapshot()).unwrap();
    let stress = encode_snapshot_link(&stress_snapshot()).unwrap();
    println!(
        "demo: {} compressed bytes, {} link characters",
        demo.compressed_bytes,
        demo.payload.len()
    );
    println!(
        "stress: {} compressed bytes, {} link characters",
        stress.compressed_bytes,
        stress.payload.len()
    );
    assert!(demo.payload.len() < 1500, "{}", demo.payload.len());
    assert!(stress.payload.len() < 8000, "{}", stress.payload.len());
}

#[test]
fn stations_that_no_journey_uses_stay_out_of_the_snapshot() {
    let snapshot = demo_snapshot();
    let mut legs = snapshot.legs.clone();
    if let LegPlanningResult::Found(plan) = &mut legs[0].result {
        // Two stations the planner considered near a place, one of them a repeat of a used one.
        plan.station_table.push(LegPlanStation {
            name: "Köln Messe/Deutz".into(),
            coordinates: GeoCoordinates::from_degrees(50.9413, 6.9744).unwrap(),
            time_zone: "Europe/Berlin".into(),
        });
        let repeat = plan.station_table[0].clone();
        plan.station_table.push(repeat);
    }
    let rebuilt = TripSnapshot::new(
        &snapshot.trip,
        &legs,
        snapshot.planned_at,
        snapshot.bundle_version,
    );
    assert_eq!(rebuilt, snapshot);
}

#[test]
fn a_snapshot_of_the_chosen_journeys_is_smaller_and_still_reads() {
    let full = demo_snapshot();
    let chosen = full.clone().chosen_journeys_only();
    for leg in &chosen.legs {
        if let LegPlanningResult::Found(plan) = &leg.result {
            assert!(plan.alternative_journeys.is_empty());
        }
    }
    let full_link = encode_snapshot_link(&full).unwrap();
    let chosen_link = encode_snapshot_link(&chosen).unwrap();
    assert!(chosen_link.payload.len() < full_link.payload.len());
    assert_eq!(decode_snapshot_link(&chosen_link.payload).unwrap(), chosen);
}
