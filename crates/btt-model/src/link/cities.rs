//! The dictionary of city names that links refer to by number.
//!
//! The dictionary is the file `data/cities.tsv`, one name per line as the name and the latitude
//! and longitude of the city in microdegrees, separated by tabs. The web viewer reads the same
//! file. A place that the traveller found by search carries the English name that the geocoder
//! gave it, such as "Vienna", and a link writes that name as the line of the dictionary, with the
//! position of the place as the difference to the position of the city. A link names a city by its
//! line, so lines are only ever appended. The script `tools/city-dictionary.js` writes the file
//! from `GeoNames` data, which is licensed under CC BY 4.0.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::itinerary::TripPlace;
use crate::snapshot::PLACE_STEP_MICRODEGREES;

/// The dictionary file, compiled in.
const DICTIONARY_FILE: &str = include_str!("../../data/cities.tsv");

/// Furthest a place may lie from its city and still use the entry, in microdegrees on either axis.
/// That is about 50 kilometers, which holds the point that a geocoder gives for a city and the
/// point that `GeoNames` gives for it, even for the cities that sprawl.
const MAXIMUM_OFFSET_MICRODEGREES: i64 = 500_000;

/// A line of the dictionary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DictionaryCity {
    /// Name of the city.
    pub name: &'static str,
    /// Latitude in microdegrees.
    pub latitude: i32,
    /// Longitude in microdegrees.
    pub longitude: i32,
}

impl DictionaryCity {
    /// Returns the latitude of the city in steps of `PLACE_STEP_MICRODEGREES`, rounded.
    pub(super) fn latitude_steps(&self) -> i64 {
        steps_of(i64::from(self.latitude))
    }

    /// Returns the longitude of the city in steps of `PLACE_STEP_MICRODEGREES`, rounded.
    pub(super) fn longitude_steps(&self) -> i64 {
        steps_of(i64::from(self.longitude))
    }
}

/// Returns a value in microdegrees in steps of `PLACE_STEP_MICRODEGREES`, rounded to the
/// nearest step, with a half step rounded up.
pub(super) fn steps_of(microdegrees: i64) -> i64 {
    (microdegrees + PLACE_STEP_MICRODEGREES / 2).div_euclid(PLACE_STEP_MICRODEGREES)
}

/// A place that matches a city of the dictionary, with how far it lies from the city.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CityMatch {
    /// Position of the city in the dictionary.
    pub index: usize,
    /// The place's latitude minus the city's, in steps.
    pub latitude_offset: i64,
    /// The place's longitude minus the city's, in steps.
    pub longitude_offset: i64,
}

/// The cities of the dictionary file, and a lookup by name.
#[derive(Debug)]
pub(super) struct CityDictionary {
    entries: Vec<DictionaryCity>,
    positions_by_name: HashMap<&'static str, Vec<usize>>,
}

/// Returns whether a link writes the name of a place as a line of the dictionary, which is
/// shorter than the name itself.
pub(crate) fn names_a_city(place: &TripPlace) -> bool {
    dictionary().find(place).is_some()
}

/// Returns the dictionary, which is parsed on first use.
pub(super) fn dictionary() -> &'static CityDictionary {
    static DICTIONARY: OnceLock<CityDictionary> = OnceLock::new();
    DICTIONARY.get_or_init(|| CityDictionary::parse(DICTIONARY_FILE))
}

impl CityDictionary {
    /// Reads the lines of a dictionary file. A line that is not a city is skipped, which would
    /// shift the numbers of the cities after it, so a test checks that the file has none.
    fn parse(text: &'static str) -> Self {
        let mut entries = Vec::new();
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            if let [name, latitude, longitude] = fields[..]
                && let (Ok(latitude), Ok(longitude)) = (latitude.parse(), longitude.parse())
            {
                entries.push(DictionaryCity {
                    name,
                    latitude,
                    longitude,
                });
            }
        }
        let mut positions_by_name: HashMap<&'static str, Vec<usize>> = HashMap::new();
        for (position, entry) in entries.iter().enumerate() {
            positions_by_name
                .entry(entry.name)
                .or_default()
                .push(position);
        }
        Self {
            entries,
            positions_by_name,
        }
    }

    /// Returns the city at a position.
    pub(super) fn get(&self, index: usize) -> Option<&DictionaryCity> {
        self.entries.get(index)
    }

    /// Returns the number of cities.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Finds the city that a place stands for: one with its name that lies the nearest to it,
    /// within `MAXIMUM_OFFSET_MICRODEGREES`. Among cities as near, the one earlier in the
    /// dictionary wins. The place's position has to be a multiple of `PLACE_STEP_MICRODEGREES`.
    pub(super) fn find(&self, place: &TripPlace) -> Option<CityMatch> {
        let latitude = steps_of(i64::from(place.coordinates.latitude.microdegrees()));
        let longitude = steps_of(i64::from(place.coordinates.longitude.microdegrees()));
        let limit = MAXIMUM_OFFSET_MICRODEGREES / PLACE_STEP_MICRODEGREES;
        self.positions_by_name
            .get(place.name.as_str())?
            .iter()
            .filter_map(|&index| {
                let city = &self.entries[index];
                let latitude_offset = latitude - city.latitude_steps();
                let longitude_offset = longitude - city.longitude_steps();
                (latitude_offset.abs() <= limit && longitude_offset.abs() <= limit).then_some((
                    latitude_offset.abs() + longitude_offset.abs(),
                    CityMatch {
                        index,
                        latitude_offset,
                        longitude_offset,
                    },
                ))
            })
            .min_by_key(|(distance, found)| (*distance, found.index))
            .map(|(_, found)| found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinates::GeoCoordinates;
    use crate::itinerary::{StationSearchRadius, TripPlaceId};

    fn place_at(name: &str, latitude: f64, longitude: f64) -> TripPlace {
        TripPlace {
            id: TripPlaceId(std::num::NonZeroU32::MIN),
            name: name.into(),
            coordinates: GeoCoordinates::from_degrees(latitude, longitude).unwrap(),
            station_search_radius: StationSearchRadius::DEFAULT,
            window_override: None,
        }
    }

    #[test]
    fn every_line_of_the_file_is_a_city() {
        let lines = DICTIONARY_FILE.lines().count();
        assert_eq!(dictionary().len(), lines);
        assert!(lines >= 1000);
    }

    #[test]
    fn the_big_cities_come_first() {
        let first = dictionary().get(0).unwrap();
        assert_eq!(first.name, "Istanbul");
    }

    #[test]
    fn a_place_with_the_name_of_a_city_near_it_finds_the_city() {
        let found = dictionary()
            .find(&place_at("Vienna", 48.2084, 16.3725))
            .unwrap();
        assert_eq!(dictionary().get(found.index).unwrap().name, "Vienna");
        assert!(found.latitude_offset.abs() < 100 && found.longitude_offset.abs() < 100);
    }

    #[test]
    fn a_namesake_far_away_is_not_the_city() {
        // Vienna in Virginia is not the capital of Austria.
        assert_eq!(
            dictionary().find(&place_at("Vienna", 38.9014, -77.2652)),
            None
        );
        assert_eq!(
            dictionary().find(&place_at("Nowhere Halt", 48.2084, 16.3725)),
            None
        );
    }
}
