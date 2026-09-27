//! Steps 3 and 4: station matching and footpaths, both in SQL.

use std::path::Path;

use crate::BuildError;
use crate::database::{WorkDatabase, sql_string};

/// Largest distance at which a Trainline station with a similar name matches.
const MATCH_RADIUS_METERS: f64 = 300.0;

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
         SET VARIABLE same_name_radius_meters = {SAME_NAME_RADIUS_METERS};
         SET VARIABLE accepted_similarity = {ACCEPTED_SIMILARITY};
         SET VARIABLE uncertain_similarity = {UNCERTAIN_SIMILARITY};",
        sql_string(&trainline_path),
        sql_string(&stopword_pattern(stopwords)),
    ))?;
    database.run(include_str!("../sql/03_match_stations.sql"))?;
    Ok(())
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
}
