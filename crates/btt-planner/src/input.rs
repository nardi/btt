//! What each leg of a trip needs for planning, and a cache key over it.

use btt_model::{
    DailyTimeWindow, DurationMinutes, GeoCoordinates, InterrailPassPolicy, JourneyChoice,
    LegTravelMode, StationSearchRadius, TransferAversion, Trip, TripPlace, TripPlaceId,
};
use chrono::NaiveDate;
use serde::Serialize;
use smallvec::SmallVec;

/// One end of a leg, with the time window that applies at that end.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct LegEndpoint {
    /// Point the traveller leaves from or travels to.
    pub coordinates: GeoCoordinates,
    /// Largest distance to a station the planner may use.
    pub station_search_radius: StationSearchRadius,
    /// Local-time window for departing from or arriving at this end.
    pub time_window: DailyTimeWindow,
}

/// Everything that affects the result of planning one leg, and nothing else.
///
/// Two legs with equal inputs get the same result, which is what the cache relies on. Names of
/// places are left out for that reason.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct LegPlanningInput {
    /// Where and when the leg starts.
    pub departure: LegEndpoint,
    /// Where and when the leg ends.
    pub arrival: LegEndpoint,
    /// Local date on which the leg departs.
    pub departure_date: NaiveDate,
    /// Whether the leg includes a night train.
    pub travel_mode: LegTravelMode,
    /// Shortest time allowed between two trains.
    pub minimum_transfer_time: DurationMinutes,
    /// How the planner treats trains the pass does not cover.
    pub pass_policy: InterrailPassPolicy,
    /// Trade-off between travel time and transfers when picking a journey.
    pub transfer_aversion: TransferAversion,
    /// The journey the traveller picked for this leg, which the planner shows when it can.
    pub chosen_journey: Option<JourneyChoice>,
}

/// Identifies a leg result in the cache. Covers the leg input and the backend that planned it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LegCacheKey(pub u64);

impl LegPlanningInput {
    /// Returns the cache key of this input for a backend.
    ///
    /// The key hashes the `postcard` encoding with `xxh3`, which gives the same value on every
    /// run and every machine, so it can go into persistent storage. `backend_namespace` separates
    /// results of different backends and timetable bundle versions.
    pub fn cache_key(&self, backend_namespace: u64) -> LegCacheKey {
        // Every field is a plain value that postcard encodes without failing.
        let encoded = postcard::to_stdvec(self).unwrap_or_default();
        LegCacheKey(xxhash_rust::xxh3::xxh3_64_with_seed(
            &encoded,
            backend_namespace,
        ))
    }
}

/// A leg planning input together with the place that identifies its leg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentifiedLegInput {
    /// Place at which the leg arrives.
    pub arrival_place_id: TripPlaceId,
    /// What the leg needs for planning.
    pub input: LegPlanningInput,
}

/// Returns the planning input of every leg of a trip, in travel order.
pub fn leg_planning_inputs(trip: &Trip) -> SmallVec<[IdentifiedLegInput; 16]> {
    let preferences = &trip.preferences;
    let departure_dates = trip.itinerary.leg_departure_dates(trip.start_date);
    let windows_of = |place: &TripPlace| place.windows(preferences.default_windows);
    trip.itinerary
        .legs()
        .zip(departure_dates)
        .map(|(leg, departure_date)| IdentifiedLegInput {
            arrival_place_id: leg.arrival_place.id,
            input: LegPlanningInput {
                departure: LegEndpoint {
                    coordinates: leg.departure_place.coordinates,
                    station_search_radius: leg.departure_place.station_search_radius,
                    time_window: windows_of(leg.departure_place).departure,
                },
                arrival: LegEndpoint {
                    coordinates: leg.arrival_place.coordinates,
                    station_search_radius: leg.arrival_place.station_search_radius,
                    time_window: windows_of(leg.arrival_place).arrival,
                },
                departure_date,
                travel_mode: *leg.travel_mode,
                minimum_transfer_time: preferences.minimum_transfer_time,
                pass_policy: preferences.pass_policy,
                transfer_aversion: preferences.transfer_aversion,
                chosen_journey: trip
                    .chosen_journeys
                    .iter()
                    .find(|chosen| {
                        chosen.departure_place_id == leg.departure_place.id
                            && chosen.arrival_place_id == leg.arrival_place.id
                    })
                    .map(|chosen| chosen.choice.clone()),
            },
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use btt_model::{PlaceInsertionPoint, PlaceStay, TripEdit, edit::NewTripPlace};

    use super::*;

    pub(crate) fn trip_with_places(places: &[(&str, f64, f64)]) -> Trip {
        let start_date = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let mut trip = Trip::new("Test".into(), start_date);
        for (name, latitude, longitude) in places {
            let index = u16::try_from(trip.itinerary.place_count()).unwrap();
            trip.apply_edit(TripEdit::AddPlace {
                place: NewTripPlace {
                    name: (*name).into(),
                    coordinates: GeoCoordinates::from_degrees(*latitude, *longitude).unwrap(),
                    station_search_radius: StationSearchRadius::DEFAULT,
                },
                insert_at: PlaceInsertionPoint::AtIndex(index),
            })
            .unwrap();
        }
        trip
    }

    pub(crate) const FOUR_CITIES: [(&str, f64, f64); 4] = [
        ("Amsterdam", 52.378_9, 4.900_3),
        ("Berlin", 52.525_1, 13.369_4),
        ("Vienna", 48.185_2, 16.376_1),
        ("Rome", 41.901_0, 12.500_7),
    ];

    pub(crate) fn place_id(trip: &Trip, index: usize) -> TripPlaceId {
        trip.itinerary.trip_places().nth(index).unwrap().id
    }

    fn cache_keys(trip: &Trip) -> Vec<LegCacheKey> {
        leg_planning_inputs(trip)
            .iter()
            .map(|leg| leg.input.cache_key(1))
            .collect()
    }

    #[test]
    fn renaming_a_place_keeps_every_key() {
        let mut trip = trip_with_places(&FOUR_CITIES);
        let before = cache_keys(&trip);
        trip.apply_edit(TripEdit::RenamePlace {
            place_id: place_id(&trip, 1),
            name: "Berlin Hbf".into(),
        })
        .unwrap();
        assert_eq!(cache_keys(&trip), before);
    }

    #[test]
    fn choosing_a_journey_changes_only_its_leg() {
        let mut trip = trip_with_places(&FOUR_CITIES);
        let before = cache_keys(&trip);
        let choice = btt_model::JourneyChoice {
            first_departure_time: btt_model::UnixTimeMinutes(29_000_000),
            trains: smallvec::smallvec![btt_model::TrainLabel {
                category: "ICE".into(),
                number: "145".into(),
            }],
        };
        trip.apply_edit(TripEdit::ChooseJourney {
            arrival_place_id: place_id(&trip, 1),
            choice: Some(choice.clone()),
        })
        .unwrap();
        let inputs = leg_planning_inputs(&trip);
        assert_eq!(inputs[0].input.chosen_journey, Some(choice));
        let after = cache_keys(&trip);
        assert_ne!(after[0], before[0]);
        assert_eq!(after[1..], before[1..]);
    }

    #[test]
    fn moving_a_place_changes_its_two_legs() {
        let mut trip = trip_with_places(&FOUR_CITIES);
        let before = cache_keys(&trip);
        let potsdam = GeoCoordinates::from_degrees(52.391_3, 13.066_8).unwrap();
        trip.apply_edit(TripEdit::MovePlace {
            place_id: place_id(&trip, 1),
            coordinates: potsdam,
        })
        .unwrap();
        let after = cache_keys(&trip);
        assert_ne!(after[0], before[0]);
        assert_ne!(after[1], before[1]);
        assert_eq!(after[2], before[2]);
    }

    #[test]
    fn changing_nights_changes_later_legs_only() {
        let mut trip = trip_with_places(&FOUR_CITIES);
        let before = cache_keys(&trip);
        let two_nights = PlaceStay::Nights(2.try_into().unwrap());
        trip.apply_edit(TripEdit::SetPlaceStay {
            place_id: place_id(&trip, 1),
            stay: two_nights,
        })
        .unwrap();
        let after = cache_keys(&trip);
        assert_eq!(after[0], before[0]);
        assert_ne!(after[1], before[1]);
        assert_ne!(after[2], before[2]);
    }

    #[test]
    fn backend_namespace_separates_keys() {
        let trip = trip_with_places(&FOUR_CITIES);
        let input = &leg_planning_inputs(&trip)[0].input;
        assert_ne!(input.cache_key(1), input.cache_key(2));
    }
}
