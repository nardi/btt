//! Edits the frontend sends to change a trip, and the rules that apply them.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use smol_str::SmolStr;
use specta::Type;
use thiserror::Error;

use crate::coordinates::GeoCoordinates;
use crate::history::TripHistoryState;
use crate::itinerary::{
    Itinerary, ItineraryRoute, LegTravelMode, PlaceStay, PlaceWindowOverride, StationSearchRadius,
    TripPlace, TripPlaceId, ViaPlace,
};
use crate::plan::JourneyChoice;
use crate::trip::{ChosenJourney, Trip, TripPreferences};

/// Largest number of places in one itinerary.
pub const MAXIMUM_PLACE_COUNT: usize = 255;

/// A place to add to a trip. The trip assigns its ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct NewTripPlace {
    /// Name shown in the itinerary.
    pub name: SmolStr,
    /// Point the traveller travels to or from.
    pub coordinates: GeoCoordinates,
    /// Largest distance to a station the planner may use for this place.
    pub station_search_radius: StationSearchRadius,
}

/// Where a new place goes in the itinerary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum PlaceInsertionPoint {
    /// Before the place at this index, or at the end when the index equals the place count.
    AtIndex(u16),
    /// The position that adds the least straight-line distance to the itinerary, including a new
    /// origin before the first place or a new destination after the last.
    LeastAddedDistance,
}

/// One change to a trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum TripEdit {
    /// Changes the trip's name.
    RenameTrip(SmolStr),
    /// Changes the date of the first departure.
    SetStartDate(NaiveDate),
    /// Adds a place.
    AddPlace {
        /// The place to add.
        place: NewTripPlace,
        /// Where the place goes.
        insert_at: PlaceInsertionPoint,
    },
    /// Moves a place to new coordinates.
    MovePlace {
        /// The place to move.
        place_id: TripPlaceId,
        /// The new coordinates.
        coordinates: GeoCoordinates,
    },
    /// Changes the name of a place.
    RenamePlace {
        /// The place to rename.
        place_id: TripPlaceId,
        /// The new name.
        name: SmolStr,
    },
    /// Changes how far from a place the planner may look for stations.
    SetStationSearchRadius {
        /// The place to change.
        place_id: TripPlaceId,
        /// The new radius.
        radius: StationSearchRadius,
    },
    /// Sets or clears the time windows that replace the trip's defaults at a place.
    SetPlaceWindowOverride {
        /// The place to change.
        place_id: TripPlaceId,
        /// The new override, or `None` to use the trip's defaults.
        window_override: Option<PlaceWindowOverride>,
    },
    /// Removes a place.
    RemovePlace(TripPlaceId),
    /// Moves a place to another position in the itinerary.
    ReorderPlace {
        /// The place to move.
        place_id: TripPlaceId,
        /// Index of the place after the move.
        new_index: u16,
    },
    /// Changes how long the traveller stays at a via place.
    SetPlaceStay {
        /// The via place to change.
        place_id: TripPlaceId,
        /// The new stay.
        stay: PlaceStay,
    },
    /// Changes whether the leg that arrives at a place includes a night train.
    SetLegTravelMode {
        /// Place at which the leg arrives. Every place except the origin has one arriving leg.
        arrival_place_id: TripPlaceId,
        /// The new travel mode.
        travel_mode: LegTravelMode,
    },
    /// Replaces the trip-wide preferences.
    SetTripPreferences(TripPreferences),
    /// Picks a journey for the leg that arrives at a place, or goes back to the planner's pick.
    ChooseJourney {
        /// Place at which the leg arrives.
        arrival_place_id: TripPlaceId,
        /// The picked journey, or `None` for the planner's pick. Boxed, because a choice holds
        /// several train labels and would make every edit as large.
        choice: Option<Box<JourneyChoice>>,
    },
}

impl TripEdit {
    /// Describes the edit in a few words for the undo and redo buttons, such as "Move Berlin".
    /// `trip` is the trip before the edit, which still has the names of the places it touches.
    pub fn describe(&self, trip: &Trip) -> SmolStr {
        let place_name = |place_id: &TripPlaceId| {
            trip.itinerary
                .place(*place_id)
                .map_or_else(|| SmolStr::new("a place"), |place| place.name.clone())
        };
        let text = match self {
            Self::RenameTrip(_) => "Rename the trip".to_owned(),
            Self::SetStartDate(_) => "Change the start date".to_owned(),
            Self::AddPlace { place, .. } => format!("Add {}", place.name),
            Self::MovePlace { place_id, .. } => format!("Move {}", place_name(place_id)),
            Self::RenamePlace { place_id, name } => {
                format!("Rename {} to {name}", place_name(place_id))
            }
            Self::SetStationSearchRadius { place_id, .. } => {
                format!("Change the station radius of {}", place_name(place_id))
            }
            Self::SetPlaceWindowOverride {
                place_id,
                window_override: Some(_),
            } => format!("Change the times of {}", place_name(place_id)),
            Self::SetPlaceWindowOverride {
                place_id,
                window_override: None,
            } => format!("Use the default times at {}", place_name(place_id)),
            Self::RemovePlace(place_id) => format!("Remove {}", place_name(place_id)),
            Self::ReorderPlace { place_id, .. } => format!("Reorder {}", place_name(place_id)),
            Self::SetPlaceStay {
                place_id,
                stay: PlaceStay::Nights(nights),
            } => {
                let unit = if nights.get() == 1 { "night" } else { "nights" };
                format!("Stay {nights} {unit} in {}", place_name(place_id))
            }
            Self::SetPlaceStay { place_id, .. } => {
                format!("Visit {} on the same day", place_name(place_id))
            }
            Self::SetLegTravelMode {
                arrival_place_id,
                travel_mode: LegTravelMode::Overnight(_),
            } => format!("Take a night train to {}", place_name(arrival_place_id)),
            Self::SetLegTravelMode {
                arrival_place_id, ..
            } => format!("Travel to {} by day", place_name(arrival_place_id)),
            Self::SetTripPreferences(_) => "Change the preferences".to_owned(),
            Self::ChooseJourney {
                arrival_place_id,
                choice: Some(_),
            } => format!("Pick a journey to {}", place_name(arrival_place_id)),
            Self::ChooseJourney {
                arrival_place_id,
                choice: None,
            } => format!(
                "Use the planner's journey to {}",
                place_name(arrival_place_id)
            ),
        };
        SmolStr::new(text)
    }
}

/// Something an edit changed besides what it asked for, shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum TripEditNotice {
    /// Two places with an overnight leg between them are no longer adjacent, so the overnight
    /// options of that leg were dropped.
    OvernightOptionDropped {
        /// Place the dropped leg left from.
        departure_place_id: TripPlaceId,
        /// Place the dropped leg arrived at.
        arrival_place_id: TripPlaceId,
    },
}

/// Notices of one edit. Most edits have none.
pub type TripEditNotices = SmallVec<[TripEditNotice; 2]>;

/// The trip after an edit, with the notices the edit produced and the state of the undo
/// history. Undo and redo return it too, with no notices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TripEditOutcome {
    /// The trip after the edit.
    pub trip: Trip,
    /// Side effects of the edit, shown to the user.
    #[specta(type = Vec<TripEditNotice>)]
    pub notices: TripEditNotices,
    /// What undo and redo would do after the edit.
    pub history: TripHistoryState,
}

/// Reasons an edit can be rejected. A rejected edit leaves the trip unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error, Serialize, Deserialize, Type)]
pub enum TripEditError {
    /// The trip has no place with this ID.
    #[error("the trip has no place with ID {0:?}")]
    UnknownPlace(TripPlaceId),
    /// Only via places have a stay. The origin and the destination do not.
    #[error("place {0:?} is the origin or the destination, which have no stay")]
    PlaceHasNoStay(TripPlaceId),
    /// The origin has no leg that arrives at it.
    #[error("place {0:?} is the origin, which has no arriving leg")]
    PlaceHasNoArrivingLeg(TripPlaceId),
    /// The index is past the end of the itinerary.
    #[error("index {index} is past the end of an itinerary with {place_count} places")]
    IndexOutOfRange {
        /// The rejected index.
        index: u16,
        /// Number of places in the itinerary.
        place_count: u16,
    },
    /// The itinerary already has the largest number of places allowed.
    #[error("an itinerary can have at most {MAXIMUM_PLACE_COUNT} places")]
    TooManyPlaces,
}

impl Trip {
    /// Applies one edit. On error the trip stays unchanged.
    ///
    /// # Errors
    ///
    /// Returns an error when the edit names a place the trip does not have, asks for something
    /// the place's position does not allow, or would exceed the place limit.
    pub fn apply_edit(&mut self, edit: TripEdit) -> Result<TripEditNotices, TripEditError> {
        let notices = self.apply_edit_to_parts(edit)?;
        self.drop_choices_of_missing_legs();
        Ok(notices)
    }

    /// Applies one edit to the part of the trip it changes.
    fn apply_edit_to_parts(&mut self, edit: TripEdit) -> Result<TripEditNotices, TripEditError> {
        match edit {
            TripEdit::RenameTrip(name) => {
                self.name = name;
                Ok(TripEditNotices::new())
            }
            TripEdit::SetStartDate(start_date) => {
                self.start_date = start_date;
                Ok(TripEditNotices::new())
            }
            TripEdit::SetTripPreferences(preferences) => {
                self.preferences = preferences;
                Ok(TripEditNotices::new())
            }
            TripEdit::AddPlace { place, insert_at } => self.add_place(place, insert_at),
            TripEdit::MovePlace {
                place_id,
                coordinates,
            } => self.update_place(place_id, |place| place.coordinates = coordinates),
            TripEdit::RenamePlace { place_id, name } => {
                self.update_place(place_id, |place| place.name = name)
            }
            TripEdit::SetStationSearchRadius { place_id, radius } => {
                self.update_place(place_id, |place| place.station_search_radius = radius)
            }
            TripEdit::SetPlaceWindowOverride {
                place_id,
                window_override,
            } => self.update_place(place_id, |place| place.window_override = window_override),
            TripEdit::RemovePlace(place_id) => {
                let mut layout = ItineraryLayout::from_itinerary(&self.itinerary);
                let index = layout.index_of(place_id)?;
                layout.places.remove(index);
                Ok(self.replace_itinerary(&layout))
            }
            TripEdit::ReorderPlace {
                place_id,
                new_index,
            } => {
                let mut layout = ItineraryLayout::from_itinerary(&self.itinerary);
                let index = layout.index_of(place_id)?;
                let place = layout.places.remove(index);
                if usize::from(new_index) > layout.places.len() {
                    return Err(TripEditError::IndexOutOfRange {
                        index: new_index,
                        place_count: layout.place_count(),
                    });
                }
                layout.places.insert(usize::from(new_index), place);
                Ok(self.replace_itinerary(&layout))
            }
            TripEdit::SetPlaceStay { place_id, stay } => {
                let mut layout = ItineraryLayout::from_itinerary(&self.itinerary);
                let index = layout.index_of(place_id)?;
                if index == 0 || index + 1 == layout.places.len() {
                    return Err(TripEditError::PlaceHasNoStay(place_id));
                }
                set_entry(&mut layout.stays, place_id, stay);
                Ok(self.replace_itinerary(&layout))
            }
            TripEdit::SetLegTravelMode {
                arrival_place_id,
                travel_mode,
            } => {
                let mut layout = ItineraryLayout::from_itinerary(&self.itinerary);
                let index = layout.index_of(arrival_place_id)?;
                let Some(previous_index) = index.checked_sub(1) else {
                    return Err(TripEditError::PlaceHasNoArrivingLeg(arrival_place_id));
                };
                let leg_key = (layout.places[previous_index].id, arrival_place_id);
                set_entry(&mut layout.leg_travel_modes, leg_key, travel_mode);
                Ok(self.replace_itinerary(&layout))
            }
            TripEdit::ChooseJourney {
                arrival_place_id,
                choice,
            } => {
                let layout = ItineraryLayout::from_itinerary(&self.itinerary);
                let index = layout.index_of(arrival_place_id)?;
                let Some(previous_index) = index.checked_sub(1) else {
                    return Err(TripEditError::PlaceHasNoArrivingLeg(arrival_place_id));
                };
                let departure_place_id = layout.places[previous_index].id;
                self.chosen_journeys
                    .retain(|chosen| chosen.arrival_place_id != arrival_place_id);
                if let Some(choice) = choice {
                    self.chosen_journeys.push(ChosenJourney {
                        departure_place_id,
                        arrival_place_id,
                        choice: *choice,
                    });
                }
                Ok(TripEditNotices::new())
            }
        }
    }

    /// Drops the journey choices of legs that no longer exist, such as after a reorder that
    /// separated their two places.
    fn drop_choices_of_missing_legs(&mut self) {
        let legs: SmallVec<[(TripPlaceId, TripPlaceId); 16]> = self
            .itinerary
            .legs()
            .map(|leg| (leg.departure_place.id, leg.arrival_place.id))
            .collect();
        self.chosen_journeys
            .retain(|chosen| legs.contains(&(chosen.departure_place_id, chosen.arrival_place_id)));
    }

    /// Adds a place at the requested position and assigns it the next free ID.
    fn add_place(
        &mut self,
        new_place: NewTripPlace,
        insert_at: PlaceInsertionPoint,
    ) -> Result<TripEditNotices, TripEditError> {
        let mut layout = ItineraryLayout::from_itinerary(&self.itinerary);
        if layout.places.len() >= MAXIMUM_PLACE_COUNT {
            return Err(TripEditError::TooManyPlaces);
        }
        let index = match insert_at {
            PlaceInsertionPoint::AtIndex(index) if usize::from(index) <= layout.places.len() => {
                usize::from(index)
            }
            PlaceInsertionPoint::AtIndex(index) => {
                return Err(TripEditError::IndexOutOfRange {
                    index,
                    place_count: layout.place_count(),
                });
            }
            PlaceInsertionPoint::LeastAddedDistance => {
                least_added_distance_index(&layout.places, new_place.coordinates)
            }
        };
        let id = TripPlaceId(self.next_place_id);
        let next_place_id = self
            .next_place_id
            .checked_add(1)
            .ok_or(TripEditError::TooManyPlaces)?;
        layout.places.insert(
            index,
            TripPlace {
                id,
                name: new_place.name,
                coordinates: new_place.coordinates,
                station_search_radius: new_place.station_search_radius,
                window_override: None,
            },
        );
        self.next_place_id = next_place_id;
        Ok(self.replace_itinerary(&layout))
    }

    /// Changes one field of a place without touching the itinerary's structure.
    fn update_place(
        &mut self,
        place_id: TripPlaceId,
        update: impl FnOnce(&mut TripPlace),
    ) -> Result<TripEditNotices, TripEditError> {
        let place = match &mut self.itinerary {
            Itinerary::Empty => None,
            Itinerary::OriginOnly(origin) => Some(origin).filter(|place| place.id == place_id),
            Itinerary::Route(route) => std::iter::once(&mut route.origin)
                .chain(
                    route
                        .via_places
                        .iter_mut()
                        .map(|via_place| &mut via_place.place),
                )
                .chain(std::iter::once(&mut route.destination))
                .find(|place| place.id == place_id),
        };
        let place = place.ok_or(TripEditError::UnknownPlace(place_id))?;
        update(place);
        Ok(TripEditNotices::new())
    }

    /// Rebuilds the itinerary from a layout and returns the notices of the rebuild.
    fn replace_itinerary(&mut self, layout: &ItineraryLayout) -> TripEditNotices {
        let (itinerary, notices) = layout.build();
        self.itinerary = itinerary;
        notices
    }
}

/// An itinerary taken apart into its place order and the settings keyed by place.
///
/// Structural edits change the order and rebuild the itinerary from it. Stays and leg travel
/// modes follow their places, so a stay survives a reorder and a leg keeps its travel mode as
/// long as its two places stay adjacent.
struct ItineraryLayout {
    /// Places in travel order.
    places: Vec<TripPlace>,
    /// Stay per place that was a via place, or that an edit gave a stay.
    stays: SmallVec<[(TripPlaceId, PlaceStay); 16]>,
    /// Travel mode per leg, keyed by its departure and arrival place.
    leg_travel_modes: SmallVec<[((TripPlaceId, TripPlaceId), LegTravelMode); 16]>,
}

impl ItineraryLayout {
    /// Takes an itinerary apart. The itinerary itself stays unchanged.
    fn from_itinerary(itinerary: &Itinerary) -> Self {
        let places = itinerary.trip_places().cloned().collect();
        let mut stays = SmallVec::new();
        let mut leg_travel_modes = SmallVec::new();
        for leg in itinerary.legs() {
            if let Some(stay) = leg.stay_at_departure_place {
                stays.push((leg.departure_place.id, *stay));
            }
            leg_travel_modes.push((
                (leg.departure_place.id, leg.arrival_place.id),
                *leg.travel_mode,
            ));
        }
        Self {
            places,
            stays,
            leg_travel_modes,
        }
    }

    /// Returns the number of places as a `u16`. The place limit keeps it in range.
    #[allow(clippy::cast_possible_truncation)]
    fn place_count(&self) -> u16 {
        self.places.len() as u16
    }

    /// Returns the index of a place in travel order.
    fn index_of(&self, place_id: TripPlaceId) -> Result<usize, TripEditError> {
        self.places
            .iter()
            .position(|place| place.id == place_id)
            .ok_or(TripEditError::UnknownPlace(place_id))
    }

    /// Builds an itinerary from the place order.
    ///
    /// A new via place gets the default stay and a new pair of adjacent places gets a daytime
    /// leg. Every overnight leg whose places are no longer adjacent becomes a notice.
    fn build(&self) -> (Itinerary, TripEditNotices) {
        let travel_mode_between = |departure: &TripPlace, arrival: &TripPlace| {
            find_entry(&self.leg_travel_modes, &(departure.id, arrival.id))
                .unwrap_or(LegTravelMode::Daytime)
        };
        let itinerary = match self.places.as_slice() {
            [] => Itinerary::Empty,
            [origin] => Itinerary::OriginOnly(origin.clone()),
            [origin, via_places @ .., destination] => {
                let via_places = via_places
                    .iter()
                    .scan(origin, |previous_place, place| {
                        let via_place = ViaPlace {
                            arriving_leg_travel_mode: travel_mode_between(previous_place, place),
                            place: place.clone(),
                            stay: find_entry(&self.stays, &place.id).unwrap_or(PlaceStay::DEFAULT),
                        };
                        *previous_place = place;
                        Some(via_place)
                    })
                    .collect::<Vec<_>>();
                let last_departure = via_places
                    .last()
                    .map_or(origin, |via_place| &via_place.place);
                Itinerary::Route(ItineraryRoute {
                    final_leg_travel_mode: travel_mode_between(last_departure, destination),
                    origin: origin.clone(),
                    via_places,
                    destination: destination.clone(),
                })
            }
        };
        let is_adjacent_pair = |(departure_id, arrival_id): &(TripPlaceId, TripPlaceId)| {
            self.places
                .windows(2)
                .any(|pair| pair[0].id == *departure_id && pair[1].id == *arrival_id)
        };
        let notices = self
            .leg_travel_modes
            .iter()
            .filter(|(leg_key, travel_mode)| {
                travel_mode.is_overnight() && !is_adjacent_pair(leg_key)
            })
            .map(|((departure_place_id, arrival_place_id), _)| {
                TripEditNotice::OvernightOptionDropped {
                    departure_place_id: *departure_place_id,
                    arrival_place_id: *arrival_place_id,
                }
            })
            .collect();
        (itinerary, notices)
    }
}

/// Returns the value stored for a key in a small list of key and value pairs.
fn find_entry<K: PartialEq, V: Copy>(entries: &[(K, V)], key: &K) -> Option<V> {
    entries
        .iter()
        .find(|(entry_key, _)| entry_key == key)
        .map(|(_, value)| *value)
}

/// Replaces or adds the value for a key in a small list of key and value pairs.
fn set_entry<K: PartialEq, V, const N: usize>(
    entries: &mut SmallVec<[(K, V); N]>,
    key: K,
    value: V,
) {
    match entries.iter_mut().find(|(entry_key, _)| *entry_key == key) {
        Some(entry) => entry.1 = value,
        None => entries.push((key, value)),
    }
}

/// Returns the insertion index that adds the least straight-line distance to the itinerary.
///
/// Inserting between two places adds the detour through the new place. Inserting before the
/// first place or after the last adds the distance to that place. On a tie the place becomes the
/// new destination, because travellers usually add places in travel order.
fn least_added_distance_index(places: &[TripPlace], coordinates: GeoCoordinates) -> usize {
    let distance = |place: &TripPlace| place.coordinates.haversine_distance_meters(coordinates);
    let (Some(first), Some(last)) = (places.first(), places.last()) else {
        return 0;
    };
    let mut best_index = 0;
    let mut best_added_meters = distance(first);
    for (gap_index, pair) in places.windows(2).enumerate() {
        let added_meters = distance(&pair[0]) + distance(&pair[1])
            - pair[0]
                .coordinates
                .haversine_distance_meters(pair[1].coordinates);
        if added_meters < best_added_meters {
            best_index = gap_index + 1;
            best_added_meters = added_meters;
        }
    }
    if distance(last) <= best_added_meters {
        best_index = places.len();
    }
    best_index
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::itinerary::tests::overnight;

    fn new_place(name: &str, latitude: f64, longitude: f64) -> NewTripPlace {
        NewTripPlace {
            name: name.into(),
            coordinates: GeoCoordinates::from_degrees(latitude, longitude).unwrap(),
            station_search_radius: StationSearchRadius::DEFAULT,
        }
    }

    fn trip_with_places(places: &[(&str, f64, f64)]) -> Trip {
        let mut trip = Trip::new("Test".into(), NaiveDate::from_ymd_opt(2026, 10, 1).unwrap());
        for (name, latitude, longitude) in places {
            let index = u16::try_from(trip.itinerary.place_count()).unwrap();
            trip.apply_edit(TripEdit::AddPlace {
                place: new_place(name, *latitude, *longitude),
                insert_at: PlaceInsertionPoint::AtIndex(index),
            })
            .unwrap();
        }
        trip
    }

    fn names(trip: &Trip) -> Vec<&str> {
        trip.itinerary
            .trip_places()
            .map(|place| place.name.as_str())
            .collect()
    }

    fn id_of(trip: &Trip, name: &str) -> TripPlaceId {
        trip.itinerary
            .trip_places()
            .find(|place| place.name == name)
            .unwrap()
            .id
    }

    const AMSTERDAM: (&str, f64, f64) = ("Amsterdam", 52.378_9, 4.900_3);
    const BERLIN: (&str, f64, f64) = ("Berlin", 52.525_1, 13.369_4);
    const VIENNA: (&str, f64, f64) = ("Vienna", 48.185_2, 16.376_1);
    const ROME: (&str, f64, f64) = ("Rome", 41.901_0, 12.500_7);

    fn choice_of_one_train(number: &str) -> JourneyChoice {
        JourneyChoice {
            first_departure_time: crate::time::UnixTimeMinutes(29_000_000),
            trains: smallvec::smallvec![crate::plan::TrainLabel {
                category: "ICE".into(),
                number: number.into(),
                operator: "db-fernverkehr".into(),
                agency: "DB Fernverkehr AG".into(),
            }],
        }
    }

    #[test]
    fn a_journey_choice_stays_with_its_leg_until_the_leg_disappears() {
        let mut trip = trip_with_places(&[AMSTERDAM, BERLIN, VIENNA]);
        let berlin = id_of(&trip, "Berlin");
        let vienna = id_of(&trip, "Vienna");
        trip.apply_edit(TripEdit::ChooseJourney {
            arrival_place_id: berlin,
            choice: Some(Box::new(choice_of_one_train("145"))),
        })
        .unwrap();
        // A second choice for the same leg replaces the first.
        trip.apply_edit(TripEdit::ChooseJourney {
            arrival_place_id: berlin,
            choice: Some(Box::new(choice_of_one_train("147"))),
        })
        .unwrap();
        assert_eq!(trip.chosen_journeys.len(), 1);
        assert_eq!(trip.chosen_journeys[0].choice.trains[0].number, "147");

        // Renaming keeps the leg, and moving Berlin to the end removes it.
        trip.apply_edit(TripEdit::RenamePlace {
            place_id: vienna,
            name: "Wien".into(),
        })
        .unwrap();
        assert_eq!(trip.chosen_journeys.len(), 1);
        trip.apply_edit(TripEdit::ReorderPlace {
            place_id: berlin,
            new_index: 2,
        })
        .unwrap();
        assert_eq!(trip.chosen_journeys, Vec::new());
    }

    #[test]
    fn the_origin_has_no_journey_to_choose() {
        let mut trip = trip_with_places(&[AMSTERDAM, BERLIN]);
        let amsterdam = id_of(&trip, "Amsterdam");
        assert_eq!(
            trip.apply_edit(TripEdit::ChooseJourney {
                arrival_place_id: amsterdam,
                choice: None,
            }),
            Err(TripEditError::PlaceHasNoArrivingLeg(amsterdam))
        );
    }

    #[test]
    fn adding_places_moves_through_itinerary_variants() {
        let mut trip = trip_with_places(&[]);
        assert_eq!(trip.itinerary, Itinerary::Empty);
        trip = trip_with_places(&[AMSTERDAM]);
        assert!(matches!(trip.itinerary, Itinerary::OriginOnly(_)));
        trip = trip_with_places(&[AMSTERDAM, VIENNA, ROME]);
        let Itinerary::Route(route) = &trip.itinerary else {
            panic!("expected a route")
        };
        assert_eq!(route.via_places[0].stay, PlaceStay::DEFAULT);
    }

    #[test]
    fn least_added_distance_puts_berlin_between_amsterdam_and_vienna() {
        let mut trip = trip_with_places(&[AMSTERDAM, VIENNA]);
        let (name, latitude, longitude) = BERLIN;
        trip.apply_edit(TripEdit::AddPlace {
            place: new_place(name, latitude, longitude),
            insert_at: PlaceInsertionPoint::LeastAddedDistance,
        })
        .unwrap();
        assert_eq!(names(&trip), ["Amsterdam", "Berlin", "Vienna"]);
    }

    #[test]
    fn least_added_distance_extends_past_the_destination() {
        let mut trip = trip_with_places(&[AMSTERDAM, BERLIN]);
        let (name, latitude, longitude) = VIENNA;
        trip.apply_edit(TripEdit::AddPlace {
            place: new_place(name, latitude, longitude),
            insert_at: PlaceInsertionPoint::LeastAddedDistance,
        })
        .unwrap();
        assert_eq!(names(&trip), ["Amsterdam", "Berlin", "Vienna"]);
    }

    #[test]
    fn least_added_distance_appends_to_a_single_place() {
        let mut trip = trip_with_places(&[AMSTERDAM]);
        let (name, latitude, longitude) = BERLIN;
        trip.apply_edit(TripEdit::AddPlace {
            place: new_place(name, latitude, longitude),
            insert_at: PlaceInsertionPoint::LeastAddedDistance,
        })
        .unwrap();
        assert_eq!(names(&trip), ["Amsterdam", "Berlin"]);
    }

    #[test]
    fn reorder_keeps_stays_and_drops_broken_overnight_legs() {
        let mut trip = trip_with_places(&[AMSTERDAM, BERLIN, VIENNA, ROME]);
        let berlin = id_of(&trip, "Berlin");
        let vienna = id_of(&trip, "Vienna");
        let rome = id_of(&trip, "Rome");
        let three_nights = PlaceStay::Nights(3.try_into().unwrap());
        trip.apply_edit(TripEdit::SetPlaceStay {
            place_id: berlin,
            stay: three_nights,
        })
        .unwrap();
        trip.apply_edit(TripEdit::SetLegTravelMode {
            arrival_place_id: rome,
            travel_mode: overnight(),
        })
        .unwrap();

        let notices = trip
            .apply_edit(TripEdit::ReorderPlace {
                place_id: vienna,
                new_index: 1,
            })
            .unwrap();

        assert_eq!(names(&trip), ["Amsterdam", "Vienna", "Berlin", "Rome"]);
        assert_eq!(
            notices.as_slice(),
            [TripEditNotice::OvernightOptionDropped {
                departure_place_id: vienna,
                arrival_place_id: rome,
            }]
        );
        let Itinerary::Route(route) = &trip.itinerary else {
            panic!("expected a route")
        };
        assert_eq!(route.via_places[1].stay, three_nights);
        assert_eq!(route.final_leg_travel_mode, LegTravelMode::Daytime);
    }

    #[test]
    fn reorder_keeps_overnight_leg_when_pair_stays_adjacent() {
        let mut trip = trip_with_places(&[AMSTERDAM, BERLIN, VIENNA, ROME]);
        let rome = id_of(&trip, "Rome");
        trip.apply_edit(TripEdit::SetLegTravelMode {
            arrival_place_id: rome,
            travel_mode: overnight(),
        })
        .unwrap();
        let berlin = id_of(&trip, "Berlin");
        let notices = trip
            .apply_edit(TripEdit::ReorderPlace {
                place_id: berlin,
                new_index: 0,
            })
            .unwrap();
        assert!(notices.is_empty());
        let Itinerary::Route(route) = &trip.itinerary else {
            panic!("expected a route")
        };
        assert_eq!(route.final_leg_travel_mode, overnight());
    }

    #[test]
    fn origin_and_destination_reject_stays_and_origin_rejects_leg_mode() {
        let mut trip = trip_with_places(&[AMSTERDAM, VIENNA]);
        let amsterdam = id_of(&trip, "Amsterdam");
        let vienna = id_of(&trip, "Vienna");
        let before = trip.clone();
        assert_eq!(
            trip.apply_edit(TripEdit::SetPlaceStay {
                place_id: vienna,
                stay: PlaceStay::DEFAULT
            }),
            Err(TripEditError::PlaceHasNoStay(vienna))
        );
        assert_eq!(
            trip.apply_edit(TripEdit::SetLegTravelMode {
                arrival_place_id: amsterdam,
                travel_mode: LegTravelMode::Daytime,
            }),
            Err(TripEditError::PlaceHasNoArrivingLeg(amsterdam))
        );
        assert_eq!(trip, before);
    }

    #[test]
    fn removed_place_ids_are_not_reused() {
        let mut trip = trip_with_places(&[AMSTERDAM, VIENNA]);
        let vienna = id_of(&trip, "Vienna");
        trip.apply_edit(TripEdit::RemovePlace(vienna)).unwrap();
        let (name, latitude, longitude) = ROME;
        trip.apply_edit(TripEdit::AddPlace {
            place: new_place(name, latitude, longitude),
            insert_at: PlaceInsertionPoint::AtIndex(1),
        })
        .unwrap();
        assert_ne!(id_of(&trip, "Rome"), vienna);
    }

    #[test]
    fn edits_describe_themselves_with_place_names() {
        let trip = trip_with_places(&[("Amsterdam", 52.37, 4.90), ("Berlin", 52.52, 13.37)]);
        let berlin = trip.itinerary.trip_places().nth(1).unwrap().id;
        let describe = |edit: TripEdit| edit.describe(&trip).to_string();
        assert_eq!(describe(TripEdit::RemovePlace(berlin)), "Remove Berlin");
        assert_eq!(
            describe(TripEdit::SetLegTravelMode {
                arrival_place_id: berlin,
                travel_mode: overnight(),
            }),
            "Take a night train to Berlin"
        );
        assert_eq!(
            describe(TripEdit::RenamePlace {
                place_id: berlin,
                name: "Berlin Hbf".into(),
            }),
            "Rename Berlin to Berlin Hbf"
        );
    }
}
