//! Step 5: resolves GTFS local times to UTC for every day a trip runs, and reads train labels.

use btt_timetable::{
    BundleStationIndex, ServiceDayBitset, TrainLabelInput, TripInput, TripStopInput,
};
use chrono::{Days, NaiveDate, TimeZone};
use chrono_tz::Tz;
use rustc_hash::FxHashMap;

use crate::BuildError;
use crate::config::{FeedConfig, TrainLabelField, TrainLabelRule};
use crate::database::WorkDatabase;
use crate::load::ValidityWindow;

/// Seconds in one day.
const SECONDS_PER_DAY: i64 = 86_400;

/// A trip after UTC resolution, ready for deduplication and the bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTrip {
    /// Position of the trip's feed in the configuration.
    pub feed_index: usize,
    /// Train number with leading zeros removed, which deduplication compares across feeds.
    /// Empty when the feed gives no number.
    pub train_number: String,
    /// Stops, times, days and label.
    pub trip: TripInput,
}

/// A stop of a trip as the feed gives it, before UTC resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FeedTripStop {
    station: BundleStationIndex,
    arrival_seconds: i32,
    departure_seconds: i32,
    boarding_allowed: bool,
    alighting_allowed: bool,
}

/// Fields of a trip that stop times do not carry.
#[derive(Debug)]
struct FeedTripHeader {
    service_id: String,
    trip_short_name: Option<String>,
    route_short_name: Option<String>,
    route_type: i32,
    agency_name: String,
    time_zone: Tz,
    trip_headsign: Option<String>,
    route_desc: Option<String>,
    /// Stop ID of the trip's first stop, which some feeds use to name the train's product.
    first_stop_id: Option<String>,
}

/// Reads every rail trip from the working database and resolves it to UTC.
///
/// A trip that runs on days with different UTC offsets, such as across a daylight saving
/// change, becomes one resolved trip per distinct list of UTC times.
///
/// # Errors
///
/// Returns an error when a query fails.
pub fn resolve_trips(
    database: &WorkDatabase,
    feeds: &[&FeedConfig],
    night_train_categories: &[String],
    window: ValidityWindow,
) -> Result<Vec<ResolvedTrip>, BuildError> {
    let feed_indices: FxHashMap<&str, usize> = feeds
        .iter()
        .enumerate()
        .map(|(index, feed)| (feed.mirror_name.as_str(), index))
        .collect();
    let service_days = read_service_days(database, &feed_indices)?;
    let headers = read_trip_headers(database, &feed_indices)?;

    let mut statement = database.connection.prepare(
        "SELECT stop_times.feed_id, stop_times.trip_id, station_ids.bundle_station_id,
                stop_times.arrival_seconds, stop_times.departure_seconds,
                stop_times.boarding_allowed, stop_times.alighting_allowed
         FROM feed_stop_times AS stop_times
         JOIN feed_stops AS stops USING (feed_id, stop_id)
         JOIN feed_station_bundle_ids AS station_ids USING (feed_id, station_key)
         ORDER BY stop_times.feed_id, stop_times.trip_id, stop_times.stop_sequence",
    )?;
    let mut rows = statement.query([])?;
    let mut resolved = Vec::new();
    let mut current_trip: Option<(usize, String)> = None;
    let mut stops: Vec<FeedTripStop> = Vec::new();
    let mut finish_trip = |trip_key: &(usize, String), stops: &[FeedTripStop]| {
        let Some(header) = headers.get(trip_key) else {
            return;
        };
        let days = service_days
            .get(&(trip_key.0, header.service_id.clone()))
            .copied()
            .unwrap_or_default();
        let feed = &feeds[trip_key.0];
        let (train_number, mut train_label) = train_label(&feed.train_label, header);
        train_label.offers_berths = offers_berths(
            header.route_type,
            &train_label.category,
            night_train_categories,
        );
        for (service_days, trip_stops) in resolve_to_utc(stops, days, header.time_zone, window) {
            resolved.push(ResolvedTrip {
                feed_index: trip_key.0,
                train_number: train_number.clone(),
                trip: TripInput {
                    stops: trip_stops,
                    service_days,
                    train_label: train_label.clone(),
                },
            });
        }
    };
    while let Some(row) = rows.next()? {
        let feed_id: String = row.get(0)?;
        let trip_id: String = row.get(1)?;
        let Some(&feed_index) = feed_indices.get(feed_id.as_str()) else {
            continue;
        };
        let trip_key = (feed_index, trip_id);
        if current_trip.as_ref() != Some(&trip_key) {
            if let Some(previous) = current_trip.take() {
                finish_trip(&previous, &stops);
            }
            stops.clear();
            current_trip = Some(trip_key);
        }
        let station = BundleStationIndex(row.get::<_, u32>(2)?);
        let stop = FeedTripStop {
            station,
            arrival_seconds: row.get(3)?,
            departure_seconds: row.get(4)?,
            boarding_allowed: row.get(5)?,
            alighting_allowed: row.get(6)?,
        };
        // Two platforms of one station in a row become a single stop.
        match stops.last_mut() {
            Some(previous) if previous.station == station => {
                previous.departure_seconds = stop.departure_seconds;
                previous.boarding_allowed = stop.boarding_allowed;
            }
            _ => stops.push(stop),
        }
    }
    if let Some(previous) = current_trip.take() {
        finish_trip(&previous, &stops);
    }
    Ok(resolved)
}

/// Reads the service days of every service as a bitset per feed and service ID.
fn read_service_days(
    database: &WorkDatabase,
    feed_indices: &FxHashMap<&str, usize>,
) -> Result<FxHashMap<(usize, String), ServiceDayBitset>, BuildError> {
    let mut statement = database
        .connection
        .prepare("SELECT feed_id, service_id, service_day FROM feed_service_days")?;
    let mut rows = statement.query([])?;
    let mut service_days: FxHashMap<(usize, String), ServiceDayBitset> = FxHashMap::default();
    while let Some(row) = rows.next()? {
        let feed_id: String = row.get(0)?;
        let Some(&feed_index) = feed_indices.get(feed_id.as_str()) else {
            continue;
        };
        let day: u16 = row.get(2)?;
        service_days
            .entry((feed_index, row.get(1)?))
            .or_default()
            .insert(day);
    }
    Ok(service_days)
}

/// Reads the fields of every rail trip that the stop times do not carry.
fn read_trip_headers(
    database: &WorkDatabase,
    feed_indices: &FxHashMap<&str, usize>,
) -> Result<FxHashMap<(usize, String), FeedTripHeader>, BuildError> {
    // A route without agency_id belongs to the feed's only agency, so any agency of the feed
    // fits. The QUALIFY keeps one row per trip either way.
    let mut statement = database.connection.prepare(
        "WITH first_stops AS (
             SELECT feed_id, trip_id, arg_min(stop_id, stop_sequence) AS first_stop_id
             FROM feed_stop_times
             GROUP BY feed_id, trip_id
         )
         SELECT trips.feed_id, trips.trip_id, trips.service_id, trips.trip_short_name,
                routes.route_short_name, coalesce(agencies.agency_name, ''),
                coalesce(agencies.agency_timezone, feed_config.default_time_zone, 'UTC'),
                routes.route_type, trips.trip_headsign, routes.route_desc,
                first_stops.first_stop_id
         FROM feed_trips AS trips
         JOIN feed_routes AS routes USING (feed_id, route_id)
         JOIN feed_config USING (feed_id)
         LEFT JOIN first_stops USING (feed_id, trip_id)
         LEFT JOIN feed_agencies AS agencies
           ON agencies.feed_id = trips.feed_id
          AND (agencies.agency_id = routes.agency_id OR routes.agency_id IS NULL)
         QUALIFY row_number() OVER (PARTITION BY trips.feed_id, trips.trip_id) = 1",
    )?;
    let mut rows = statement.query([])?;
    let mut headers = FxHashMap::default();
    while let Some(row) = rows.next()? {
        let feed_id: String = row.get(0)?;
        let Some(&feed_index) = feed_indices.get(feed_id.as_str()) else {
            continue;
        };
        let time_zone_name: String = row.get(6)?;
        headers.insert(
            (feed_index, row.get(1)?),
            FeedTripHeader {
                service_id: row.get(2)?,
                trip_short_name: row.get(3)?,
                route_short_name: row.get(4)?,
                route_type: row.get(7)?,
                agency_name: row.get(5)?,
                time_zone: time_zone_name.parse().unwrap_or(Tz::UTC),
                trip_headsign: row.get(8)?,
                route_desc: row.get(9)?,
                first_stop_id: row.get(10)?,
            },
        );
    }
    Ok(headers)
}

/// GTFS route type of a sleeper rail service in the extended route types.
const SLEEPER_RAIL_ROUTE_TYPE: i32 = 105;

/// Returns whether a trip runs on a train with couchettes or sleepers, by its route type or its
/// category. Feeds disagree on both, so either one is enough.
fn offers_berths(route_type: i32, category: &str, night_train_categories: &[String]) -> bool {
    let category = category.to_lowercase();
    route_type == SLEEPER_RAIL_ROUTE_TYPE
        || night_train_categories
            .iter()
            .any(|night_category| night_category.to_lowercase() == category)
}

/// Reads a trip's train category and number with the feed's rule. Returns the number for
/// deduplication and the label for the bundle.
fn train_label(rule: &TrainLabelRule, header: &FeedTripHeader) -> (String, TrainLabelInput) {
    let field_text = |field: TrainLabelField| match field {
        TrainLabelField::TripShortName => header.trip_short_name.as_deref().unwrap_or(""),
        TrainLabelField::TripHeadsign => header.trip_headsign.as_deref().unwrap_or(""),
        TrainLabelField::RouteShortName => header.route_short_name.as_deref().unwrap_or(""),
        TrainLabelField::RouteDesc => header.route_desc.as_deref().unwrap_or(""),
        TrainLabelField::RouteType => route_type_category(header.route_type),
        TrainLabelField::AgencyName => header.agency_name.as_str(),
        TrainLabelField::FirstStopId => header.first_stop_id.as_deref().unwrap_or(""),
    };
    let text = field_text(rule.field).trim();
    let captures = rule.pattern.captures(text);
    let number = captures
        .as_ref()
        .and_then(|captures| captures.name("number"))
        .map_or(text, |number| number.as_str());
    let category = captures
        .as_ref()
        .and_then(|captures| captures.name("category"))
        .map_or_else(
            || {
                let category_text = field_text(rule.category_field).trim();
                // A category pattern picks the category out of a longer field. A field it does
                // not match stays whole, so an unexpected value still shows up in the label.
                rule.category_pattern
                    .as_ref()
                    .and_then(|pattern| pattern.captures(category_text))
                    .and_then(|captures| captures.name("category"))
                    .map_or(category_text, |category| category.as_str().trim())
            },
            |category| category.as_str(),
        );
    let train_number = if captures.is_some() {
        number.trim_start_matches('0').to_owned()
    } else {
        String::new()
    };
    let label = TrainLabelInput {
        category: category.to_owned(),
        number: number.to_owned(),
        agency: header.agency_name.clone(),
        // The caller decides this with the list of night train categories.
        offers_berths: false,
    };
    (train_number, label)
}

/// Returns the usual German product name for a GTFS route type, for feeds whose route short
/// names are line numbers. The extended types follow the European rail categories.
fn route_type_category(route_type: i32) -> &'static str {
    match route_type {
        101 => "ICE",
        102 => "IC",
        103 => "IR",
        105 => "NJ",
        106 => "RE",
        107 => "Tourist",
        109 => "S",
        _ => "Zug",
    }
}

/// Resolves the local times of a trip to UTC for every day it runs, and groups days that give
/// the same UTC times.
///
/// GTFS times count from noon minus 12 hours in local time, which is midnight except on
/// daylight saving days. Each result counts minutes from 00:00 UTC on the UTC date of its first
/// departure, with arrivals rounded up and departures rounded down.
fn resolve_to_utc(
    stops: &[FeedTripStop],
    service_days: ServiceDayBitset,
    time_zone: Tz,
    window: ValidityWindow,
) -> Vec<(ServiceDayBitset, Vec<TripStopInput>)> {
    if stops.len() < 2 {
        return Vec::new();
    }
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or_default();
    let window_start_day = (window.start - epoch).num_days();
    let mut groups: FxHashMap<Vec<(u16, u16)>, ServiceDayBitset> = FxHashMap::default();
    for day in (0..window.days).filter(|&day| service_days.runs_on(day)) {
        let Some(date) = window.start.checked_add_days(Days::new(u64::from(day))) else {
            continue;
        };
        let Some(noon) = date.and_hms_opt(12, 0, 0) else {
            continue;
        };
        let Some(local_noon) = time_zone.from_local_datetime(&noon).earliest() else {
            continue;
        };
        let reference = local_noon.timestamp() - SECONDS_PER_DAY / 2;
        let first_departure = reference + i64::from(stops[0].departure_seconds);
        let utc_date = first_departure.div_euclid(SECONDS_PER_DAY);
        let Ok(bundle_day) = u16::try_from(utc_date - window_start_day) else {
            continue;
        };
        if bundle_day >= window.days {
            continue;
        }
        let utc_midnight = utc_date * SECONDS_PER_DAY;
        let times = stops
            .iter()
            .enumerate()
            .scan(0_i64, |previous_departure, (stop_position, stop)| {
                let departure =
                    (reference + i64::from(stop.departure_seconds) - utc_midnight).div_euclid(60);
                // Nobody arrives at the first stop, and its arrival rounded up could otherwise
                // push the first departure past midnight onto the next UTC date.
                let arrival = if stop_position == 0 {
                    departure
                } else {
                    (reference + i64::from(stop.arrival_seconds) - utc_midnight + 59).div_euclid(60)
                };
                // Rounding can put a departure before its arrival, and feeds occasionally step
                // back in time. Both get clamped so the trip never goes backwards.
                let arrival = arrival.max(*previous_departure);
                let departure = departure.max(arrival);
                *previous_departure = departure;
                Some((u16::try_from(arrival).ok()?, u16::try_from(departure).ok()?))
            })
            .collect::<Vec<_>>();
        if times.len() == stops.len() {
            groups.entry(times).or_default().insert(bundle_day);
        }
    }
    let mut resolved: Vec<_> = groups
        .into_iter()
        .map(|(times, days)| {
            let trip_stops = stops
                .iter()
                .zip(times)
                .map(|(stop, (arrival, departure))| TripStopInput {
                    station: stop.station,
                    arrival,
                    departure,
                    boarding_allowed: stop.boarding_allowed,
                    alighting_allowed: stop.alighting_allowed,
                })
                .collect();
            (days, trip_stops)
        })
        .collect();
    // Hash map order varies between runs, and bundles should not.
    resolved.sort_by_key(|(days, _)| days.0);
    resolved
}

#[cfg(test)]
mod tests {
    use regex::Regex;

    use super::*;

    fn stop(station: u32, arrival_seconds: i32, departure_seconds: i32) -> FeedTripStop {
        FeedTripStop {
            station: BundleStationIndex(station),
            arrival_seconds,
            departure_seconds,
            boarding_allowed: true,
            alighting_allowed: true,
        }
    }

    fn window() -> ValidityWindow {
        ValidityWindow {
            start: NaiveDate::from_ymd_opt(2026, 10, 20).unwrap(),
            days: 14,
        }
    }

    fn days(days: &[u16]) -> ServiceDayBitset {
        let mut bitset = ServiceDayBitset::default();
        for &day in days {
            bitset.insert(day);
        }
        bitset
    }

    #[test]
    fn daylight_saving_change_splits_a_daily_trip() {
        // 25 October 2026 is day 5 of the window, and Vienna leaves summer time that morning.
        let stops = [
            stop(0, 8 * 3600, 8 * 3600),
            stop(1, 10 * 3600 + 30, 10 * 3600 + 30),
        ];
        let resolved = resolve_to_utc(&stops, days(&[4, 6]), Tz::Europe__Vienna, window());
        assert_eq!(resolved.len(), 2);
        let summer = resolved.iter().find(|(days, _)| days.runs_on(4)).unwrap();
        let winter = resolved.iter().find(|(days, _)| days.runs_on(6)).unwrap();
        assert_eq!(summer.1[0].departure, 6 * 60, "08:00 CEST is 06:00 UTC");
        assert_eq!(winter.1[0].departure, 7 * 60, "08:00 CET is 07:00 UTC");
        assert_eq!(
            winter.1[1].arrival,
            9 * 60 + 1,
            "seconds round the arrival up"
        );
    }

    #[test]
    fn early_local_departures_move_to_the_previous_utc_date() {
        // 00:30 in Vienna on day 1 is 22:30 UTC on day 0.
        let stops = [stop(0, 1800, 1800), stop(1, 7200, 7200)];
        let resolved = resolve_to_utc(&stops, days(&[1]), Tz::Europe__Vienna, window());
        let (service_days, trip_stops) = &resolved[0];
        assert!(service_days.runs_on(0) && !service_days.runs_on(1));
        assert_eq!(trip_stops[0].departure, 22 * 60 + 30);
        assert_eq!(trip_stops[1].arrival, 24 * 60);
    }

    #[test]
    fn a_first_stop_just_before_midnight_stays_on_its_utc_date() {
        // 01:59:30 in Vienna summer time is 23:59:30 UTC. Rounding the first arrival up would
        // reach midnight and drag the departure onto the next UTC date.
        let first_seconds = 3600 + 59 * 60 + 30;
        let stops = [
            stop(0, first_seconds, first_seconds),
            stop(1, 3 * 3600, 3 * 3600),
        ];
        let resolved = resolve_to_utc(&stops, days(&[1]), Tz::Europe__Vienna, window());
        let (service_days, trip_stops) = &resolved[0];
        assert!(service_days.runs_on(0));
        assert_eq!(trip_stops[0].departure, 23 * 60 + 59);
        assert_eq!(trip_stops[0].arrival, trip_stops[0].departure);
    }

    #[test]
    fn labels_read_category_and_number() {
        let header = FeedTripHeader {
            service_id: String::new(),
            trip_short_name: Some("000141".into()),
            route_short_name: Some("ICE".into()),
            route_type: 101,
            agency_name: "DB Fernverkehr AG".into(),
            time_zone: Tz::Europe__Berlin,
            trip_headsign: None,
            route_desc: None,
            first_stop_id: None,
        };
        let rule = TrainLabelRule {
            field: TrainLabelField::TripShortName,
            pattern: Regex::new(r"^0*(?<number>\d+)$").unwrap(),
            category_field: TrainLabelField::RouteShortName,
            category_pattern: None,
        };
        let (number, label) = train_label(&rule, &header);
        assert_eq!(number, "141");
        assert_eq!(
            (label.category.as_str(), label.number.as_str()),
            ("ICE", "141")
        );
    }

    #[test]
    fn labels_read_the_category_out_of_the_first_stop() {
        // SNCF puts the train number in the headsign and the product in its stop point IDs.
        let header = FeedTripHeader {
            service_id: String::new(),
            trip_short_name: None,
            route_short_name: Some("K82".into()),
            route_type: 2,
            agency_name: "SNCF VOYAGEURS".into(),
            time_zone: Tz::Europe__Paris,
            trip_headsign: Some("6201".into()),
            route_desc: None,
            first_stop_id: Some("StopPoint:OCETGV INOUI-87686006".into()),
        };
        let rule = TrainLabelRule {
            field: TrainLabelField::TripHeadsign,
            pattern: Regex::new(r"^(?<number>\d+)$").unwrap(),
            category_field: TrainLabelField::FirstStopId,
            category_pattern: Some(Regex::new(r"^StopPoint:OCE(?<category>.+)-\d+$").unwrap()),
        };
        let (number, label) = train_label(&rule, &header);
        assert_eq!(number, "6201");
        assert_eq!(label.category, "TGV INOUI");
    }

    #[test]
    fn berths_follow_the_route_type_or_the_category() {
        let night_categories = ["NJ".to_owned(), "INTERCITÉS de nuit".to_owned()];
        assert!(offers_berths(105, "D", &night_categories));
        assert!(offers_berths(2, "nj", &night_categories));
        assert!(offers_berths(2, "Intercités de nuit", &night_categories));
        assert!(!offers_berths(102, "IC", &night_categories));
    }
}
