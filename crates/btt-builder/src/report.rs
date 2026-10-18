//! Step 10: the build report, as Markdown for people and Parquet for trend queries.

use std::fmt::Write as _;
use std::path::Path;

use btt_timetable::TimetableBundle;
use btt_timetable::format::{
    ArchivedBundleStation, ArchivedBundleTrainLabel, ArchivedBundleTrip, ArchivedFootpath,
    ArchivedPackedPatternStop, ArchivedRaptorRoute, ArchivedRouteStopPosition,
    ArchivedServiceDayBitset, ArchivedStopEventTimes,
};

use crate::BuildError;
use crate::config::FeedConfig;
use crate::database::{WorkDatabase, sql_string};
use crate::dedup::MergeCounts;
use crate::download::CachedReuse;

/// Most uncertain station matches the report lists.
const LISTED_UNCERTAIN_MATCHES: i64 = 300;

/// One number of the build, such as the trips of a feed.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildStatistic {
    /// What the number is about, such as a feed name or `bundle`.
    pub scope: String,
    /// What the number counts.
    pub statistic: String,
    /// The number.
    pub value: i64,
}

/// Collects the statistics of a build and writes the report files.
///
/// # Errors
///
/// Returns an error when a query fails or a file cannot be written.
pub fn write_report(
    database: &WorkDatabase,
    feeds: &[&FeedConfig],
    merge_counts: &MergeCounts,
    bundle: &TimetableBundle,
    bundle_file_bytes: u64,
    cached_reuses: &[CachedReuse],
    output_directory: &Path,
) -> Result<Vec<BuildStatistic>, BuildError> {
    let mut statistics = feed_statistics(database, feeds)?;
    for (&(first, second), &count) in merge_counts {
        statistics.push(BuildStatistic {
            scope: format!(
                "{} and {}",
                feeds[first].mirror_name, feeds[second].mirror_name
            ),
            statistic: "merged trains".to_owned(),
            value: to_statistic(count),
        });
    }
    statistics.extend(bundle_statistics(bundle, bundle_file_bytes));

    let mut markdown = String::from("# Timetable build report\n\n");
    markdown.push_str(&cached_reuses_section(cached_reuses));
    markdown.push_str("## Statistics\n\n");
    markdown.push_str("| Scope | Statistic | Value |\n|---|---|---:|\n");
    for statistic in &statistics {
        let _ = writeln!(
            markdown,
            "| {} | {} | {} |",
            statistic.scope, statistic.statistic, statistic.value
        );
    }
    markdown.push_str(&uncertain_matches_section(database)?);
    std::fs::write(output_directory.join("report.md"), markdown)?;

    write_statistics_parquet(
        database,
        &statistics,
        &output_directory.join("build_statistics.parquet"),
    )?;
    Ok(statistics)
}

/// Lists the downloads that came from the cache, so a stale feed shows in the report. The
/// section is empty when every download is up to date.
fn cached_reuses_section(cached_reuses: &[CachedReuse]) -> String {
    if cached_reuses.is_empty() {
        return String::new();
    }
    let mut section = String::from(
        "## Stale downloads\n\n\
         These files could not be checked or downloaded, so the build used the copy in the cache. \
         The bundle can miss changes made after that copy.\n\n\
         | File | Reason |\n|---|---|\n",
    );
    for reuse in cached_reuses {
        let _ = writeln!(section, "| {} | {} |", reuse.url, reuse.reason);
    }
    section.push('\n');
    section
}

/// Counts routes, trips, stop times, station matches and joined stations per feed.
fn feed_statistics(
    database: &WorkDatabase,
    feeds: &[&FeedConfig],
) -> Result<Vec<BuildStatistic>, BuildError> {
    let counts = [
        (
            "rail routes",
            "SELECT count(*) FROM feed_routes WHERE feed_id = {feed}",
        ),
        (
            "rail trips",
            "SELECT count(*) FROM feed_trips WHERE feed_id = {feed}",
        ),
        (
            "stop times",
            "SELECT count(*) FROM feed_stop_times WHERE feed_id = {feed}",
        ),
        (
            "stations",
            "SELECT count(*) FROM feed_stations WHERE feed_id = {feed}",
        ),
        (
            "stations matched by code",
            "SELECT count(*) FROM station_matches WHERE feed_id = {feed} AND match_kind = 'code'",
        ),
        (
            "stations matched by name and distance",
            "SELECT count(*) FROM station_matches
             WHERE feed_id = {feed} AND match_kind = 'name and distance'",
        ),
        (
            "uncertain station matches",
            "SELECT count(*) FROM uncertain_station_matches WHERE feed_id = {feed}",
        ),
        (
            "stations joined to another by distance",
            "SELECT count(DISTINCT numbers.feed_station_number) FROM station_group_links AS links
             JOIN feed_station_numbers AS numbers
               ON numbers.feed_station_number IN (links.first_station, links.second_station)
             WHERE numbers.feed_id = {feed} AND links.link_kind = 'distance'",
        ),
        (
            "stations joined to another by shared key",
            "SELECT count(DISTINCT numbers.feed_station_number) FROM station_group_links AS links
             JOIN feed_station_numbers AS numbers
               ON numbers.feed_station_number IN (links.first_station, links.second_station)
             WHERE numbers.feed_id = {feed} AND links.link_kind = 'shared key'",
        ),
        (
            "stations joined to another by Trainline station",
            "SELECT count(DISTINCT numbers.feed_station_number) FROM station_group_links AS links
             JOIN feed_station_numbers AS numbers
               ON numbers.feed_station_number IN (links.first_station, links.second_station)
             WHERE numbers.feed_id = {feed} AND links.link_kind = 'Trainline station'",
        ),
    ];
    let mut statistics = Vec::new();
    for feed in feeds {
        let feed_literal = sql_string(&feed.mirror_name);
        for (statistic, query) in counts {
            let value = database.count(&query.replace("{feed}", &feed_literal))?;
            statistics.push(BuildStatistic {
                scope: feed.mirror_name.clone(),
                statistic: statistic.to_owned(),
                value,
            });
        }
    }
    Ok(statistics)
}

/// Counts the bundle tables and their sizes in the archive.
fn bundle_statistics(bundle: &TimetableBundle, bundle_file_bytes: u64) -> Vec<BuildStatistic> {
    let table = |statistic: &str, value: usize| BuildStatistic {
        scope: "bundle".to_owned(),
        statistic: statistic.to_owned(),
        value: to_statistic(value),
    };
    let bytes = |name: &str, count: usize, element_bytes: usize| {
        table(&format!("{name} bytes"), count * element_bytes)
    };
    vec![
        table("stations", bundle.stations.len()),
        table("RAPTOR routes", bundle.raptor_routes.len()),
        table("trips", bundle.trips.len()),
        table("stop events", bundle.stop_events.len()),
        table("footpaths", bundle.footpaths_by_station.values.len()),
        table(
            "distinct service day sets",
            bundle.distinct_service_days.len(),
        ),
        table("train labels", bundle.train_labels.len()),
        bytes(
            "stations",
            bundle.stations.len(),
            size_of::<ArchivedBundleStation>(),
        ),
        bytes(
            "RAPTOR routes",
            bundle.raptor_routes.len(),
            size_of::<ArchivedRaptorRoute>(),
        ),
        bytes(
            "pattern stops",
            bundle.pattern_stops.len(),
            size_of::<ArchivedPackedPatternStop>(),
        ),
        bytes("trips", bundle.trips.len(), size_of::<ArchivedBundleTrip>()),
        bytes(
            "stop events",
            bundle.stop_events.len(),
            size_of::<ArchivedStopEventTimes>(),
        ),
        bytes(
            "route stops by station",
            bundle.route_stops_by_station.values.len(),
            size_of::<ArchivedRouteStopPosition>(),
        ),
        bytes(
            "footpaths",
            bundle.footpaths_by_station.values.len(),
            size_of::<ArchivedFootpath>(),
        ),
        bytes(
            "service days by station",
            bundle.service_days_by_station.len(),
            size_of::<ArchivedServiceDayBitset>(),
        ),
        bytes(
            "train labels",
            bundle.train_labels.len(),
            size_of::<ArchivedBundleTrainLabel>(),
        ),
        table("string pool bytes", bundle.string_pool.len()),
        BuildStatistic {
            scope: "bundle".to_owned(),
            statistic: "file bytes".to_owned(),
            value: i64::try_from(bundle_file_bytes).unwrap_or(i64::MAX),
        },
    ]
}

/// Lists the uncertain station matches, most similar first.
fn uncertain_matches_section(database: &WorkDatabase) -> Result<String, BuildError> {
    let mut section = String::from(
        "\n## Uncertain station matches\n\n\
         These feed stations have a Trainline station within 300 m whose name is similar but not \
         similar enough. They stay unmatched.\n\n\
         | Feed | Feed name | Trainline name | Distance (m) | Similarity |\n|---|---|---|---:|---:|\n",
    );
    let mut statement = database.connection.prepare(&format!(
        "SELECT feed_id, feed_name, trainline_name, distance_meters, name_similarity
         FROM uncertain_station_matches
         ORDER BY name_similarity DESC
         LIMIT {LISTED_UNCERTAIN_MATCHES}"
    ))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let feed_id: String = row.get(0)?;
        let feed_name: String = row.get(1)?;
        let trainline_name: String = row.get(2)?;
        let distance: f64 = row.get(3)?;
        let similarity: f64 = row.get(4)?;
        let _ = writeln!(
            section,
            "| {feed_id} | {feed_name} | {trainline_name} | {distance:.0} | {similarity:.3} |"
        );
    }
    Ok(section)
}

/// Writes the statistics as Parquet, one row per scope and statistic.
fn write_statistics_parquet(
    database: &WorkDatabase,
    statistics: &[BuildStatistic],
    path: &Path,
) -> Result<(), BuildError> {
    database.run(
        "CREATE OR REPLACE TABLE build_statistics (scope VARCHAR, statistic VARCHAR, value BIGINT)",
    )?;
    let mut appender = database.connection.appender("build_statistics")?;
    for statistic in statistics {
        appender.append_row(duckdb::params![
            statistic.scope,
            statistic.statistic,
            statistic.value
        ])?;
    }
    drop(appender);
    let path = path.display().to_string().replace('\\', "/");
    database.run(&format!(
        "COPY build_statistics TO {} (FORMAT parquet, COMPRESSION zstd)",
        sql_string(&path)
    ))?;
    Ok(())
}

/// Converts a count to a statistic value. Counts never come near `i64::MAX`.
fn to_statistic(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stale_downloads_section_lists_each_file_and_vanishes_without_any() {
        assert_eq!(cached_reuses_section(&[]), "");
        let section = cached_reuses_section(&[CachedReuse {
            url: "https://example.org/feed.gtfs.zip".to_owned(),
            reason: "the check for a newer version failed: timed out".to_owned(),
        }]);
        assert!(section.starts_with("## Stale downloads"));
        assert!(section.contains(
            "| https://example.org/feed.gtfs.zip | the check for a newer version failed: timed out |"
        ));
    }
}
