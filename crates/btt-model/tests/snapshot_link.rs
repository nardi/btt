//! Tests of the link that carries a snapshot: round trips, damaged links and sizes.

mod common;

use std::fmt::Write as _;

use btt_model::{
    GeoCoordinates, LegPlanStation, LegPlanningResult, LinkAlphabet, SNAPSHOT_SCHEMA_VERSION,
    SnapshotLinkError, StoredSnapshot, TripSnapshot, decode_snapshot_link, encode_snapshot_link,
    encode_snapshot_link_in,
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
    assert_eq!(snapshot.trip.chosen_journeys, Vec::new());
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

#[test]
fn a_link_for_a_qr_code_uses_only_the_characters_of_its_mode() {
    let qr_characters = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ$*+-./:";
    for snapshot in [demo_snapshot(), stress_snapshot()] {
        let link = encode_snapshot_link_in(&snapshot, LinkAlphabet::QrCode).unwrap();
        assert!(
            link.payload
                .chars()
                .all(|character| qr_characters.contains(character)),
            "{}",
            link.payload
        );
        assert_eq!(decode_snapshot_link(&link.payload).unwrap(), snapshot);
    }
}

#[test]
fn a_qr_link_is_shorter_in_bits_than_a_fragment_link_is_in_bytes() {
    // The alphanumeric mode of a QR code spends 5.5 bits on a character, and the byte mode a
    // fragment link needs spends 8.
    let chosen = demo_snapshot().chosen_journeys_only();
    let fragment = encode_snapshot_link(&chosen).unwrap();
    let qr = encode_snapshot_link_in(&chosen, LinkAlphabet::QrCode).unwrap();
    let fragment_bits = fragment.payload.len() * 8;
    let qr_bits = (qr.payload.len() * 11).div_ceil(2);
    assert!(qr_bits < fragment_bits, "{qr_bits} against {fragment_bits}");
}

#[test]
fn a_cut_qr_link_is_recognised_as_cut() {
    let link = encode_snapshot_link_in(&demo_snapshot(), LinkAlphabet::QrCode)
        .unwrap()
        .payload;
    assert_eq!(
        decode_snapshot_link(&link[..link.len() - 1]),
        Err(SnapshotLinkError::Cut)
    );
}

#[test]
fn a_dictionary_station_costs_less_than_a_literal_one() {
    // The demo trip has three stations from the dictionary and one that the dictionary lacks.
    let with_dictionary = encode_snapshot_link(&demo_snapshot()).unwrap();
    let mut literal = demo_snapshot();
    for leg in &mut literal.legs {
        if let LegPlanningResult::Found(plan) = &mut leg.result {
            for station in &mut plan.station_table {
                // A name that no dictionary has, so the station travels as a literal.
                station.name = format!("{} (test)", station.name).into();
            }
        }
    }
    let literal_link = encode_snapshot_link(&literal).unwrap();
    assert!(with_dictionary.compressed_bytes + 50 < literal_link.compressed_bytes);
    assert_eq!(
        decode_snapshot_link(&literal_link.payload).unwrap(),
        literal
    );
}

#[test]
fn places_sit_on_a_grid_of_eleven_meters() {
    for snapshot in [demo_snapshot(), stress_snapshot()] {
        for place in snapshot.trip.itinerary.trip_places() {
            assert_eq!(place.coordinates.latitude.microdegrees() % 100, 0);
            assert_eq!(place.coordinates.longitude.microdegrees() % 100, 0);
        }
    }
}

#[test]
fn a_place_with_the_name_of_a_city_costs_less_than_one_without() {
    let snapshot = demo_snapshot();
    let with_cities = encode_snapshot_link(&snapshot).unwrap();
    // The demo trip names Amsterdam, Vienna and Rome, which the city dictionary has, and a place
    // that it lacks. A suffix takes the cities out of the dictionary.
    let mut renamed = snapshot.clone();
    if let btt_model::Itinerary::Route(route) = &mut renamed.trip.itinerary {
        route.origin.name = format!("{} (test)", route.origin.name).into();
        for via_place in &mut route.via_places {
            via_place.place.name = format!("{} (test)", via_place.place.name).into();
        }
    }
    let literal = encode_snapshot_link(&renamed).unwrap();
    assert!(with_cities.compressed_bytes + 20 < literal.compressed_bytes);
    assert_eq!(decode_snapshot_link(&literal.payload).unwrap(), renamed);
}

#[test]
fn a_compact_link_has_far_fewer_characters_and_reads_back() {
    for snapshot in [demo_snapshot(), stress_snapshot()] {
        let fragment = encode_snapshot_link(&snapshot).unwrap();
        let compact = encode_snapshot_link_in(&snapshot, LinkAlphabet::Compact).unwrap();
        assert!(compact.payload.chars().count() * 2 < fragment.payload.len());
        // Every letter but the marker and the check characters is one UTF-16 unit.
        assert_eq!(
            compact.payload.encode_utf16().count(),
            compact.payload.chars().count()
        );
        assert_eq!(decode_snapshot_link(&compact.payload).unwrap(), snapshot);
    }
}

#[test]
fn a_compact_link_reads_back_as_a_browser_keeps_it() {
    let snapshot = demo_snapshot();
    let compact = encode_snapshot_link_in(&snapshot, LinkAlphabet::Compact).unwrap();
    let mut percent_encoded = String::new();
    for character in compact.payload.chars() {
        if character.is_ascii() {
            percent_encoded.push(character);
        } else {
            let mut buffer = [0; 4];
            for byte in character.encode_utf8(&mut buffer).bytes() {
                write!(percent_encoded, "%{byte:02X}").unwrap();
            }
        }
    }
    assert!(percent_encoded.len() > compact.payload.len());
    assert_eq!(decode_snapshot_link(&percent_encoded).unwrap(), snapshot);
}

#[test]
fn a_cut_or_damaged_compact_link_is_recognised() {
    let link = encode_snapshot_link_in(&demo_snapshot(), LinkAlphabet::Compact)
        .unwrap()
        .payload;
    let characters: Vec<char> = link.chars().collect();
    for cut in 1..=3 {
        let shortened: String = characters[..characters.len() - cut].iter().collect();
        assert_eq!(
            decode_snapshot_link(&shortened),
            Err(SnapshotLinkError::Cut)
        );
    }
    // A letter that is no letter of the alphabet is no payload, and does not panic.
    let mut changed = characters.clone();
    changed[10] = 'x';
    let changed: String = changed.into_iter().collect();
    assert!(decode_snapshot_link(&changed).is_err());
    assert!(decode_snapshot_link("c%E4%BA").is_err());
    assert!(decode_snapshot_link("c%ZZZZ").is_err());
}
