//! Step 2: loads the rail part of a feed into the typed tables.

use chrono::NaiveDate;

use crate::BuildError;
use crate::config::{FeedConfig, FeedStationCodeScheme, StationKeyRule};
use crate::database::{WorkDatabase, sql_string};
use crate::feed_archive::{FeedArchive, column_or_null};

/// The days a bundle covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidityWindow {
    /// First day, which is day number 0.
    pub start: NaiveDate,
    /// Number of days.
    pub days: u16,
}

/// GTFS route types that count as rail: the basic type 2 and the extended types 100 to 117.
pub const RAIL_ROUTE_TYPE_FILTER: &str =
    "(TRY_CAST(route_type AS INTEGER) = 2 OR TRY_CAST(route_type AS INTEGER) BETWEEN 100 AND 117)";

/// Loads the rail routes, their trips, stop times, all stops and the service days of a feed.
///
/// # Errors
///
/// Returns an error when a feed file cannot be read or a statement fails.
pub fn load_feed(
    database: &WorkDatabase,
    archive: &FeedArchive,
    feed: &FeedConfig,
    window: ValidityWindow,
) -> Result<(), BuildError> {
    let feed_id = sql_string(&feed.mirror_name);
    let csv = |file_name: &str| -> Result<String, BuildError> {
        let location = archive.csv_location(file_name, &database.zip_reading)?;
        Ok(format!(
            "read_csv({location}, header = true, all_varchar = true, delim = ',', quote = '\"', \
             escape = '\"', null_padding = true, strict_mode = false)"
        ))
    };

    let agency_columns = archive.columns("agency.txt")?;
    let route_columns = archive.columns("routes.txt")?;
    let trip_columns = archive.columns("trips.txt")?;
    let stop_time_columns = archive.columns("stop_times.txt")?;
    let stop_columns = archive.columns("stops.txt")?;

    database.run(&format!(
        "INSERT INTO feed_agencies
         SELECT {feed_id}, {agency_id}, {agency_name}, {agency_timezone} FROM {agencies};

         INSERT INTO feed_routes
         SELECT {feed_id}, route_id, {route_agency_id}, {route_short_name}, {route_desc},
                {route_long_name},
                TRY_CAST(route_type AS INTEGER)
         FROM {routes}
         WHERE {RAIL_ROUTE_TYPE_FILTER};

         INSERT INTO feed_trips
         SELECT {feed_id}, trip_id, route_id, service_id, {trip_short_name}, {trip_headsign}
         FROM {trips}
         WHERE route_id IN (SELECT route_id FROM feed_routes WHERE feed_id = {feed_id});",
        agency_id = column_or_null(&agency_columns, "agency_id"),
        agency_name = column_or_null(&agency_columns, "agency_name"),
        agency_timezone = column_or_null(&agency_columns, "agency_timezone"),
        agencies = csv("agency.txt")?,
        route_agency_id = column_or_null(&route_columns, "agency_id"),
        route_short_name = column_or_null(&route_columns, "route_short_name"),
        route_desc = column_or_null(&route_columns, "route_desc"),
        route_long_name = column_or_null(&route_columns, "route_long_name"),
        routes = csv("routes.txt")?,
        trip_short_name = column_or_null(&trip_columns, "trip_short_name"),
        trip_headsign = column_or_null(&trip_columns, "trip_headsign"),
        trips = csv("trips.txt")?,
    ))?;

    // Stops without any time are timepoints GTFS lets consumers interpolate. Rail feeds rarely
    // have them, and the builder drops them.
    database.run(&format!(
        "INSERT INTO feed_stop_times
         SELECT {feed_id}, trip_id, CAST(stop_sequence AS INTEGER), stop_id,
                gtfs_seconds(coalesce(nullif(arrival_time, ''), departure_time)),
                gtfs_seconds(coalesce(nullif(departure_time, ''), arrival_time)),
                coalesce({pickup_type}, '') <> '1',
                coalesce({drop_off_type}, '') <> '1'
         FROM {stop_times}
         WHERE trip_id IN (SELECT trip_id FROM feed_trips WHERE feed_id = {feed_id})
           AND coalesce(nullif(arrival_time, ''), nullif(departure_time, '')) IS NOT NULL;",
        pickup_type = column_or_null(&stop_time_columns, "pickup_type"),
        drop_off_type = column_or_null(&stop_time_columns, "drop_off_type"),
        stop_times = csv("stop_times.txt")?,
    ))?;

    let parent_station = column_or_null(&stop_columns, "parent_station");
    let station_key = match &feed.station_key {
        StationKeyRule::ParentStation => format!("coalesce(nullif({parent_station}, ''), stop_id)"),
        StationKeyRule::StopIdPattern(pattern) => format!(
            "coalesce(nullif(regexp_extract(stop_id, {}, 1), ''), nullif({parent_station}, ''), stop_id)",
            sql_string(pattern)
        ),
    };
    let station_code = station_code_expression(feed, &parent_station);
    database.run(&format!(
        "INSERT INTO feed_stops
         SELECT {feed_id}, stop_id, {stop_name}, TRY_CAST(stop_lat AS DOUBLE),
                TRY_CAST(stop_lon AS DOUBLE), {station_key}, {station_code}
         FROM {stops};

         INSERT INTO feed_config
         SELECT {feed_id}, {scheme},
                (SELECT any_value(agency_timezone) FROM feed_agencies WHERE feed_id = {feed_id}),
                {namespace};",
        stop_name = column_or_null(&stop_columns, "stop_name"),
        stops = csv("stops.txt")?,
        scheme = sql_string(station_code_scheme_name(feed.station_code)),
        namespace = feed
            .station_key_namespace
            .as_deref()
            .map_or_else(|| "NULL".to_owned(), sql_string),
    ))?;

    load_service_days(database, archive, &feed_id, window, &csv)?;
    Ok(())
}

/// Expands `calendar.txt` and `calendar_dates.txt` into one row per service and day of the
/// validity window, for the services the feed's rail trips use.
fn load_service_days(
    database: &WorkDatabase,
    archive: &FeedArchive,
    feed_id: &str,
    window: ValidityWindow,
    csv: &impl Fn(&str) -> Result<String, BuildError>,
) -> Result<(), BuildError> {
    let regular_days = if archive.has("calendar.txt") {
        format!(
            "SELECT calendar.service_id, window_days.service_day
             FROM {} AS calendar
             JOIN window_days
               ON window_days.service_date
                  BETWEEN gtfs_date(calendar.start_date) AND gtfs_date(calendar.end_date)
             WHERE CASE isodow(window_days.service_date)
                       WHEN 1 THEN calendar.monday WHEN 2 THEN calendar.tuesday
                       WHEN 3 THEN calendar.wednesday WHEN 4 THEN calendar.thursday
                       WHEN 5 THEN calendar.friday WHEN 6 THEN calendar.saturday
                       ELSE calendar.sunday
                   END = '1'",
            csv("calendar.txt")?
        )
    } else {
        "SELECT NULL::VARCHAR AS service_id, NULL::SMALLINT AS service_day WHERE false".to_owned()
    };
    let date_changes = if archive.has("calendar_dates.txt") {
        format!(
            "SELECT service_id, gtfs_date(date) AS service_date, exception_type FROM {}",
            csv("calendar_dates.txt")?
        )
    } else {
        "SELECT NULL::VARCHAR AS service_id, NULL::DATE AS service_date, \
         NULL::VARCHAR AS exception_type WHERE false"
            .to_owned()
    };
    database.run(&format!(
        "INSERT INTO feed_service_days
         WITH window_days AS (
             SELECT CAST(day_number AS SMALLINT) AS service_day,
                    DATE {start} + CAST(day_number AS INTEGER) AS service_date
             FROM range(0, {days}) AS days(day_number)
         ),
         regular_days AS ({regular_days}),
         date_changes AS ({date_changes}),
         added_days AS (
             SELECT date_changes.service_id, window_days.service_day
             FROM date_changes JOIN window_days USING (service_date)
             WHERE trim(date_changes.exception_type) = '1'
         ),
         removed_days AS (
             SELECT date_changes.service_id, window_days.service_day
             FROM date_changes JOIN window_days USING (service_date)
             WHERE trim(date_changes.exception_type) = '2'
         ),
         active_days AS (
             (SELECT * FROM regular_days UNION SELECT * FROM added_days)
             EXCEPT SELECT * FROM removed_days
         )
         SELECT {feed_id}, service_id, service_day
         FROM active_days
         WHERE service_id IN (SELECT service_id FROM feed_trips WHERE feed_id = {feed_id});",
        start = sql_string(&window.start.to_string()),
        days = window.days,
    ))?;
    Ok(())
}

/// Returns the name the SQL steps use for a station code scheme.
pub(crate) fn station_code_scheme_name(scheme: FeedStationCodeScheme) -> &'static str {
    match scheme {
        FeedStationCodeScheme::NoStationCodes => "none",
        FeedStationCodeScheme::UicInStopId | FeedStationCodeScheme::UicInPrivateCode => "uic",
        FeedStationCodeScheme::EvaInStopId => "eva",
        FeedStationCodeScheme::Uic8InStopId => "uic8",
        FeedStationCodeScheme::EnturStopPlace => "entur",
    }
}

/// Returns the SQL expression that reads the station code of a stop, or `NULL` when the feed's
/// stops carry none. `parent_station` is the expression for the stop's parent station.
///
/// Platform IDs often add a suffix to the station code, as in `8891140_1`, so the default is
/// the leading digits. SNCF IDs end in the code, as in `StopArea:OCE87313759`.
fn station_code_expression(feed: &FeedConfig, parent_station: &str) -> String {
    let code_from_stop_id = |default_pattern: &str| {
        let pattern = feed
            .station_code_pattern
            .as_deref()
            .unwrap_or(default_pattern);
        format!(
            "nullif(regexp_extract(stop_id, {}, 1), '')",
            sql_string(pattern)
        )
    };
    let code = match feed.station_code {
        FeedStationCodeScheme::NoStationCodes | FeedStationCodeScheme::UicInPrivateCode => {
            return "NULL".to_owned();
        }
        FeedStationCodeScheme::UicInStopId => code_from_stop_id(r"^(\d{7})"),
        FeedStationCodeScheme::EvaInStopId => code_from_stop_id(r"^(\d+)"),
        FeedStationCodeScheme::Uic8InStopId => code_from_stop_id(r"(\d{8})$"),
        // The Trainline dataset writes the separators of a stop place ID as bars.
        FeedStationCodeScheme::EnturStopPlace => {
            format!("replace(coalesce(nullif({parent_station}, ''), stop_id), ':', '|')")
        }
    };
    if feed.station_code_prefix.is_empty() {
        code
    } else {
        // A stop without a code stays without one, because joining a missing value gives one.
        format!("{} || {code}", sql_string(&feed.station_code_prefix))
    }
}
