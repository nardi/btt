//! The dictionary of stations that links refer to by number.
//!
//! The dictionary is the file `data/stations.tsv`, one station per line as its name, latitude and
//! longitude in microdegrees and time zone, separated by tabs. The web viewer reads the same file.
//! A link names a station by its line, so lines are only ever appended, and a station that the
//! dictionary lacks travels as a literal. The example `station_dictionary` of `btt-planner` writes
//! the file from a timetable bundle.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::plan::LegPlanStation;

/// The dictionary file, compiled in.
const DICTIONARY_FILE: &str = include_str!("../../data/stations.tsv");

/// Furthest a station may lie from its dictionary entry and still use it, in microdegrees on
/// either axis. That is about two kilometers. A bundle moves a station a little when its feeds
/// change, and the link then spends a few bytes on the difference.
const MAXIMUM_OFFSET_MICRODEGREES: i64 = 20_000;

/// A line of the dictionary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DictionaryStation {
    /// Name of the station.
    pub name: &'static str,
    /// Latitude in microdegrees.
    pub latitude: i32,
    /// Longitude in microdegrees.
    pub longitude: i32,
    /// IANA time zone of the station.
    pub time_zone: &'static str,
}

/// A station that matches a dictionary entry, with how far it lies from the entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DictionaryMatch {
    /// Position of the entry in the dictionary.
    pub index: usize,
    /// The station's latitude minus the entry's, in microdegrees.
    pub latitude_offset: i32,
    /// The station's longitude minus the entry's, in microdegrees.
    pub longitude_offset: i32,
}

/// The stations of the dictionary file, and a lookup by name.
#[derive(Debug)]
pub(super) struct StationDictionary {
    entries: Vec<DictionaryStation>,
    positions_by_name: HashMap<&'static str, Vec<usize>>,
}

/// Returns the dictionary, which is parsed on first use.
pub(super) fn dictionary() -> &'static StationDictionary {
    static DICTIONARY: OnceLock<StationDictionary> = OnceLock::new();
    DICTIONARY.get_or_init(|| StationDictionary::parse(DICTIONARY_FILE))
}

impl StationDictionary {
    /// Reads the lines of a dictionary file. A line that is not a station is skipped, which
    /// would shift the numbers of the stations after it, so a test checks that the file has none.
    fn parse(text: &'static str) -> Self {
        let mut entries = Vec::new();
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            if let [name, latitude, longitude, time_zone] = fields[..]
                && let (Ok(latitude), Ok(longitude)) = (latitude.parse(), longitude.parse())
            {
                entries.push(DictionaryStation {
                    name,
                    latitude,
                    longitude,
                    time_zone,
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

    /// Returns the station at a position.
    pub(super) fn get(&self, index: usize) -> Option<&DictionaryStation> {
        self.entries.get(index)
    }

    /// Returns the number of stations.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Finds the entry that a station stands for: one with its name and its time zone that lies
    /// the nearest to it, within `MAXIMUM_OFFSET_MICRODEGREES`. Among entries as near, the one
    /// earlier in the dictionary wins, which is the one with the shorter number.
    pub(super) fn find(&self, station: &LegPlanStation) -> Option<DictionaryMatch> {
        let latitude = station.coordinates.latitude.microdegrees();
        let longitude = station.coordinates.longitude.microdegrees();
        self.positions_by_name
            .get(station.name.as_str())?
            .iter()
            .filter_map(|&index| {
                let entry = &self.entries[index];
                let latitude_offset = i64::from(latitude) - i64::from(entry.latitude);
                let longitude_offset = i64::from(longitude) - i64::from(entry.longitude);
                (entry.time_zone == station.time_zone.as_str()
                    && latitude_offset.abs() <= MAXIMUM_OFFSET_MICRODEGREES
                    && longitude_offset.abs() <= MAXIMUM_OFFSET_MICRODEGREES)
                    .then_some((
                        latitude_offset.abs() + longitude_offset.abs(),
                        DictionaryMatch {
                            index,
                            // The offsets are within 20,000, so they fit in an `i32`.
                            latitude_offset: i32::try_from(latitude_offset).unwrap_or(0),
                            longitude_offset: i32::try_from(longitude_offset).unwrap_or(0),
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

    #[test]
    fn every_line_of_the_file_is_a_station() {
        let lines = DICTIONARY_FILE.lines().count();
        assert_eq!(dictionary().len(), lines);
        assert!(lines >= 1000);
    }

    #[test]
    fn a_station_of_the_dictionary_finds_itself() {
        let entry = dictionary().get(0).unwrap();
        let station = LegPlanStation {
            name: entry.name.into(),
            coordinates: GeoCoordinates::from_degrees(
                f64::from(entry.latitude) / 1e6,
                f64::from(entry.longitude) / 1e6,
            )
            .unwrap(),
            time_zone: entry.time_zone.into(),
        };
        let found = dictionary().find(&station).unwrap();
        assert_eq!(found.index, 0);
        assert_eq!((found.latitude_offset, found.longitude_offset), (0, 0));
    }

    #[test]
    fn a_station_with_another_time_zone_or_far_away_is_not_found() {
        let entry = dictionary().get(0).unwrap();
        let near = GeoCoordinates::from_degrees(
            f64::from(entry.latitude) / 1e6,
            f64::from(entry.longitude) / 1e6,
        )
        .unwrap();
        let other_zone = LegPlanStation {
            name: entry.name.into(),
            coordinates: near,
            time_zone: "Pacific/Auckland".into(),
        };
        assert_eq!(dictionary().find(&other_zone), None);
        let far = LegPlanStation {
            name: entry.name.into(),
            coordinates: GeoCoordinates::from_degrees(0.0, 0.0).unwrap(),
            time_zone: entry.time_zone.into(),
        };
        assert_eq!(dictionary().find(&far), None);
    }

    #[test]
    fn a_station_a_little_off_its_entry_reports_the_offset() {
        let entry = dictionary().get(1).unwrap();
        let station = LegPlanStation {
            name: entry.name.into(),
            coordinates: GeoCoordinates::from_degrees(
                f64::from(entry.latitude + 120) / 1e6,
                f64::from(entry.longitude - 80) / 1e6,
            )
            .unwrap(),
            time_zone: entry.time_zone.into(),
        };
        let found = dictionary().find(&station).unwrap();
        assert_eq!(found.index, 1);
        assert_eq!((found.latitude_offset, found.longitude_offset), (120, -80));
    }
}
