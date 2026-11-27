//! Searches places by name without a network connection.
//!
//! The index holds the cities of the link dictionary and the stations of the timetable. A query
//! matches a place when every word of the query is the start of a word of the place's name. The
//! comparison ignores case and diacritics, so "koln" finds "Köln". The index has no other
//! languages than the ones the names have, so "Mailand" does not find Milan.

use std::sync::OnceLock;

use btt_geo::{TimeZoneFinder, country_of_time_zone};
use btt_model::link::dictionary_cities;
use btt_model::{GeoCoordinates, PlaceSearchResult};
use btt_timetable::{BundleStationIndex, LoadedTimetableBundle};
use rstar::RTree;
use rstar::primitives::GeomWithData;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use smol_str::SmolStr;

/// Largest number of results a local search returns.
pub const LOCAL_RESULT_LIMIT: usize = 8;

/// Largest number of results that an online search adds after the local results.
pub const ONLINE_RESULT_LIMIT: usize = 5;

/// Furthest a station may lie from a city for its context to name the city, in meters.
const NEARBY_CITY_METERS: f64 = 25_000.0;

/// Furthest a station may lie from a bigger city for its context to name the bigger city instead
/// of a nearer one, in meters.
const CONTEXT_CITY_METERS: f64 = 12_000.0;

/// Furthest two results with the same name may lie apart and still be the same place, in meters.
/// Cities sprawl, and a geocoder and `GeoNames` put the point of a city in different places.
const SAME_NAME_METERS: f64 = 20_000.0;

/// Furthest two results with different names may lie apart and still be the same place, in
/// meters. This joins a station that two sources name differently, such as "Wien Hbf" and "Wien
/// Hauptbahnhof".
const SAME_PLACE_METERS: f64 = 300.0;

/// A city position as longitude and latitude in degrees, with the index of the city's place.
type CityPoint = GeomWithData<[f64; 2], usize>;

/// What a place of the index is, which decides its rank among places that match equally well.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PlaceImportance {
    /// A city, with its line in the dictionary. A lower line is a bigger city.
    City {
        /// The first line of the city in the dictionary.
        dictionary_line: usize,
    },
    /// A station, with the number of routes that stop at it. More routes rank higher.
    Station {
        /// The number of routes, as a negative number so that the order is ascending.
        negative_route_count: i64,
    },
}

/// A name of a place, in the form that the search compares.
#[derive(Debug)]
struct FoldedName {
    /// The name as the traveller sees it.
    shown: SmolStr,
    /// The words of the folded name, joined by single spaces.
    whole: Box<str>,
    /// The words of the folded name.
    words: Box<[Box<str>]>,
}

impl FoldedName {
    fn new(shown: &str) -> Self {
        let words = folded_words(shown);
        Self {
            shown: shown.into(),
            whole: words.join(" ").into_boxed_str(),
            words: words.into_iter().map(String::into_boxed_str).collect(),
        }
    }

    /// Returns whether the words of this name are the first words of one of the other names.
    fn is_short_form_of_any<'other>(
        &self,
        others: impl IntoIterator<Item = &'other FoldedName>,
    ) -> bool {
        others.into_iter().any(|other| {
            other.words.len() > self.words.len() && other.words.starts_with(&self.words)
        })
    }

    /// Returns how well a query matches the name, lower is better, or `None` when it does not
    /// match. Every query word has to be the start of a word of the name.
    fn match_tier(&self, query: &FoldedQuery) -> Option<u8> {
        let all_words_match = query.words.iter().all(|query_word| {
            self.words
                .iter()
                .any(|word| word.starts_with(&**query_word))
        });
        if !all_words_match {
            return None;
        }
        let first_query_word = query.words.first()?;
        let first_word = self.words.first()?;
        Some(if *self.whole == *query.whole {
            0
        } else if **first_word == **first_query_word {
            1
        } else if first_word.starts_with(&**first_query_word) {
            2
        } else {
            3
        })
    }
}

/// A query in the form that the search compares.
struct FoldedQuery {
    /// The words of the folded query, joined by single spaces.
    whole: String,
    /// The words of the folded query.
    words: Vec<String>,
}

impl FoldedQuery {
    fn new(query: &str) -> Self {
        let words = folded_words(query);
        Self {
            whole: words.join(" "),
            words,
        }
    }
}

/// A place that the search can find.
#[derive(Debug)]
struct IndexedPlace {
    /// The names of the place. The first is the one a result shows unless another name matches
    /// the query better.
    names: SmallVec<[FoldedName; 1]>,
    /// The context that a result shows next to the name.
    context: SmolStr,
    /// The position of the place.
    coordinates: GeoCoordinates,
    /// The rank of the place among places that match equally well.
    importance: PlaceImportance,
}

impl IndexedPlace {
    /// Returns the name of a city that the context of a station shows. That is the first name
    /// that is not the start of another name, such as "Frankfurt am Main" and not "Frankfurt".
    fn context_name(&self) -> &SmolStr {
        let name = self
            .names
            .iter()
            .find(|name| !name.is_short_form_of_any(&self.names))
            .unwrap_or(&self.names[0]);
        &name.shown
    }
}

/// The cities of the link dictionary and the stations of a timetable, searchable by name.
#[derive(Debug)]
pub struct PlaceIndex {
    /// Cities first, in the order of the dictionary, then stations.
    places: Vec<IndexedPlace>,
}

impl PlaceIndex {
    /// Builds the index over the cities of the link dictionary and the stations of a bundle.
    ///
    /// A station that no route stops at is left out. So is a station that has the name of a
    /// city near it, because the city result already covers it.
    pub fn new(bundle: Option<&LoadedTimetableBundle>, time_zone_finder: &TimeZoneFinder) -> Self {
        let mut places = city_places(time_zone_finder);
        let city_tree = RTree::bulk_load(
            places
                .iter()
                .enumerate()
                .map(|(index, place)| CityPoint::new(degrees_of(place.coordinates), index))
                .collect(),
        );
        if let Some(bundle) = bundle {
            let stations = station_places(bundle, time_zone_finder, &places, &city_tree);
            places.extend(stations);
        }
        Self { places }
    }

    /// Returns the index over the cities of the link dictionary only, for an app that has no
    /// timetable yet. The index is built on first use.
    pub fn cities_only(time_zone_finder: &TimeZoneFinder) -> &'static Self {
        static CITIES_ONLY: OnceLock<PlaceIndex> = OnceLock::new();
        CITIES_ONLY.get_or_init(|| Self::new(None, time_zone_finder))
    }

    /// Returns the places whose names match a query, best first, at most `LOCAL_RESULT_LIMIT`.
    ///
    /// A result shows the longest name of the place that matches the query.
    ///
    /// The place whose whole name is the query comes first. Then come places whose first word
    /// is the first word of the query, then places whose first word starts with it, then the
    /// others. Within each group, cities come before stations, bigger cities first and stations
    /// with more routes first.
    pub fn search(&self, query: &str) -> Vec<PlaceSearchResult> {
        let query = FoldedQuery::new(query);
        if query.words.is_empty() {
            return Vec::new();
        }
        let mut matches: Vec<(u8, PlaceImportance, &FoldedName, &IndexedPlace)> = self
            .places
            .iter()
            .filter_map(|place| {
                let matching_names: SmallVec<[(u8, &FoldedName); 2]> = place
                    .names
                    .iter()
                    .filter_map(|name| Some((name.match_tier(&query)?, name)))
                    .collect();
                let tier = matching_names.iter().map(|(tier, _)| *tier).min()?;
                let name = shown_name(&matching_names)?;
                Some((tier, place.importance, name, place))
            })
            .collect();
        matches.sort_by_key(|(tier, importance, _, _)| (*tier, *importance));
        matches
            .into_iter()
            .take(LOCAL_RESULT_LIMIT)
            .map(|(_, _, name, place)| PlaceSearchResult {
                name: name.shown.clone(),
                context: place.context.clone(),
                coordinates: place.coordinates,
            })
            .collect()
    }

    /// Returns the results of an online search that the local search for the same query lacks,
    /// in their order, at most `ONLINE_RESULT_LIMIT`.
    pub fn online_extras(
        &self,
        query: &str,
        online_results: impl IntoIterator<Item = PlaceSearchResult>,
    ) -> Vec<PlaceSearchResult> {
        let local_results = self.search(query);
        online_results
            .into_iter()
            .filter(|online| {
                !local_results
                    .iter()
                    .any(|local| results_are_same_place(local, online))
            })
            .take(ONLINE_RESULT_LIMIT)
            .collect()
    }
}

/// Returns the name that a result shows, among the names of a place that match a query.
///
/// A name that is the start of another name that matches is a short form, such as "Frankfurt"
/// for "Frankfurt am Main", and the longer name is shown. Otherwise the name that matches best is
/// shown. Among names that match equally well, the one with diacritics is the original name, such
/// as "Zürich" for "Zurich".
fn shown_name<'name>(matching_names: &[(u8, &'name FoldedName)]) -> Option<&'name FoldedName> {
    matching_names
        .iter()
        .filter(|(_, name)| {
            !name.is_short_form_of_any(matching_names.iter().map(|(_, other)| *other))
        })
        .min_by_key(|(tier, name)| (*tier, name.shown.is_ascii()))
        .map(|(_, name)| *name)
}

/// Returns whether two results are the same place: the same name near each other, or any names
/// very close to each other.
fn results_are_same_place(left: &PlaceSearchResult, right: &PlaceSearchResult) -> bool {
    let distance = left
        .coordinates
        .haversine_distance_meters(right.coordinates);
    distance <= SAME_PLACE_METERS
        || (distance <= SAME_NAME_METERS && folded_words(&left.name) == folded_words(&right.name))
}

/// Returns the cities of the dictionary, one place per position. Lines with the same position
/// are names of one city.
///
/// A district of a bigger city is left out, such as "Paris 15 Vaugirard" or "Amsterdam-Zuid".
/// It is a city whose name starts with the name of a bigger city near it.
fn city_places(time_zone_finder: &TimeZoneFinder) -> Vec<IndexedPlace> {
    let mut places: Vec<IndexedPlace> = Vec::new();
    let mut place_by_position: FxHashMap<(i32, i32), usize> = FxHashMap::default();
    for (dictionary_line, (name, coordinates)) in dictionary_cities().enumerate() {
        let position = (
            coordinates.latitude.microdegrees(),
            coordinates.longitude.microdegrees(),
        );
        if let Some(&index) = place_by_position.get(&position) {
            places[index].names.push(FoldedName::new(name));
            continue;
        }
        place_by_position.insert(position, places.len());
        let country = country_of_time_zone(time_zone_finder.time_zone_at(coordinates));
        places.push(IndexedPlace {
            names: smallvec::smallvec![FoldedName::new(name)],
            context: country.unwrap_or_default().into(),
            coordinates,
            importance: PlaceImportance::City { dictionary_line },
        });
    }
    let mut kept: Vec<IndexedPlace> = Vec::with_capacity(places.len());
    for place in places {
        let is_district = kept.iter().any(|bigger| {
            bigger
                .coordinates
                .haversine_distance_meters(place.coordinates)
                <= SAME_NAME_METERS
                && bigger.names.iter().any(|bigger_name| {
                    place
                        .names
                        .iter()
                        .any(|name| bigger_name.is_short_form_of_any(std::iter::once(name)))
                })
        });
        if !is_district {
            kept.push(place);
        }
    }
    kept
}

/// Returns the stations of a bundle that routes stop at, except those that a city covers.
fn station_places(
    bundle: &LoadedTimetableBundle,
    time_zone_finder: &TimeZoneFinder,
    cities: &[IndexedPlace],
    city_tree: &RTree<CityPoint>,
) -> Vec<IndexedPlace> {
    let mut stations = Vec::new();
    for index in 0..bundle.station_count() {
        let Ok(index) = u32::try_from(index) else {
            break;
        };
        let station = BundleStationIndex(index);
        let route_count = bundle.route_stops_at(station).len();
        if route_count == 0 {
            continue;
        }
        let name = FoldedName::new(bundle.station_name(station));
        let coordinates = bundle.station_coordinates(station);
        let context_city = context_city_of(coordinates, cities, city_tree);
        let city_has_station_name = context_city.is_some_and(|city| {
            city.coordinates.haversine_distance_meters(coordinates) <= SAME_NAME_METERS
                && city
                    .names
                    .iter()
                    .any(|city_name| city_name.whole == name.whole)
        });
        if city_has_station_name {
            continue;
        }
        let country = country_of_time_zone(time_zone_finder.time_zone_at(coordinates));
        let context = match (context_city, country) {
            (Some(city), Some(country)) => format!("{}, {country}", city.context_name()).into(),
            (Some(city), None) => city.context_name().clone(),
            (None, country) => country.unwrap_or_default().into(),
        };
        stations.push(IndexedPlace {
            names: smallvec::smallvec![name],
            context,
            coordinates,
            importance: PlaceImportance::Station {
                negative_route_count: -i64::try_from(route_count).unwrap_or(i64::MAX),
            },
        });
    }
    stations
}

/// Returns the city that the context of a station names. That is the biggest city within
/// `CONTEXT_CITY_METERS`, so that a station in a suburb names the city it belongs to. Without
/// one, it is the nearest city within `NEARBY_CITY_METERS`.
fn context_city_of<'city>(
    coordinates: GeoCoordinates,
    cities: &'city [IndexedPlace],
    city_tree: &RTree<CityPoint>,
) -> Option<&'city IndexedPlace> {
    let mut nearby = city_tree
        .nearest_neighbor_iter(degrees_of(coordinates))
        .map(|point| {
            let city = &cities[point.data];
            (
                point.data,
                city.coordinates.haversine_distance_meters(coordinates),
            )
        })
        .take_while(|(_, distance)| *distance <= NEARBY_CITY_METERS)
        .peekable();
    let nearest = nearby.peek().copied();
    // The cities are in the order of the dictionary, so a lower index is a bigger city.
    let biggest = nearby
        .filter(|(_, distance)| *distance <= CONTEXT_CITY_METERS)
        .map(|(index, _)| index)
        .min();
    biggest
        .or(nearest.map(|(index, _)| index))
        .map(|index| &cities[index])
}

/// Returns a position as longitude and latitude in degrees, the order of the city tree.
fn degrees_of(coordinates: GeoCoordinates) -> [f64; 2] {
    [
        coordinates.longitude.degrees(),
        coordinates.latitude.degrees(),
    ]
}

/// Returns the words of a text in lowercase and without diacritics. Every character that is not
/// a letter or a digit separates words, except apostrophes, which are dropped.
fn folded_words(text: &str) -> Vec<String> {
    let mut folded = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\'' | '’' => {}
            _ => push_folded(&mut folded, character),
        }
    }
    folded
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Appends a character in lowercase and without diacritics.
///
/// The table covers the letters of the languages that the timetable covers. A letter outside the
/// table is only lowercased.
fn push_folded(folded: &mut String, character: char) {
    let replacement = match character {
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'Ā' | 'Ă' | 'Ą' | 'à' | 'á' | 'â' | 'ã' | 'ä' | 'å'
        | 'ā' | 'ă' | 'ą' => "a",
        'Æ' | 'æ' => "ae",
        'Ç' | 'Ć' | 'Č' | 'ç' | 'ć' | 'č' => "c",
        'Ď' | 'Đ' | 'ď' | 'đ' => "d",
        'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ė' | 'Ę' | 'Ě' | 'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę'
        | 'ě' => "e",
        'Ğ' | 'Ģ' | 'ğ' | 'ģ' => "g",
        'Ì' | 'Í' | 'Î' | 'Ï' | 'Ī' | 'İ' | 'ì' | 'í' | 'î' | 'ï' | 'ī' | 'ı' => "i",
        'Ķ' | 'ķ' => "k",
        'Ļ' | 'Ľ' | 'Ł' | 'ļ' | 'ľ' | 'ł' => "l",
        'Ñ' | 'Ń' | 'Ņ' | 'Ň' | 'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' | 'Ő' | 'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ő' => {
            "o"
        }
        'Œ' | 'œ' => "oe",
        'Ř' | 'ř' => "r",
        'Ś' | 'Ş' | 'Š' | 'Ș' | 'ś' | 'ş' | 'š' | 'ș' => "s",
        'ß' => "ss",
        'Ţ' | 'Ť' | 'Ț' | 'ţ' | 'ť' | 'ț' => "t",
        'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ū' | 'Ŭ' | 'Ů' | 'Ű' | 'Ų' | 'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ŭ'
        | 'ů' | 'ű' | 'ų' => "u",
        'Ý' | 'Ÿ' | 'ý' | 'ÿ' => "y",
        'Ź' | 'Ż' | 'Ž' | 'ź' | 'ż' | 'ž' => "z",
        'Ά' | 'ά' => "α",
        'Έ' | 'έ' => "ε",
        'Ή' | 'ή' => "η",
        'Ί' | 'ί' | 'ϊ' | 'ΐ' => "ι",
        'Ό' | 'ό' => "ο",
        'Ύ' | 'ύ' | 'ϋ' | 'ΰ' => "υ",
        'Ώ' | 'ώ' => "ω",
        'ς' => "σ",
        _ => {
            folded.extend(character.to_lowercase());
            return;
        }
    };
    folded.push_str(replacement);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> &'static PlaceIndex {
        static INDEX: OnceLock<PlaceIndex> = OnceLock::new();
        INDEX.get_or_init(|| PlaceIndex::new(None, &TimeZoneFinder::new()))
    }

    fn result(name: &str, latitude: f64, longitude: f64) -> PlaceSearchResult {
        PlaceSearchResult {
            name: name.into(),
            context: SmolStr::default(),
            coordinates: GeoCoordinates::from_degrees(latitude, longitude).unwrap(),
        }
    }

    #[test]
    fn folding_removes_case_diacritics_and_punctuation() {
        assert_eq!(folded_words("Köln Hbf"), ["koln", "hbf"]);
        assert_eq!(folded_words("'s-Hertogenbosch"), ["s", "hertogenbosch"]);
        assert_eq!(folded_words("Gießen"), ["giessen"]);
        assert_eq!(folded_words("Łódź Fabryczna"), ["lodz", "fabryczna"]);
        assert_eq!(folded_words("İstanbul"), ["istanbul"]);
    }

    #[test]
    fn an_english_name_finds_the_city_with_its_country() {
        let results = index().search("vienna");
        assert_eq!(results[0].name, "Vienna");
        assert_eq!(results[0].context, "Austria");
    }

    #[test]
    fn a_local_name_without_diacritics_shows_the_local_name() {
        let results = index().search("koln");
        assert_eq!(results[0].name, "Köln");
        let results = index().search("zurich");
        assert_eq!(results[0].name, "Zürich");
    }

    #[test]
    fn a_prefix_finds_the_bigger_city_first() {
        let results = index().search("fra");
        assert_eq!(results[0].name, "Frankfurt am Main");
        assert_eq!(results[1].name, "Frankfurt (Oder)");
    }

    #[test]
    fn every_query_word_has_to_match() {
        assert!(!index().search("frankfurt main").is_empty());
        assert!(index().search("frankfurt nowhere").is_empty());
    }

    #[test]
    fn a_query_without_letters_finds_nothing() {
        assert!(index().search(" - ").is_empty());
    }

    #[test]
    fn online_results_that_the_local_search_has_are_left_out() {
        let extras = index().online_extras(
            "vienna",
            [
                result("Vienna", 48.2083, 16.3731),
                result("Vienna", 38.9012, -77.2653),
            ],
        );
        assert_eq!(extras.len(), 1);
        assert!(extras[0].coordinates.longitude.degrees() < 0.0);
    }

    #[test]
    fn a_differently_named_result_at_the_same_point_is_the_same_place() {
        let left = result("Wien Hbf", 48.1851, 16.3762);
        let right = result("Wien Hauptbahnhof", 48.1852, 16.3763);
        assert!(results_are_same_place(&left, &right));
        let far = result("Wien Meidling", 48.1747, 16.3333);
        assert!(!results_are_same_place(&left, &far));
    }
}
