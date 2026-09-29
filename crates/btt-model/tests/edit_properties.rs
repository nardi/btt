//! Property tests that apply random sequences of edits to a trip.

use std::collections::HashSet;
use std::num::NonZeroU8;

use btt_model::edit::NewTripPlace;
use btt_model::{
    GeoCoordinates, Itinerary, LegTravelMode, OvernightLegOptions, PlaceInsertionPoint, PlaceStay,
    StationSearchRadius, Trip, TripEdit,
};
use chrono::NaiveDate;
use proptest::prelude::*;

/// An edit that refers to places by position, so it stays meaningful for any itinerary.
#[derive(Debug, Clone, Copy)]
enum PositionalEdit {
    Add {
        latitude: f64,
        longitude: f64,
        index: Option<usize>,
    },
    Remove {
        position: usize,
    },
    Reorder {
        position: usize,
        new_index: usize,
    },
    SetNights {
        position: usize,
        nights: u8,
    },
    ToggleOvernight {
        position: usize,
    },
}

fn positional_edit() -> impl Strategy<Value = PositionalEdit> {
    prop_oneof![
        3 => (36.0..60.0f64, -5.0..25.0f64, proptest::option::of(0..12usize)).prop_map(
            |(latitude, longitude, index)| PositionalEdit::Add { latitude, longitude, index }
        ),
        1 => (0..12usize).prop_map(|position| PositionalEdit::Remove { position }),
        1 => (0..12usize, 0..12usize)
            .prop_map(|(position, new_index)| PositionalEdit::Reorder { position, new_index }),
        1 => (0..12usize, 0..4u8)
            .prop_map(|(position, nights)| PositionalEdit::SetNights { position, nights }),
        1 => (0..12usize).prop_map(|position| PositionalEdit::ToggleOvernight { position }),
    ]
}

fn overnight() -> LegTravelMode {
    LegTravelMode::Overnight(OvernightLegOptions::DEFAULT)
}

/// Turns a positional edit into a trip edit for the current itinerary.
///
/// Positions wrap around the place count, so most edits hit an existing place. Indices are not
/// wrapped, so some edits test the out-of-range errors. On an empty itinerary every edit other
/// than an addition returns `None`.
fn to_trip_edit(trip: &Trip, edit: PositionalEdit) -> Option<TripEdit> {
    let place_ids: Vec<_> = trip.itinerary.trip_places().map(|place| place.id).collect();
    let as_u16 = |value: usize| u16::try_from(value).unwrap();
    if let PositionalEdit::Add {
        latitude,
        longitude,
        index,
    } = edit
    {
        return Some(TripEdit::AddPlace {
            place: NewTripPlace {
                name: "Place".into(),
                coordinates: GeoCoordinates::from_degrees(latitude, longitude).unwrap(),
                station_search_radius: StationSearchRadius::DEFAULT,
            },
            insert_at: match index {
                Some(index) => PlaceInsertionPoint::AtIndex(as_u16(index)),
                None => PlaceInsertionPoint::LeastAddedDistance,
            },
        });
    }
    if place_ids.is_empty() {
        return None;
    }
    let place_at = |position: usize| place_ids[position % place_ids.len()];
    Some(match edit {
        PositionalEdit::Add { .. } => unreachable!("additions return above"),
        PositionalEdit::Remove { position } => TripEdit::RemovePlace(place_at(position)),
        PositionalEdit::Reorder {
            position,
            new_index,
        } => TripEdit::ReorderPlace {
            place_id: place_at(position),
            new_index: as_u16(new_index),
        },
        PositionalEdit::SetNights { position, nights } => TripEdit::SetPlaceStay {
            place_id: place_at(position),
            stay: NonZeroU8::new(nights).map_or(
                PlaceStay::SameDay {
                    stay_range_override: None,
                    prefers_longer_stay: false,
                },
                PlaceStay::Nights,
            ),
        },
        PositionalEdit::ToggleOvernight { position } => {
            let arrival_place_id = place_at(position);
            let is_overnight = trip.itinerary.legs().any(|leg| {
                leg.arrival_place.id == arrival_place_id && leg.travel_mode.is_overnight()
            });
            TripEdit::SetLegTravelMode {
                arrival_place_id,
                travel_mode: if is_overnight {
                    LegTravelMode::Daytime
                } else {
                    overnight()
                },
            }
        }
    })
}

proptest! {
    #[test]
    fn random_edits_keep_the_itinerary_consistent(
        edits in proptest::collection::vec(positional_edit(), 0..40)
    ) {
        let start_date = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let mut trip = Trip::new("Random".into(), start_date);
        for edit in edits {
            let Some(trip_edit) = to_trip_edit(&trip, edit) else { continue };
            let before = trip.clone();
            if trip.apply_edit(trip_edit).is_err() {
                prop_assert_eq!(&trip, &before, "a rejected edit changed the trip");
            }

            let place_count = trip.itinerary.place_count();
            prop_assert_eq!(trip.itinerary.trip_places().count(), place_count);
            prop_assert_eq!(trip.itinerary.legs().count(), place_count.saturating_sub(1));

            let unique_ids: HashSet<_> = trip.itinerary.trip_places().map(|place| place.id).collect();
            prop_assert_eq!(unique_ids.len(), place_count);
            prop_assert!(unique_ids.iter().all(|id| id.0 < trip.next_place_id));

            let dates = trip.itinerary.leg_departure_dates(trip.start_date);
            prop_assert!(dates.windows(2).all(|pair| pair[0] <= pair[1]));
            prop_assert!(dates.first().is_none_or(|first| *first == start_date));

            if let Itinerary::Route(route) = &trip.itinerary {
                prop_assert_eq!(route.via_places.len() + 2, place_count);
            }

            let json = serde_json::to_string(&trip).unwrap();
            let parsed: Trip = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(&parsed, &trip);
        }
    }
}
