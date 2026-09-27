//! Steps 8 and 9: splits trips into RAPTOR routes and assembles the bundle.
//!
//! `BundleBuilder` from `btt-timetable` groups trips by stop pattern and splits them into routes
//! where no trip overtakes another. The plan grouped patterns in SQL first. Doing both in the
//! bundle builder keeps one implementation, which the router tests use too.

use std::path::Path;

use btt_model::{GeoCoordinates, Latitude, Longitude};
use btt_timetable::{
    BundleBuilder, BundleStationIndex, FeedAttributionInput, StationInput, TimetableBundle,
};
use std::num::NonZeroU32;

use crate::BuildError;
use crate::config::FeedConfig;
use crate::database::{WorkDatabase, sql_string};
use crate::load::ValidityWindow;
use crate::resolve::ResolvedTrip;

/// Assembles the bundle from the bundle stations, footpaths and resolved trips.
///
/// # Errors
///
/// Returns an error when a query fails or the bundle builder rejects the input.
pub fn assemble_bundle(
    database: &WorkDatabase,
    trips: Vec<ResolvedTrip>,
    attributions: Vec<FeedAttributionInput>,
    window: ValidityWindow,
    bundle_version: u32,
) -> Result<TimetableBundle, BuildError> {
    let mut builder = BundleBuilder::new(window.start, window.days, bundle_version)?;

    let mut statement = database.connection.prepare(
        "SELECT name, latitude, longitude, time_zone, trainline_station_id
         FROM bundle_stations ORDER BY bundle_station_id",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let latitude: f64 = row.get(1)?;
        let longitude: f64 = row.get(2)?;
        let trainline_station_id: Option<i32> = row.get(4)?;
        builder.add_station(StationInput {
            name: row.get(0)?,
            // Feed coordinates were checked on load, so a failure here means a corrupt row,
            // which gets the zero point rather than stopping the build.
            coordinates: GeoCoordinates::from_degrees(latitude, longitude).unwrap_or(
                GeoCoordinates {
                    latitude: Latitude::ZERO,
                    longitude: Longitude::ZERO,
                },
            ),
            time_zone: row.get(3)?,
            trainline_station_id: trainline_station_id
                .and_then(|id| u32::try_from(id).ok())
                .and_then(NonZeroU32::new),
        })?;
    }

    let mut statement = database.connection.prepare(
        "SELECT from_station, to_station, duration_minutes FROM footpaths
         ORDER BY from_station, duration_minutes, to_station",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let duration_minutes: i16 = row.get(2)?;
        builder.add_footpath(
            BundleStationIndex(row.get(0)?),
            BundleStationIndex(row.get(1)?),
            u16::try_from(duration_minutes).unwrap_or(1),
        );
    }

    for resolved in trips {
        builder.add_trip(resolved.trip);
    }
    for attribution in attributions {
        builder.add_attribution(attribution);
    }
    Ok(builder.build()?)
}

/// Reads the license of a feed from the Transitous feed configuration of its country.
///
/// Falls back to an unknown license when the file has no entry with a license for the feed,
/// so a gap in the configuration shows up in the bundle instead of stopping the build.
pub fn feed_attribution(
    database: &WorkDatabase,
    feed: &FeedConfig,
    country_config: &Path,
) -> FeedAttributionInput {
    let path = country_config.display().to_string().replace('\\', "/");
    // JSON path functions tolerate missing fields, where `read_json` infers one struct type for
    // all sources and fails on a field that only some of them have.
    let query = format!(
        "WITH document AS (SELECT CAST(content AS JSON) AS content FROM read_text({})),
         sources AS (
             SELECT unnest(json_extract(content, '$.sources[*]')) AS source FROM document
         )
         SELECT json_extract_string(source, '$.license.\"spdx-identifier\"'),
                coalesce(json_extract_string(source, '$.license.url'),
                         json_extract_string(source, '$.url'))
         FROM sources
         WHERE json_extract_string(source, '$.name') = {}
           AND json_extract(source, '$.license') IS NOT NULL
         LIMIT 1",
        sql_string(&path),
        sql_string(feed.source_name()),
    );
    let found: Result<(Option<String>, Option<String>), _> =
        database
            .connection
            .query_row(&query, [], |row| Ok((row.get(0)?, row.get(1)?)));
    let (license, url) = match found {
        Ok(values) => values,
        Err(error) => {
            tracing::warn!(feed = feed.mirror_name, %error, "no license found in the Transitous configuration");
            (None, None)
        }
    };
    FeedAttributionInput {
        feed_name: feed.mirror_name.clone(),
        license: license.unwrap_or_else(|| "unknown".to_owned()),
        url: url.unwrap_or_default(),
    }
}
