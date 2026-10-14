//! Steps 3 and 4: station matching, station groups and footpaths.
//!
//! SQL matches feed stations to Trainline and finds pairs of feed stations that describe the
//! same station. Rust joins those pairs into groups, and SQL turns each group into a bundle
//! station.

use std::path::Path;

use btt_timetable::BundleStationIndex;

use crate::BuildError;
use crate::database::{DatabaseError, WorkDatabase, sql_string};
use crate::dedup::{NearbyStationPairs, SAME_STOP_RADIUS_METERS};

/// Largest distance between stations of different feeds that count as the same station, the
/// distance nigiri uses for equivalent stops.
const STATION_MERGE_RADIUS_METERS: f64 = 100.0;

/// Largest distance between a Trainline row and its parent for the row to count as a part of
/// the parent, such as a group of platforms. Farther rows are separate stations that Trainline
/// files under a main station.
const STATION_PART_RADIUS_METERS: f64 = 1000.0;

/// Largest distance at which a Trainline station with a similar name matches.
const MATCH_RADIUS_METERS: f64 = 300.0;

/// Largest distance between a feed station and the Trainline station its official code names.
/// Trainline coordinates are sometimes off by a kilometer, but a farther station means the code
/// belongs to another station.
const CODE_MATCH_RADIUS_METERS: f64 = 10_000.0;

/// Largest distance at which a Trainline station with the same normalized name still matches.
const SAME_NAME_RADIUS_METERS: f64 = 2000.0;

/// Name similarity from which a candidate within the radius counts as a match.
const ACCEPTED_SIMILARITY: f64 = 0.88;

/// Name similarity from which an unmatched candidate goes into the report as uncertain.
const UNCERTAIN_SIMILARITY: f64 = 0.75;

/// Largest distance between two stations that get a footpath.
const FOOTPATH_RADIUS_METERS: f64 = 5000.0;

/// Loads the Trainline stations, collapses feed stops into stations, matches them and builds
/// the bundle station table.
///
/// # Errors
///
/// Returns an error when a statement fails.
pub fn match_stations(
    database: &WorkDatabase,
    trainline_stations_csv: &Path,
    stopwords: &[String],
) -> Result<(), BuildError> {
    let trainline_path = trainline_stations_csv
        .display()
        .to_string()
        .replace('\\', "/");
    database.run(&format!(
        "CREATE TABLE trainline_raw AS
         SELECT * FROM read_csv({}, delim = ';', header = true, all_varchar = true, quote = '\"');
         SET VARIABLE stopword_pattern = {};
         SET VARIABLE match_radius_meters = {MATCH_RADIUS_METERS};
         SET VARIABLE code_match_radius_meters = {CODE_MATCH_RADIUS_METERS};
         SET VARIABLE same_name_radius_meters = {SAME_NAME_RADIUS_METERS};
         SET VARIABLE accepted_similarity = {ACCEPTED_SIMILARITY};
         SET VARIABLE uncertain_similarity = {UNCERTAIN_SIMILARITY};
         SET VARIABLE station_merge_radius_meters = {STATION_MERGE_RADIUS_METERS};
         SET VARIABLE station_part_radius_meters = {STATION_PART_RADIUS_METERS};",
        sql_string(&trainline_path),
        sql_string(&stopword_pattern(stopwords)),
    ))?;
    database.run(include_str!("../sql/03_match_stations.sql"))?;
    group_feed_stations(database)?;
    database.run(include_str!("../sql/03b_bundle_stations.sql"))?;
    Ok(())
}

/// Reads the pairs of bundle stations close enough to stand for the same stop of a train, from
/// the footpaths, which already hold the distance of every nearby pair in both directions.
///
/// # Errors
///
/// Returns an error when the query fails.
pub fn nearby_station_pairs(database: &WorkDatabase) -> Result<NearbyStationPairs, DatabaseError> {
    let mut statement = database.connection.prepare(&format!(
        "SELECT from_station, to_station FROM footpaths
         WHERE distance_meters <= {SAME_STOP_RADIUS_METERS}"
    ))?;
    let mut rows = statement.query([])?;
    let mut pairs = NearbyStationPairs::default();
    while let Some(row) = rows.next()? {
        pairs.0.insert((
            BundleStationIndex(row.get(0)?),
            BundleStationIndex(row.get(1)?),
        ));
    }
    Ok(pairs)
}

/// Joins the feed stations that `station_group_links` connects into groups, and writes each
/// station's group number to `feed_station_groups`. A group's number is the lowest feed station
/// number in it, which keeps the numbering the same for the same input.
fn group_feed_stations(database: &WorkDatabase) -> Result<(), DatabaseError> {
    let station_count = database.count("SELECT count(*) FROM feed_station_numbers")?;
    let mut groups = StationUnionFind::new(usize::try_from(station_count).unwrap_or(0));
    let mut statement = database
        .connection
        .prepare("SELECT first_station, second_station FROM station_group_links")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let first: u32 = row.get(0)?;
        let second: u32 = row.get(1)?;
        groups.union(first, second);
    }
    database.run(
        "CREATE TABLE feed_station_groups (feed_station_number INTEGER, group_number INTEGER)",
    )?;
    let mut appender = database.connection.appender("feed_station_groups")?;
    for station in 0..groups.len() {
        appender.append_row([station, groups.find(station)])?;
    }
    appender.flush()?;
    Ok(())
}

/// A union-find over feed station numbers, where each set's representative is its lowest
/// number.
struct StationUnionFind {
    /// The parent of each station, which is the station itself for a representative.
    parents: Vec<u32>,
}

impl StationUnionFind {
    /// Starts with every station in a set of its own.
    fn new(station_count: usize) -> Self {
        Self {
            parents: (0..u32::try_from(station_count).unwrap_or(u32::MAX)).collect(),
        }
    }

    /// Returns the number of stations.
    fn len(&self) -> u32 {
        u32::try_from(self.parents.len()).unwrap_or(u32::MAX)
    }

    /// Returns the representative of a station's set, and points the stations on the way
    /// straight at it.
    fn find(&mut self, station: u32) -> u32 {
        let mut root = station;
        while self.parents[root as usize] != root {
            root = self.parents[root as usize];
        }
        let mut current = station;
        while self.parents[current as usize] != root {
            let next = self.parents[current as usize];
            self.parents[current as usize] = root;
            current = next;
        }
        root
    }

    /// Joins the sets of two stations under the lower representative.
    fn union(&mut self, first: u32, second: u32) {
        let first_root = self.find(first);
        let second_root = self.find(second);
        let (lower, higher) = if first_root < second_root {
            (first_root, second_root)
        } else {
            (second_root, first_root)
        };
        self.parents[higher as usize] = lower;
    }
}

/// Adds footpaths between bundle stations that lie close to each other.
///
/// # Errors
///
/// Returns an error when a statement fails.
pub fn compute_footpaths(database: &WorkDatabase) -> Result<(), BuildError> {
    database.run(&format!(
        "SET VARIABLE footpath_radius_meters = {FOOTPATH_RADIUS_METERS};"
    ))?;
    database.run(include_str!("../sql/04_footpaths.sql"))?;
    Ok(())
}

/// Builds a regular expression that matches any stopword as a whole word. Longer words come
/// first, so "gare de" wins over "gare".
fn stopword_pattern(stopwords: &[String]) -> String {
    let mut words: Vec<String> = stopwords
        .iter()
        .map(|word| regex::escape(&word.to_lowercase()))
        .collect();
    words.sort_by_key(|word| std::cmp::Reverse(word.len()));
    if words.is_empty() {
        // A pattern that never matches, because nothing follows the end of the text.
        return "$^".to_owned();
    }
    format!(r"\b({})\b", words.join("|"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopword_pattern_prefers_longer_words() {
        let pattern = stopword_pattern(&["gare".to_owned(), "gare de".to_owned()]);
        assert_eq!(pattern, r"\b(gare de|gare)\b");
    }

    #[test]
    fn union_find_joins_chains_under_the_lowest_station() {
        let mut groups = StationUnionFind::new(6);
        groups.union(4, 2);
        groups.union(2, 5);
        groups.union(1, 3);
        let representatives: Vec<u32> = (0..groups.len())
            .map(|station| groups.find(station))
            .collect();
        assert_eq!(representatives, [0, 1, 2, 1, 2, 2]);
    }
}
