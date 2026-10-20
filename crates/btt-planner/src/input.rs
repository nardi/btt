//! What each leg of a trip needs for planning, and a cache key over it.

use btt_model::{
    DailyTimeWindow, DurationMinutes, GeoCoordinates, InterrailPassPolicy, JourneyChoice,
    LegTravelMode, PlaceStay, PlaceWindowOverride, PriceWeight, SameDayStayRange,
    StationSearchRadius, TransferAversion, Trip, TripPlace, TripPlaceId,
};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

/// One end of a leg, with the time window that applies at that end.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// How many minutes of travel time one euro of the journey's price is worth, in hundredths
    /// of a minute. Zero plans by time alone, and a plan at any weight ranks the journeys by
    /// their cost and their price together.
    pub price_weight: PriceWeight,
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
        LegCacheKey(xxhash_rust::xxh3::xxh3_64_with_seed(
            &self.key_bytes(),
            backend_namespace,
        ))
    }

    /// Returns the encoding of this input that the cache key hashes. It lists the fields one by
    /// one, and the price weight only when it is above zero, so results planned without prices
    /// keep the keys they always had.
    fn key_bytes(&self) -> Vec<u8> {
        let parts = (
            &self.departure,
            &self.arrival,
            &self.departure_date,
            &self.travel_mode,
            &self.minimum_transfer_time,
            &self.pass_policy,
            &self.transfer_aversion,
            &self.chosen_journey,
        );
        if self.price_weight.hundredths_of_minute_per_euro() > 0 {
            postcard::to_stdvec(&(parts, self.price_weight)).unwrap_or_default()
        } else {
            postcard::to_stdvec(&parts).unwrap_or_default()
        }
    }
}

/// A leg planning input together with the place that identifies its leg.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
                minimum_transfer_time: trip
                    .minimum_transfer_time_of_leg(leg.departure_place.id, leg.arrival_place.id),
                pass_policy: preferences.pass_policy,
                transfer_aversion: preferences.transfer_aversion,
                price_weight: preferences.price_weight,
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

/// A place with a same-day stay between two legs of a chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SameDayStop {
    /// Shortest and longest stay the traveller wants.
    pub stay_range: SameDayStayRange,
    /// Whether a longer stay within the range is better.
    pub prefers_longer_stay: bool,
    /// Whether the traveller set a departure window for the place, which then bounds the
    /// departure next to the stay range.
    pub has_departure_window: bool,
}

/// Legs joined by same-day stops, which the planner plans together, because the arrival of
/// one leg limits when the next can leave. A leg after a place with nights starts a new chain,
/// so most chains hold a single leg.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegChainInput {
    /// The legs in travel order. Never empty.
    pub legs: SmallVec<[IdentifiedLegInput; 4]>,
    /// The stop between leg `i` and leg `i + 1`.
    pub stops: SmallVec<[SameDayStop; 3]>,
}

impl LegChainInput {
    /// Returns the cache key of one leg of the chain.
    ///
    /// A single leg keeps the key of its own input. A leg of a longer chain depends on every
    /// leg and stop of the chain, so its key covers all of them and the leg's position.
    pub fn leg_cache_key(&self, backend_namespace: u64, leg_index: usize) -> LegCacheKey {
        if self.legs.len() == 1 {
            return self.legs[0].input.cache_key(backend_namespace);
        }
        // The legs are encoded with the length prefix a sequence encoding has, so a chain of
        // legs without a price weight keeps the key it always had.
        let mut encoded = postcard::to_stdvec(&self.legs.len()).unwrap_or_default();
        for leg in &self.legs {
            encoded.extend(leg.input.key_bytes());
        }
        encoded.extend(
            // Every field is a plain value that postcard encodes without failing.
            postcard::to_stdvec(&(&self.stops, leg_index)).unwrap_or_default(),
        );
        LegCacheKey(xxhash_rust::xxh3::xxh3_64_with_seed(
            &encoded,
            backend_namespace,
        ))
    }
}

/// Groups the legs of a trip into chains joined by same-day stops, in travel order.
pub fn leg_chain_inputs(trip: &Trip) -> SmallVec<[LegChainInput; 8]> {
    let mut chains: SmallVec<[LegChainInput; 8]> = SmallVec::new();
    for (leg, input) in trip.itinerary.legs().zip(leg_planning_inputs(trip)) {
        let same_day_stop = match leg.stay_at_departure_place {
            Some(PlaceStay::SameDay {
                stay_range_override,
                prefers_longer_stay,
            }) => Some(SameDayStop {
                stay_range: stay_range_override.unwrap_or(trip.preferences.default_same_day_stay),
                prefers_longer_stay: *prefers_longer_stay,
                has_departure_window: matches!(
                    leg.departure_place.window_override,
                    Some(PlaceWindowOverride::Departure(_) | PlaceWindowOverride::Both(_))
                ),
            }),
            Some(PlaceStay::Nights(_)) | None => None,
        };
        match (same_day_stop, chains.last_mut()) {
            (Some(stop), Some(chain)) => {
                chain.stops.push(stop);
                chain.legs.push(input);
            }
            _ => chains.push(LegChainInput {
                legs: smallvec::smallvec![input],
                stops: SmallVec::new(),
            }),
        }
    }
    chains
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
                operator: "db-fernverkehr".into(),
                agency: "DB Fernverkehr AG".into(),
            }],
        };
        trip.apply_edit(TripEdit::ChooseJourney {
            arrival_place_id: place_id(&trip, 1),
            choice: Some(Box::new(choice.clone())),
        })
        .unwrap();
        let inputs = leg_planning_inputs(&trip);
        assert_eq!(inputs[0].input.chosen_journey, Some(choice));
        let after = cache_keys(&trip);
        assert_ne!(after[0], before[0]);
        assert_eq!(after[1..], before[1..]);
    }

    #[test]
    fn a_transfer_time_override_changes_only_its_leg() {
        let mut trip = trip_with_places(&FOUR_CITIES);
        let before = cache_keys(&trip);
        trip.apply_edit(TripEdit::SetLegMinimumTransferTime {
            arrival_place_id: place_id(&trip, 2),
            minimum_transfer_time: Some(DurationMinutes(30)),
        })
        .unwrap();
        let inputs = leg_planning_inputs(&trip);
        assert_eq!(inputs[1].input.minimum_transfer_time, DurationMinutes(30));
        assert_eq!(
            inputs[0].input.minimum_transfer_time,
            trip.preferences.minimum_transfer_time
        );
        let after = cache_keys(&trip);
        assert_eq!(after[0], before[0]);
        assert_ne!(after[1], before[1]);
        assert_eq!(after[2], before[2]);
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

    #[test]
    fn a_zero_price_weight_keeps_the_key_and_a_larger_one_changes_it() {
        let mut trip = trip_with_places(&FOUR_CITIES);
        let without_price = leg_planning_inputs(&trip)[0].input.clone();
        trip.preferences.price_weight = btt_model::PriceWeight::FRUGAL;
        let frugal = leg_planning_inputs(&trip)[0].input.clone();
        // The encoding of an input without a weight is the encoding the input had before the
        // weight existed, so cached results planned at weight zero survive.
        let before_prices = postcard::to_stdvec(&(
            &without_price.departure,
            &without_price.arrival,
            &without_price.departure_date,
            &without_price.travel_mode,
            &without_price.minimum_transfer_time,
            &without_price.pass_policy,
            &without_price.transfer_aversion,
            &without_price.chosen_journey,
        ))
        .unwrap();
        assert_eq!(without_price.key_bytes(), before_prices);
        assert_ne!(frugal.key_bytes(), before_prices);
        assert_ne!(without_price.cache_key(1), frugal.cache_key(1));
    }
}
