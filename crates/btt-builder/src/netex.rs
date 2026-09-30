//! Step 2 for NeTEx feeds: reads the rail part of a feed in the Italian NeTEx profile into the
//! same typed tables the GTFS loader fills, so every later step treats both formats alike.
//!
//! The reader streams the XML once and keeps the elements the profile uses for timetables.
//! A Trenitalia file of 389 MB holds about 21,000 journeys, which fit in memory easily.

use std::io::{BufRead, BufReader};
use std::path::Path;

use chrono::{Datelike, NaiveDate, Weekday};
use quick_xml::events::{BytesStart, Event};
use rustc_hash::FxHashMap;

use crate::BuildError;
use crate::config::{FeedConfig, FeedStationCodeScheme};
use crate::load::ValidityWindow;

/// Errors from reading a NeTEx file.
#[derive(Debug, thiserror::Error)]
pub enum NetexError {
    /// The ZIP holds no XML file.
    #[error("the NeTEx archive {0} holds no XML file")]
    NoXmlFile(String),
    /// The ZIP could not be read.
    #[error("NeTEx archive: {0}")]
    Archive(#[from] zip::result::ZipError),
    /// The XML is not well-formed.
    #[error("NeTEx XML: {0}")]
    Xml(#[from] quick_xml::Error),
    /// An attribute could not be read.
    #[error("NeTEx attribute: {0}")]
    Attribute(#[from] quick_xml::events::attributes::AttrError),
}

/// A stop place with its first coordinates.
#[derive(Debug, Default, Clone, PartialEq)]
struct NetexStopPlace {
    id: String,
    name: String,
    private_code: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
}

/// A line with its product name.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct NetexLine {
    name: String,
    short_name: String,
    transport_mode: String,
}

/// A stop of a journey pattern.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct NetexPatternPoint {
    /// Position in the pattern from the `order` attribute. IDs don't give the order, because
    /// some feeds sort `:10` before `:2`.
    order: u32,
    scheduled_stop_point: String,
    for_boarding: bool,
    for_alighting: bool,
}

/// A journey pattern with its stops by ID.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct NetexPattern {
    line: Option<String>,
    points: FxHashMap<String, NetexPatternPoint>,
}

/// The time a journey passes a stop of its pattern.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct NetexPassingTime {
    point: String,
    arrival: Option<String>,
    arrival_day_offset: u32,
    departure: Option<String>,
    departure_day_offset: u32,
}

/// A service journey, which is one train on its days.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct NetexJourney {
    id: String,
    name: String,
    transport_mode: Option<String>,
    line: Option<String>,
    pattern: Option<String>,
    day_types: Vec<String>,
    passing_times: Vec<NetexPassingTime>,
}

/// The days of an operating period.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct NetexOperatingPeriod {
    from_date: Option<NaiveDate>,
    to_date: Option<NaiveDate>,
    /// One character per day from `from_date`, `1` for a day the period covers.
    valid_day_bits: Option<String>,
}

/// Everything the reader keeps from a NeTEx file.
#[derive(Debug, Default)]
struct NetexDocument {
    time_zone: Option<String>,
    operators: Vec<(String, String)>,
    stop_places: Vec<NetexStopPlace>,
    /// Stop place of each quay.
    quay_stop_places: FxHashMap<String, String>,
    /// Stop place or quay each scheduled stop point is assigned to.
    stop_assignments: FxHashMap<String, String>,
    lines: FxHashMap<String, NetexLine>,
    patterns: FxHashMap<String, NetexPattern>,
    journeys: Vec<NetexJourney>,
    /// Days of the week each day type applies to. Empty means every day.
    day_types: FxHashMap<String, Vec<Weekday>>,
    operating_periods: FxHashMap<String, NetexOperatingPeriod>,
    /// Pairs of day type and operating period.
    day_type_assignments: Vec<(String, String)>,
}

/// The element the reader is filling in, besides the stack of element names.
#[derive(Debug, Default)]
struct ParserState {
    stack: Vec<String>,
    text: String,
    stop_place: Option<NetexStopPlace>,
    quay: Option<String>,
    assignment: (Option<String>, Option<String>),
    line: Option<(String, NetexLine)>,
    pattern: Option<(String, NetexPattern)>,
    point: Option<(String, NetexPatternPoint)>,
    journey: Option<NetexJourney>,
    passing_time: Option<NetexPassingTime>,
    day_type: Option<(String, Vec<Weekday>)>,
    operating_period: Option<(String, NetexOperatingPeriod)>,
    day_type_assignment: (Option<String>, Option<String>),
    operator: Option<(String, String)>,
}

/// Reads the first XML file of a NeTEx ZIP.
fn read_netex_archive(zip_path: &Path) -> Result<NetexDocument, NetexError> {
    let mut archive = zip::ZipArchive::new(
        std::fs::File::open(zip_path)
            .map_err(|error| NetexError::Archive(zip::result::ZipError::Io(error)))?,
    )?;
    let xml_index = (0..archive.len())
        .find(|&index| {
            archive
                .name_for_index(index)
                .is_some_and(|name| name.to_ascii_lowercase().ends_with(".xml"))
        })
        .ok_or_else(|| NetexError::NoXmlFile(zip_path.display().to_string()))?;
    let entry = archive.by_index(xml_index)?;
    parse_netex(BufReader::with_capacity(1 << 20, entry))
}

/// Streams a NeTEx document and keeps the elements the timetable needs.
fn parse_netex(input: impl BufRead) -> Result<NetexDocument, NetexError> {
    let mut reader = quick_xml::Reader::from_reader(input);
    // Text stays untrimmed, because an entity splits it into pieces whose spaces count. The
    // whole text of an element is trimmed when it closes.
    reader.config_mut().trim_text(false);
    let mut document = NetexDocument::default();
    let mut state = ParserState::default();
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) => start_element(&element, &mut state)?,
            Event::Empty(element) => {
                start_element(&element, &mut state)?;
                end_element(&mut state, &mut document);
            }
            Event::End(_) => end_element(&mut state, &mut document),
            Event::Text(text) => state.text.push_str(&text),
            Event::CData(text) => state.text.push_str(&text),
            Event::GeneralRef(reference) => state.text.push_str(&resolve_reference(&reference)),
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(document)
}

/// Returns the character an entity or character reference such as `amp` or `#39` stands for.
fn resolve_reference(reference: &str) -> String {
    let character = match reference {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => reference.strip_prefix('#').and_then(|code| {
            let value = match code.strip_prefix('x') {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => code.parse().ok()?,
            };
            char::from_u32(value)
        }),
    };
    character.map_or_else(String::new, String::from)
}

/// Opens the objects an element starts and reads their attributes.
// One arm per element keeps the whole mapping of the profile in one place.
#[allow(clippy::too_many_lines)]
fn start_element(element: &BytesStart<'_>, state: &mut ParserState) -> Result<(), NetexError> {
    let name = element.local_name().as_ref().to_owned();
    let attribute = |key: &str| -> Result<Option<String>, NetexError> {
        Ok(element
            .try_get_attribute(key)?
            .map(|value| {
                value
                    .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .map(std::borrow::Cow::into_owned)
            })
            .transpose()?)
    };
    let parent = state.stack.last().map(String::as_str);
    match name.as_str() {
        "StopPlace" => {
            state.stop_place = Some(NetexStopPlace {
                id: attribute("id")?.unwrap_or_default(),
                ..NetexStopPlace::default()
            });
        }
        "Quay" => state.quay = attribute("id")?,
        "PassengerStopAssignment" => state.assignment = (None, None),
        "ScheduledStopPointRef" => {
            let reference = attribute("ref")?;
            if parent == Some("PassengerStopAssignment") {
                state.assignment.0 = reference;
            } else if let Some((_, point)) = &mut state.point {
                point.scheduled_stop_point = reference.unwrap_or_default();
            }
        }
        "StopPlaceRef" if parent == Some("PassengerStopAssignment") => {
            // A quay is more precise, so a stop place only fills an empty assignment.
            if state.assignment.1.is_none() {
                state.assignment.1 = attribute("ref")?;
            }
        }
        "QuayRef" if parent == Some("PassengerStopAssignment") => {
            state.assignment.1 = attribute("ref")?;
        }
        "Line" => {
            state.line = Some((attribute("id")?.unwrap_or_default(), NetexLine::default()));
        }
        "ServiceJourneyPattern" | "JourneyPattern" => {
            state.pattern = Some((
                attribute("id")?.unwrap_or_default(),
                NetexPattern::default(),
            ));
        }
        "LineRef" => {
            let reference = attribute("ref")?;
            if let Some(journey) = &mut state.journey {
                journey.line = reference;
            } else if let Some((_, pattern)) = &mut state.pattern {
                pattern.line = reference;
            }
        }
        "StopPointInJourneyPattern" => {
            state.point = Some((
                attribute("id")?.unwrap_or_default(),
                NetexPatternPoint {
                    order: attribute("order")?
                        .and_then(|order| order.parse().ok())
                        .unwrap_or(0),
                    // NeTEx allows boarding and alighting unless a stop says otherwise.
                    for_boarding: true,
                    for_alighting: true,
                    ..NetexPatternPoint::default()
                },
            ));
        }
        "ServiceJourney" => {
            state.journey = Some(NetexJourney {
                id: attribute("id")?.unwrap_or_default(),
                ..NetexJourney::default()
            });
        }
        "DayTypeRef" => {
            let reference = attribute("ref")?;
            if let Some(journey) = &mut state.journey {
                journey.day_types.extend(reference);
            } else if parent == Some("DayTypeAssignment") {
                state.day_type_assignment.0 = reference;
            }
        }
        "ServiceJourneyPatternRef" | "JourneyPatternRef" => {
            if let Some(journey) = &mut state.journey {
                journey.pattern = attribute("ref")?;
            }
        }
        "TimetabledPassingTime" => state.passing_time = Some(NetexPassingTime::default()),
        "StopPointInJourneyPatternRef" => {
            if let Some(passing_time) = &mut state.passing_time {
                passing_time.point = attribute("ref")?.unwrap_or_default();
            }
        }
        "DayType" => state.day_type = Some((attribute("id")?.unwrap_or_default(), Vec::new())),
        "UicOperatingPeriod" | "OperatingPeriod" => {
            state.operating_period = Some((
                attribute("id")?.unwrap_or_default(),
                NetexOperatingPeriod::default(),
            ));
        }
        "DayTypeAssignment" => state.day_type_assignment = (None, None),
        "OperatingPeriodRef" if parent == Some("DayTypeAssignment") => {
            state.day_type_assignment.1 = attribute("ref")?;
        }
        "Operator" => state.operator = Some((attribute("id")?.unwrap_or_default(), String::new())),
        _ => {}
    }
    state.stack.push(name);
    state.text.clear();
    Ok(())
}

/// Stores the text of a closing element in the open object, and stores a closing object in
/// the document.
// One arm per element keeps the whole mapping of the profile in one place.
#[allow(clippy::too_many_lines)]
fn end_element(state: &mut ParserState, document: &mut NetexDocument) {
    let Some(name) = state.stack.pop() else {
        return;
    };
    let text = std::mem::take(&mut state.text);
    let text = text.trim();
    let parent = state.stack.last().map(String::as_str);
    let grandparents = &state.stack[..state.stack.len().saturating_sub(1)];
    match (name.as_str(), parent) {
        ("Name", Some("StopPlace")) => {
            if let Some(stop_place) = &mut state.stop_place {
                text.clone_into(&mut stop_place.name);
            }
        }
        ("PrivateCode", Some("StopPlace")) => {
            if let Some(stop_place) = &mut state.stop_place {
                stop_place.private_code = Some(text.to_owned());
            }
        }
        ("Latitude" | "Longitude", Some("Location")) => {
            // Only the stop place's own centroid counts, not its quays'.
            let is_stop_place_centroid = matches!(
                grandparents,
                [.., stop_place, centroid] if stop_place == "StopPlace" && centroid == "Centroid"
            );
            if let (Some(stop_place), true) = (&mut state.stop_place, is_stop_place_centroid) {
                let value = text.parse().ok();
                if name == "Latitude" {
                    stop_place.latitude = value;
                } else {
                    stop_place.longitude = value;
                }
            }
        }
        ("Quay", _) => {
            if let (Some(quay), Some(stop_place)) = (state.quay.take(), &state.stop_place) {
                document
                    .quay_stop_places
                    .insert(quay, stop_place.id.clone());
            }
        }
        ("StopPlace", _) => document.stop_places.extend(state.stop_place.take()),
        ("PassengerStopAssignment", _) => {
            if let (Some(point), Some(stop)) = std::mem::take(&mut state.assignment) {
                document.stop_assignments.insert(point, stop);
            }
        }
        ("Name", Some("Line")) => {
            if let Some((_, line)) = &mut state.line {
                text.clone_into(&mut line.name);
            }
        }
        ("ShortName", Some("Line")) => {
            if let Some((_, line)) = &mut state.line {
                text.clone_into(&mut line.short_name);
            }
        }
        ("TransportMode", Some("Line")) => {
            if let Some((_, line)) = &mut state.line {
                text.clone_into(&mut line.transport_mode);
            }
        }
        ("Line", _) => {
            if let Some((id, line)) = state.line.take() {
                document.lines.insert(id, line);
            }
        }
        ("ForBoarding", Some("StopPointInJourneyPattern")) => {
            if let Some((_, point)) = &mut state.point {
                point.for_boarding = text != "false";
            }
        }
        ("ForAlighting", Some("StopPointInJourneyPattern")) => {
            if let Some((_, point)) = &mut state.point {
                point.for_alighting = text != "false";
            }
        }
        ("StopPointInJourneyPattern", _) => {
            if let (Some((id, point)), Some((_, pattern))) =
                (state.point.take(), &mut state.pattern)
            {
                pattern.points.insert(id, point);
            }
        }
        ("ServiceJourneyPattern" | "JourneyPattern", _) => {
            if let Some((id, pattern)) = state.pattern.take() {
                document.patterns.insert(id, pattern);
            }
        }
        ("Name", Some("ServiceJourney")) => {
            if let Some(journey) = &mut state.journey {
                text.clone_into(&mut journey.name);
            }
        }
        ("TransportMode", Some("ServiceJourney")) => {
            if let Some(journey) = &mut state.journey {
                journey.transport_mode = Some(text.to_owned());
            }
        }
        ("ArrivalTime", Some("TimetabledPassingTime")) => {
            if let Some(passing_time) = &mut state.passing_time {
                passing_time.arrival = Some(text.to_owned());
            }
        }
        ("DepartureTime", Some("TimetabledPassingTime")) => {
            if let Some(passing_time) = &mut state.passing_time {
                passing_time.departure = Some(text.to_owned());
            }
        }
        ("ArrivalDayOffset", Some("TimetabledPassingTime")) => {
            if let Some(passing_time) = &mut state.passing_time {
                passing_time.arrival_day_offset = text.parse().unwrap_or(0);
            }
        }
        ("DepartureDayOffset", Some("TimetabledPassingTime")) => {
            if let Some(passing_time) = &mut state.passing_time {
                passing_time.departure_day_offset = text.parse().unwrap_or(0);
            }
        }
        ("TimetabledPassingTime", _) => {
            if let (Some(passing_time), Some(journey)) =
                (state.passing_time.take(), &mut state.journey)
            {
                journey.passing_times.push(passing_time);
            }
        }
        ("ServiceJourney", _) => document.journeys.extend(state.journey.take()),
        ("DaysOfWeek", _) => {
            if let Some((_, days)) = &mut state.day_type {
                days.extend(text.split_whitespace().filter_map(weekday_from_name));
            }
        }
        ("DayType", _) => {
            if let Some((id, days)) = state.day_type.take() {
                document.day_types.insert(id, days);
            }
        }
        ("FromDate" | "ToDate", Some("UicOperatingPeriod" | "OperatingPeriod")) => {
            if let Some((_, period)) = &mut state.operating_period {
                let date = text
                    .get(..10)
                    .and_then(|day| NaiveDate::parse_from_str(day, "%Y-%m-%d").ok());
                if name == "FromDate" {
                    period.from_date = date;
                } else {
                    period.to_date = date;
                }
            }
        }
        ("ValidDayBits", _) => {
            if let Some((_, period)) = &mut state.operating_period {
                period.valid_day_bits = Some(text.to_owned());
            }
        }
        ("UicOperatingPeriod" | "OperatingPeriod", _) => {
            if let Some((id, period)) = state.operating_period.take() {
                document.operating_periods.insert(id, period);
            }
        }
        ("DayTypeAssignment", _) => {
            if let (Some(day_type), Some(period)) = std::mem::take(&mut state.day_type_assignment) {
                document.day_type_assignments.push((day_type, period));
            }
        }
        ("Name", Some("Operator")) => {
            if let Some((_, operator_name)) = &mut state.operator {
                text.clone_into(operator_name);
            }
        }
        ("Operator", _) => document.operators.extend(state.operator.take()),
        ("TimeZone", _) if document.time_zone.is_none() => {
            document.time_zone = Some(text.to_owned());
        }
        _ => {}
    }
}

/// Returns the weekday an English day name such as `Monday` stands for.
fn weekday_from_name(name: &str) -> Option<Weekday> {
    Some(match name {
        "Monday" => Weekday::Mon,
        "Tuesday" => Weekday::Tue,
        "Wednesday" => Weekday::Wed,
        "Thursday" => Weekday::Thu,
        "Friday" => Weekday::Fri,
        "Saturday" => Weekday::Sat,
        "Sunday" => Weekday::Sun,
        _ => return None,
    })
}

/// A rail line as a GTFS route.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RouteRow {
    route_id: String,
    short_name: String,
    description: String,
    route_type: i32,
}

/// A journey as a GTFS trip. Its ID is also its service ID.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TripRow {
    trip_id: String,
    route_id: String,
    train_number: String,
}

/// A stop of a journey, with times in seconds since the service date.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StopTimeRow {
    trip_id: String,
    stop_sequence: i32,
    stop_id: String,
    arrival_seconds: i32,
    departure_seconds: i32,
    boarding_allowed: bool,
    alighting_allowed: bool,
}

/// A stop place as a GTFS stop, which is also its own station.
#[derive(Debug, Clone, PartialEq)]
struct StopRow {
    stop_id: String,
    name: String,
    latitude: Option<f64>,
    longitude: Option<f64>,
    station_code: Option<String>,
}

/// Rows for the typed tables, one vector per table.
#[derive(Debug, Default, PartialEq)]
struct NetexRows {
    /// Agency ID and name.
    agencies: Vec<(String, String)>,
    routes: Vec<RouteRow>,
    trips: Vec<TripRow>,
    stop_times: Vec<StopTimeRow>,
    stops: Vec<StopRow>,
    /// Service ID and day number in the validity window.
    service_days: Vec<(String, i16)>,
}

/// Returns the GTFS route type of a Trenitalia product, so the rest of the builder treats it
/// like a GTFS feed. Night trains get 105, which flags their berths.
fn route_type_of_product(short_name: &str) -> i32 {
    match short_name {
        "FR" | "FA" => 101,
        "FB" | "IC" | "EC" => 102,
        "ICN" | "EN" | "ECN" => 105,
        "RV" | "REG" => 106,
        "MET" | "SFM" => 109,
        _ => 2,
    }
}

/// Returns the station code of a stop place, following the feed's station code scheme.
fn station_code(stop_place: &NetexStopPlace, scheme: FeedStationCodeScheme) -> Option<String> {
    let code = stop_place.private_code.as_deref()?;
    match scheme {
        // 830001700 is country 83 and station 01700, which is UIC 8301700.
        FeedStationCodeScheme::UicInPrivateCode => {
            let is_code = code.len() == 9 && code.bytes().all(|byte| byte.is_ascii_digit());
            is_code.then(|| format!("{}{}", &code[..2], &code[4..]))
        }
        _ => None,
    }
}

/// Turns a time such as `15:25:00` and a day offset into seconds since the service date.
fn netex_seconds(time: &str, day_offset: u32) -> Option<i32> {
    let mut parts = time.split(':').map(|part| part.parse::<i32>().ok());
    let hours = parts.next()??;
    let minutes = parts.next()??;
    let seconds = parts.next().flatten().unwrap_or(0);
    Some(i32::try_from(day_offset).ok()? * 86_400 + hours * 3600 + minutes * 60 + seconds)
}

/// Longest operating period without valid day bits that the reader expands, in days. It
/// guards against a missing last date.
const LONGEST_OPERATING_PERIOD_DAYS: usize = 800;

/// Returns the days of a day type within the validity window, as day numbers.
///
/// An operating period with valid day bits covers the days whose bit is `1`, counted from its
/// first date. One without bits covers every day from its first to its last date on the day
/// type's days of the week.
fn service_days_in_window(
    document: &NetexDocument,
    day_type: &str,
    window: ValidityWindow,
) -> Vec<i16> {
    let weekdays = document.day_types.get(day_type);
    let mut days = Vec::new();
    for (_, period_id) in document
        .day_type_assignments
        .iter()
        .filter(|(assigned, _)| assigned == day_type)
    {
        let Some(period) = document.operating_periods.get(period_id) else {
            continue;
        };
        let Some(from_date) = period.from_date else {
            continue;
        };
        let dates: Vec<NaiveDate> = match &period.valid_day_bits {
            Some(bits) => bits
                .bytes()
                .enumerate()
                .filter(|(_, bit)| *bit == b'1')
                .filter_map(|(offset, _)| {
                    from_date.checked_add_days(chrono::Days::new(u64::try_from(offset).ok()?))
                })
                .collect(),
            None => from_date
                .iter_days()
                .take_while(|date| period.to_date.is_none_or(|to_date| *date <= to_date))
                .take(LONGEST_OPERATING_PERIOD_DAYS)
                .filter(|date| {
                    weekdays.is_none_or(|weekdays| {
                        weekdays.is_empty() || weekdays.contains(&date.weekday())
                    })
                })
                .collect(),
        };
        days.extend(dates.into_iter().filter_map(|date| {
            let day = (date - window.start).num_days();
            let in_window = (0..i64::from(window.days)).contains(&day);
            in_window.then(|| i16::try_from(day).ok()).flatten()
        }));
    }
    days
}

/// Returns the stops of a journey in pattern order. Passing times whose stop can't be
/// resolved are left out.
fn journey_stop_times(
    document: &NetexDocument,
    journey: &NetexJourney,
    pattern: &NetexPattern,
) -> Vec<StopTimeRow> {
    let mut stops: Vec<StopTimeRow> = journey
        .passing_times
        .iter()
        .filter_map(|passing_time| {
            let point = pattern.points.get(&passing_time.point)?;
            let assigned = document.stop_assignments.get(&point.scheduled_stop_point)?;
            let stop_place = document.quay_stop_places.get(assigned).unwrap_or(assigned);
            let arrival = passing_time
                .arrival
                .as_deref()
                .and_then(|time| netex_seconds(time, passing_time.arrival_day_offset));
            let departure = passing_time
                .departure
                .as_deref()
                .and_then(|time| netex_seconds(time, passing_time.departure_day_offset));
            let arrival_seconds = arrival.or(departure)?;
            Some(StopTimeRow {
                trip_id: journey.id.clone(),
                stop_sequence: i32::try_from(point.order).unwrap_or(i32::MAX),
                stop_id: stop_place.clone(),
                arrival_seconds,
                departure_seconds: departure.unwrap_or(arrival_seconds),
                boarding_allowed: point.for_boarding,
                alighting_allowed: point.for_alighting,
            })
        })
        .collect();
    stops.sort_by_key(|stop| stop.stop_sequence);
    stops
}

/// Turns a document into table rows: rail lines, their journeys with stops in pattern order,
/// the stop places they use and their days within the validity window.
fn netex_rows(
    document: &NetexDocument,
    window: ValidityWindow,
    station_code_scheme: FeedStationCodeScheme,
) -> NetexRows {
    let mut rows = NetexRows {
        agencies: document.operators.clone(),
        ..NetexRows::default()
    };
    let is_rail = |line: &NetexLine| line.transport_mode == "rail";
    rows.routes = document
        .lines
        .iter()
        .filter(|(_, line)| is_rail(line))
        .map(|(id, line)| RouteRow {
            route_id: id.clone(),
            short_name: line.short_name.clone(),
            description: line.name.clone(),
            route_type: route_type_of_product(&line.short_name),
        })
        .collect();
    rows.routes.sort();

    let mut used_stops = rustc_hash::FxHashSet::default();
    for journey in &document.journeys {
        let Some(pattern) = journey
            .pattern
            .as_ref()
            .and_then(|pattern| document.patterns.get(pattern))
        else {
            continue;
        };
        let Some(line_id) = journey.line.as_ref().or(pattern.line.as_ref()) else {
            continue;
        };
        let is_rail_journey = document.lines.get(line_id).is_some_and(is_rail)
            && journey
                .transport_mode
                .as_deref()
                .is_none_or(|mode| mode == "rail");
        if !is_rail_journey {
            continue;
        }
        let mut days: Vec<i16> = journey
            .day_types
            .iter()
            .flat_map(|day_type| service_days_in_window(document, day_type, window))
            .collect();
        days.sort_unstable();
        days.dedup();
        let stop_times = journey_stop_times(document, journey, pattern);
        if days.is_empty() || stop_times.len() < 2 {
            continue;
        }
        rows.trips.push(TripRow {
            trip_id: journey.id.clone(),
            route_id: line_id.clone(),
            train_number: journey.name.clone(),
        });
        rows.service_days
            .extend(days.into_iter().map(|day| (journey.id.clone(), day)));
        used_stops.extend(stop_times.iter().map(|stop| stop.stop_id.clone()));
        rows.stop_times.extend(stop_times);
    }

    rows.stops = document
        .stop_places
        .iter()
        .filter(|stop_place| used_stops.contains(&stop_place.id))
        .map(|stop_place| StopRow {
            stop_id: stop_place.id.clone(),
            name: stop_place.name.clone(),
            latitude: stop_place.latitude,
            longitude: stop_place.longitude,
            station_code: station_code(stop_place, station_code_scheme),
        })
        .collect();
    rows
}

/// Loads the rail part of a NeTEx feed into the typed tables. Returns the number of trips.
///
/// # Errors
///
/// Returns an error when the archive can't be read, the XML is malformed or an insert fails.
pub fn load_netex_feed(
    database: &crate::database::WorkDatabase,
    zip_path: &Path,
    feed: &FeedConfig,
    window: ValidityWindow,
) -> Result<usize, BuildError> {
    let document = read_netex_archive(zip_path)?;
    let rows = netex_rows(&document, window, feed.station_code);
    let feed_id = feed.mirror_name.as_str();
    let time_zone = document.time_zone.as_deref();
    let connection = &database.connection;

    let mut appender = connection.appender("feed_agencies")?;
    for (id, name) in &rows.agencies {
        appender.append_row(duckdb::params![feed_id, id, name, time_zone])?;
    }
    drop(appender);
    let mut appender = connection.appender("feed_routes")?;
    for route in &rows.routes {
        appender.append_row(duckdb::params![
            feed_id,
            route.route_id,
            None::<String>,
            route.short_name,
            route.description,
            route.route_type
        ])?;
    }
    drop(appender);
    let mut appender = connection.appender("feed_trips")?;
    for trip in &rows.trips {
        appender.append_row(duckdb::params![
            feed_id,
            trip.trip_id,
            trip.route_id,
            trip.trip_id,
            trip.train_number,
            None::<String>
        ])?;
    }
    drop(appender);
    let mut appender = connection.appender("feed_stop_times")?;
    for stop_time in &rows.stop_times {
        appender.append_row(duckdb::params![
            feed_id,
            stop_time.trip_id,
            stop_time.stop_sequence,
            stop_time.stop_id,
            stop_time.arrival_seconds,
            stop_time.departure_seconds,
            stop_time.boarding_allowed,
            stop_time.alighting_allowed
        ])?;
    }
    drop(appender);
    let mut appender = connection.appender("feed_stops")?;
    for stop in &rows.stops {
        // The stop places are the stations, so each is its own station key.
        appender.append_row(duckdb::params![
            feed_id,
            stop.stop_id,
            stop.name,
            stop.latitude,
            stop.longitude,
            stop.stop_id,
            stop.station_code
        ])?;
    }
    drop(appender);
    let mut appender = connection.appender("feed_service_days")?;
    for (service_id, day) in &rows.service_days {
        appender.append_row(duckdb::params![feed_id, service_id, day])?;
    }
    drop(appender);
    let mut appender = connection.appender("feed_config")?;
    appender.append_row(duckdb::params![
        feed_id,
        crate::load::station_code_scheme_name(feed.station_code),
        time_zone,
        feed.station_key_namespace.as_deref()
    ])?;
    drop(appender);
    Ok(rows.trips.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small file in the Italian profile: two stations, a rail line and a bus line, and one
    /// train each. The train's stop IDs sort the wrong way as text, and the passing times come
    /// in reverse, so only the `order` attribute gives the right order.
    const FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<PublicationDelivery xmlns="http://www.netex.org.uk/netex">
 <dataObjects><CompositeFrame id="c"><FrameDefaults><DefaultLocale><TimeZone>Europe/Rome</TimeZone></DefaultLocale></FrameDefaults>
 <frames>
  <ResourceFrame id="r"><organisations><Operator id="op"><Name>TRENITALIA</Name></Operator></organisations></ResourceFrame>
  <SiteFrame id="s"><stopPlaces>
   <StopPlace id="sp:milano"><Name>MILANO CENTRALE</Name><PrivateCode>830001700</PrivateCode>
    <Centroid><Location><Longitude>9.2048</Longitude><Latitude>45.4871</Latitude></Location></Centroid>
    <quays><Quay id="q:milano"><Name>MILANO CENTRALE</Name><Centroid><Location><Longitude>1</Longitude><Latitude>1</Latitude></Location></Centroid></Quay></quays>
   </StopPlace>
   <StopPlace id="sp:roma"><Name>ROMA TERMINI &amp; CO</Name><PrivateCode>830008409</PrivateCode>
    <Centroid><Location><Longitude>12.5007</Longitude><Latitude>41.901</Latitude></Location></Centroid>
   </StopPlace>
  </stopPlaces></SiteFrame>
  <ServiceFrame id="f">
   <lines>
    <Line id="line:fr"><Name>Frecciarossa</Name><ShortName>FR</ShortName><TransportMode>rail</TransportMode></Line>
    <Line id="line:bus"><Name>Autobus</Name><ShortName>BUS</ShortName><TransportMode>bus</TransportMode></Line>
   </lines>
   <stopAssignments>
    <PassengerStopAssignment id="a1"><ScheduledStopPointRef ref="ssp:milano"/><StopPlaceRef ref="sp:milano"/><QuayRef ref="q:milano"/></PassengerStopAssignment>
    <PassengerStopAssignment id="a2"><ScheduledStopPointRef ref="ssp:roma"/><StopPlaceRef ref="sp:roma"/></PassengerStopAssignment>
   </stopAssignments>
   <journeyPatterns>
    <ServiceJourneyPattern id="p:fr"><Name>Frecciarossa</Name><RouteView><LineRef ref="line:fr"/></RouteView><pointsInSequence>
     <StopPointInJourneyPattern id="p:fr:10" order="1"><ScheduledStopPointRef ref="ssp:milano"/><ForAlighting>false</ForAlighting><ForBoarding>true</ForBoarding></StopPointInJourneyPattern>
     <StopPointInJourneyPattern id="p:fr:2" order="2"><ScheduledStopPointRef ref="ssp:roma"/><ForAlighting>true</ForAlighting><ForBoarding>false</ForBoarding></StopPointInJourneyPattern>
    </pointsInSequence></ServiceJourneyPattern>
    <ServiceJourneyPattern id="p:bus"><RouteView><LineRef ref="line:bus"/></RouteView><pointsInSequence>
     <StopPointInJourneyPattern id="p:bus:1" order="1"><ScheduledStopPointRef ref="ssp:milano"/></StopPointInJourneyPattern>
     <StopPointInJourneyPattern id="p:bus:2" order="2"><ScheduledStopPointRef ref="ssp:roma"/></StopPointInJourneyPattern>
    </pointsInSequence></ServiceJourneyPattern>
   </journeyPatterns>
  </ServiceFrame>
  <ServiceCalendarFrame id="cal">
   <dayTypes><DayType id="dt:1"><properties><PropertyOfDay><DaysOfWeek>Monday</DaysOfWeek></PropertyOfDay></properties></DayType></dayTypes>
   <operatingPeriods><UicOperatingPeriod id="op:1"><FromDate>2026-09-30T00:00:00</FromDate><ToDate>2026-10-03T23:59:59</ToDate><ValidDayBits>1001</ValidDayBits></UicOperatingPeriod></operatingPeriods>
   <dayTypeAssignments><DayTypeAssignment id="dta"><OperatingPeriodRef ref="op:1"/><DayTypeRef ref="dt:1"/></DayTypeAssignment></dayTypeAssignments>
  </ServiceCalendarFrame>
  <TimetableFrame id="t"><vehicleJourneys>
   <ServiceJourney id="sj:9541"><Name>9541</Name><TransportMode>rail</TransportMode><DepartureTime>23:30:00</DepartureTime>
    <dayTypes><DayTypeRef ref="dt:1"/></dayTypes><ServiceJourneyPatternRef ref="p:fr"/>
    <passingTimes>
     <TimetabledPassingTime id="pt2"><StopPointInJourneyPatternRef ref="p:fr:2"/><ArrivalTime>02:40:00</ArrivalTime><ArrivalDayOffset>1</ArrivalDayOffset></TimetabledPassingTime>
     <TimetabledPassingTime id="pt1"><StopPointInJourneyPatternRef ref="p:fr:10"/><DepartureTime>23:30:00</DepartureTime><DepartureDayOffset>0</DepartureDayOffset></TimetabledPassingTime>
    </passingTimes>
   </ServiceJourney>
   <ServiceJourney id="sj:bus"><Name>1</Name><TransportMode>bus</TransportMode>
    <dayTypes><DayTypeRef ref="dt:1"/></dayTypes><ServiceJourneyPatternRef ref="p:bus"/>
    <passingTimes>
     <TimetabledPassingTime id="b1"><StopPointInJourneyPatternRef ref="p:bus:1"/><DepartureTime>08:00:00</DepartureTime></TimetabledPassingTime>
     <TimetabledPassingTime id="b2"><StopPointInJourneyPatternRef ref="p:bus:2"/><ArrivalTime>12:00:00</ArrivalTime></TimetabledPassingTime>
    </passingTimes>
   </ServiceJourney>
  </vehicleJourneys></TimetableFrame>
 </frames></CompositeFrame></dataObjects>
</PublicationDelivery>"#;

    #[test]
    fn a_netex_train_becomes_table_rows() {
        let document = parse_netex(FIXTURE.as_bytes()).unwrap();
        let window = ValidityWindow {
            start: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            days: 30,
        };
        let rows = netex_rows(&document, window, FeedStationCodeScheme::UicInPrivateCode);

        assert_eq!(document.time_zone.as_deref(), Some("Europe/Rome"));
        let routes: Vec<_> = rows
            .routes
            .iter()
            .map(|route| {
                (
                    route.route_id.as_str(),
                    route.short_name.as_str(),
                    route.route_type,
                )
            })
            .collect();
        assert_eq!(routes, [("line:fr", "FR", 101)], "the bus line is left out");
        assert_eq!(rows.trips.len(), 1);
        assert_eq!(rows.trips[0].train_number, "9541");
        // The bits cover 30 September and 3 October. The first lies before the window.
        assert_eq!(rows.service_days, [("sj:9541".to_owned(), 2)]);
        let stops: Vec<_> = rows
            .stop_times
            .iter()
            .map(|stop| {
                (
                    stop.stop_id.as_str(),
                    stop.arrival_seconds,
                    stop.boarding_allowed,
                    stop.alighting_allowed,
                )
            })
            .collect();
        // The quay of Milano resolves to its stop place, and Roma is reached after midnight.
        assert_eq!(
            stops,
            [
                ("sp:milano", 84_600, true, false),
                ("sp:roma", 96_000, false, true)
            ]
        );
        assert_eq!(rows.stops.len(), 2);
        assert_eq!(rows.stops[0].station_code.as_deref(), Some("8301700"));
        assert_eq!(
            rows.stops[0].latitude,
            Some(45.4871),
            "the stop place's own centroid counts, not its quay's"
        );
        assert_eq!(rows.stops[1].name, "ROMA TERMINI & CO");
    }
}
