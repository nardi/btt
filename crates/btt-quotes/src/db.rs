//! Prices from the service of the DB app.
//!
//! The DB booking site refuses scripted clients, but the service behind the DB app answers
//! plain requests. One request takes the EVA numbers of two stations, a departure time, a class
//! and the travellers, and returns the connections with the lowest price of each.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{QuoteError, QuoteStore, RequestLimiter};

/// What the client says about itself, in the generic form the booking sites accept.
const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (compatible; BigTrainTrip/",
    env!("CARGO_PKG_VERSION"),
    ")"
);

/// The address of the connection search of the DB app.
const SEARCH_URL: &str = "https://app.services-bahn.de/mob/angebote/fahrplan";

/// The media type of the search, which names its version.
const MEDIA_TYPE: &str = "application/x.db.vendo.mob.verbindungssuche.v9+json";

/// The least time between two requests.
const MINIMUM_INTERVAL: Duration = Duration::from_secs(2);

/// How long requests stop after one fails.
const FAILURE_PAUSE: Duration = Duration::from_mins(15);

/// How long the connections of a search are kept. Prices change with demand, so the time is
/// short.
const QUOTE_LIFETIME: Duration = Duration::from_mins(30);

/// A search for connections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Search {
    /// EVA number of the departure station, such as `8011160` for Berlin Hbf.
    pub origin_eva: String,
    /// EVA number of the arrival station.
    pub destination_eva: String,
    /// The moment to leave at or after.
    pub departure: DateTime<FixedOffset>,
    /// `KLASSE_1` or `KLASSE_2`.
    pub class: String,
    /// The traveller type of the DB site, such as `ERWACHSENER`.
    pub traveller_type: String,
}

impl Search {
    /// Builds the request body the DB app sends.
    pub fn request(&self) -> Value {
        json!({
            "autonomeReservierung": false,
            "einstiegsTypList": ["STANDARD"],
            "fahrverguenstigungen": {
                "deutschlandTicketVorhanden": false,
                "nurDeutschlandTicketVerbindungen": false,
            },
            "klasse": self.class,
            "reiseHin": {
                "wunsch": {
                    "abgangsLocationId": format!("A=1@L={}@", self.origin_eva),
                    "zielLocationId": format!("A=1@L={}@", self.destination_eva),
                    "verkehrsmittel": ["ALL"],
                    "alternativeHalteBerechnung": true,
                    "zeitWunsch": {
                        "reiseDatum": self.departure.to_rfc3339(),
                        "zeitPunktArt": "ABFAHRT",
                    },
                },
            },
            "reisendenProfil": {
                "reisende": [{
                    "ermaessigungen": ["KEINE_ERMAESSIGUNG KLASSENLOS"],
                    "reisendenTyp": self.traveller_type,
                }],
            },
            "reservierungsKontingenteVorhanden": false,
        })
    }

    /// Returns the key under which the connections of the search are kept.
    fn cache_key(&self) -> String {
        format!(
            "db:{}:{}:{}:{}:{}",
            self.origin_eva, self.destination_eva, self.departure, self.class, self.traveller_type
        )
    }
}

/// Returns the type of traveller of the DB site for its numeric id in a booking link, as in
/// `r=13:16:KLASSENLOS:1`, or `None` for an id the app does not write.
pub fn traveller_type(id: &str) -> Option<&'static str> {
    Some(match id {
        "13" => "ERWACHSENER",
        "9" => "JUGENDLICHER",
        "12" => "SENIOR",
        "11" => "FAMILIENKIND",
        "8" => "KLEINKIND",
        _ => return None,
    })
}

/// A connection the service found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    /// Departure of the first train.
    pub departure: DateTime<FixedOffset>,
    /// The trains, such as `ICE 604`, without the walks between them.
    pub trains: Vec<String>,
    /// The lowest price in euro, or `None` when the site sells no ticket for it.
    pub price: Option<f64>,
    /// Whether the price covers only part of the trip, as when a train of another operator
    /// runs part of it.
    #[serde(default)]
    pub partial_price: bool,
}

/// Reads the connections from an answer of the service.
///
/// # Errors
///
/// Returns an error when the answer has no list of connections.
pub fn parse_connections(response: &Value) -> Result<Vec<Connection>, QuoteError> {
    let connections = response
        .get("verbindungen")
        .and_then(Value::as_array)
        .ok_or_else(|| QuoteError::Unexpected("no connections in the answer".to_owned()))?;
    Ok(connections
        .iter()
        .filter_map(|entry| {
            let sections: Vec<&Value> = entry
                .pointer("/verbindung/verbindungsAbschnitte")?
                .as_array()?
                .iter()
                .filter(|section| section.get("typ").and_then(Value::as_str) != Some("FUSSWEG"))
                .collect();
            let departure = sections
                .first()?
                .get("abgangsDatum")?
                .as_str()
                .and_then(|text| DateTime::parse_from_rfc3339(text).ok())?;
            let trains = sections
                .iter()
                .filter_map(|section| section.get("mitteltext")?.as_str().map(str::to_owned))
                .collect();
            let price = entry
                .pointer("/angebote/preise/gesamt/ab")
                .filter(|amount| amount.get("waehrung").and_then(Value::as_str) == Some("EUR"))
                .and_then(|amount| amount.get("betrag")?.as_f64());
            let partial_price = entry
                .pointer("/angebote/preise/istTeilpreis")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Some(Connection {
                departure,
                trains,
                price,
                partial_price,
            })
        })
        .collect())
}

/// Returns the connection that is the planned trip, or `None` when the answer has none.
///
/// A connection is the planned trip when it leaves at the planned moment, in seconds since the
/// Unix epoch, with the planned number of trains. Where the plan names its trains, such as
/// `ICE 848`, the trains decide instead. The timetable and DB can disagree on a city train at
/// the start or the end of the trip, such as the `S7` at 12:18 that DB lists as the `S9` at 12:20,
/// so those two trains need no match. Every train between them must be one of the connection's
/// trains, and at least one train must match. Among the connections that qualify, the one with
/// the most matches wins, then the one that leaves closest to the planned moment, then the first
/// with a price.
///
/// A connection that leaves more than `DEPARTURE_TOLERANCE_SECONDS` from the planned moment
/// never qualifies, because it can be another run of the same train numbers.
///
/// DB can list one journey twice under different train names, such as a Nightjet as `NJ 403`
/// without a price and its seats as `IC 60403` with one. Among equal connections, the first
/// with a price wins.
pub fn find_connection<'a>(
    connections: &'a [Connection],
    departure_unix_seconds: i64,
    train_count: usize,
    planned_trains: &[impl AsRef<str>],
) -> Option<&'a Connection> {
    let planned: Vec<TrainName> = planned_trains
        .iter()
        .map(|train| TrainName::parse(train.as_ref()))
        .collect();
    if planned.len() < 3 {
        // Without a train between the first and the last, the departure and the number of
        // trains are all there is to go on.
        let exact = connections.iter().filter(|connection| {
            connection.departure.timestamp() == departure_unix_seconds
                && connection.trains.len() == train_count
        });
        return first_priced(exact);
    }
    let middle = &planned[1..planned.len() - 1];
    let matches_of = |connection: &Connection| -> Option<usize> {
        let names: Vec<TrainName> = connection
            .trains
            .iter()
            .map(|train| TrainName::parse(train))
            .collect();
        let found = |train: &TrainName| names.contains(train);
        let distance = (connection.departure.timestamp() - departure_unix_seconds).abs();
        (distance <= DEPARTURE_TOLERANCE_SECONDS && middle.iter().all(found))
            .then(|| planned.iter().filter(|train| found(train)).count())
    };
    let scored: Vec<(&Connection, usize)> = connections
        .iter()
        .filter_map(|connection| Some((connection, matches_of(connection)?)))
        .collect();
    let best = scored.iter().map(|&(_, matches)| matches).max()?;
    let closest = scored
        .iter()
        .filter(|&&(_, matches)| matches == best)
        .map(|&(connection, _)| (connection.departure.timestamp() - departure_unix_seconds).abs())
        .min()?;
    first_priced(scored.iter().filter_map(|&(connection, matches)| {
        (matches == best
            && (connection.departure.timestamp() - departure_unix_seconds).abs() == closest)
            .then_some(connection)
    }))
}

/// The furthest the first train of a connection can leave from the planned moment and still
/// be the planned trip.
const DEPARTURE_TOLERANCE_SECONDS: i64 = 30 * 60;

/// A train as a category and a number, such as `ICE` and `848`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TrainName {
    /// The category in upper case, with the long names of the timetable shortened as DB writes
    /// them.
    category: String,
    /// The digits of the number.
    number: String,
}

impl TrainName {
    /// Reads a name such as `ICE 848`, `Intercity 28559` or `RE13`, with the letters before the
    /// digits as the category.
    fn parse(text: &str) -> Self {
        let text = text.trim();
        let split = text
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(text.len());
        let (category, number) = text.split_at(split);
        let category = category.trim().to_uppercase();
        let category = match category.as_str() {
            "INTERCITY" => "IC".to_owned(),
            "EUROCITY" => "EC".to_owned(),
            _ => category,
        };
        Self {
            category,
            number: number.trim().to_owned(),
        }
    }
}

/// Returns the first connection with a price, or the first connection when none has one.
fn first_priced<'a>(connections: impl Iterator<Item = &'a Connection>) -> Option<&'a Connection> {
    let mut matching = connections;
    let first = matching.next()?;
    if first.price.is_some() {
        return Some(first);
    }
    matching
        .find(|connection| connection.price.is_some())
        .or(Some(first))
}

/// Fetches connections from the service of the DB app, keeps them for a while and spaces its
/// requests out.
pub struct DbService {
    /// The HTTP client.
    client: Client,
    /// Keeps the connections of searches.
    store: Arc<dyn QuoteStore>,
    /// Spaces requests out and stops them after a failure.
    limiter: RequestLimiter,
}

impl DbService {
    /// Creates a service that keeps its answers in a store.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be built.
    pub fn new(store: Arc<dyn QuoteStore>) -> Result<Self, QuoteError> {
        Ok(Self {
            client: Client::builder()
                .user_agent(USER_AGENT)
                .timeout(Duration::from_secs(20))
                .build()?,
            store,
            limiter: RequestLimiter::new(MINIMUM_INTERVAL, FAILURE_PAUSE),
        })
    }

    /// Returns the connections of a search. A search made in the last half hour is answered
    /// from the store.
    ///
    /// # Errors
    ///
    /// Returns an error when requests are paused after a failure, the request fails or is
    /// refused, or the answer has an unexpected shape. A failure pauses requests for 15 minutes.
    pub async fn connections(&self, search: &Search) -> Result<Vec<Connection>, QuoteError> {
        let key = search.cache_key();
        if let Some(connections) = self
            .store
            .load(&key, QUOTE_LIFETIME)
            .and_then(|document| serde_json::from_str(&document).ok())
        {
            return Ok(connections);
        }
        let result = crate::with_one_retry(|| self.fetch(search)).await;
        match &result {
            Ok(connections) => {
                if let Ok(document) = serde_json::to_string(connections) {
                    self.store.save(&key, &document);
                }
            }
            Err(QuoteError::Paused) => {}
            Err(_) => self.limiter.report_failure().await,
        }
        result
    }

    /// Asks the service for the connections of a search.
    async fn fetch(&self, search: &Search) -> Result<Vec<Connection>, QuoteError> {
        if !self.limiter.wait_for_turn().await {
            return Err(QuoteError::Paused);
        }
        let response = self
            .client
            .post(SEARCH_URL)
            .header("content-type", MEDIA_TYPE)
            .header("accept", MEDIA_TYPE)
            .header("x-correlation-id", uuid::Uuid::now_v7().to_string())
            .body(search.request().to_string())
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(QuoteError::Refused {
                url: SEARCH_URL.to_owned(),
                status: response.status().as_u16(),
            });
        }
        parse_connections(&response.json().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plan that names no trains.
    const NO_TRAINS: [&str; 0] = [];

    #[test]
    fn a_priced_listing_of_the_same_journey_wins() {
        let departure = DateTime::parse_from_rfc3339("2026-10-22T20:24:00+02:00").unwrap();
        let connection = |trains: [&str; 2], price: Option<f64>| Connection {
            departure,
            trains: trains.map(str::to_owned).to_vec(),
            price,
            partial_price: false,
        };
        let connections = [
            connection(["RE 8468", "NJ 403"], None),
            connection(["RE 8468", "IC 60403"], Some(52.99)),
        ];
        let found = find_connection(&connections, departure.timestamp(), 2, &NO_TRAINS).unwrap();
        assert_eq!(found.price, Some(52.99));
        assert!(find_connection(&connections[..1], departure.timestamp(), 2, &NO_TRAINS).is_some());
        assert!(find_connection(&connections, departure.timestamp(), 1, &NO_TRAINS).is_none());
    }

    fn connection(departure: &str, trains: &[&str], price: Option<f64>) -> Connection {
        Connection {
            departure: DateTime::parse_from_rfc3339(departure).unwrap(),
            trains: trains.iter().map(|&train| train.to_owned()).collect(),
            price,
            partial_price: false,
        }
    }

    fn moment(text: &str) -> i64 {
        DateTime::parse_from_rfc3339(text).unwrap().timestamp()
    }

    #[test]
    fn a_city_train_that_differs_from_the_timetable_still_matches() {
        let connections = [
            connection(
                "2026-10-24T12:15:00+02:00",
                &["S5", "RJ 178", "ICE 203", "IC 2038", "IC 770"],
                None,
            ),
            connection(
                "2026-10-24T12:20:00+02:00",
                &["S9", "ICE 848", "ICE 142", "IC 28470"],
                Some(137.99),
            ),
            connection(
                "2026-10-24T12:25:00+02:00",
                &["S5", "ICE 375", "RB58", "ICE 224", "IC 28474"],
                None,
            ),
        ];
        let planned = ["S7 7088", "ICE 848", "ICE 142", "Intercity 28470"];
        let found = find_connection(
            &connections,
            moment("2026-10-24T12:18:00+02:00"),
            4,
            &planned,
        );
        assert_eq!(found.unwrap().price, Some(137.99));
        // Another train in the middle of the plan is another trip.
        let other = ["S7 7088", "ICE 848", "ICE 999", "Intercity 28470"];
        assert!(
            find_connection(&connections, moment("2026-10-24T12:18:00+02:00"), 4, &other).is_none()
        );
    }

    #[test]
    fn a_listing_with_another_name_for_the_last_train_still_matches() {
        let connections = [
            connection(
                "2026-10-24T10:50:00+02:00",
                &["S3", "ICE 144", "IC 28559", "ICD 1877"],
                Some(143.99),
            ),
            connection(
                "2026-10-24T10:50:00+02:00",
                &["S3", "ICE 144", "IC 28559", "IC 1168"],
                Some(143.99),
            ),
        ];
        let planned = ["S7 7546", "ICE 144", "Intercity 28559", "Intercity 1168"];
        let found = find_connection(
            &connections,
            moment("2026-10-24T10:48:00+02:00"),
            4,
            &planned,
        )
        .unwrap();
        // The listing that also matches the last train wins.
        assert_eq!(found.trains[3], "IC 1168");
    }

    #[test]
    fn the_same_trains_far_from_the_planned_moment_do_not_match() {
        let connections = [connection(
            "2026-10-24T14:20:00+02:00",
            &["S9", "ICE 848", "ICE 142", "IC 28470"],
            Some(137.99),
        )];
        let planned = ["S7 7088", "ICE 848", "ICE 142", "Intercity 28470"];
        let found = find_connection(
            &connections,
            moment("2026-10-24T12:18:00+02:00"),
            4,
            &planned,
        );
        assert!(found.is_none());
    }

    #[test]
    fn a_plan_without_a_middle_train_matches_by_moment_and_count() {
        let connections = [connection(
            "2026-10-24T12:20:00+02:00",
            &["ICE 848", "IC 28470"],
            Some(60.0),
        )];
        let none: [&str; 0] = [];
        let at = |text: &str, count: usize| {
            find_connection(&connections, moment(text), count, &none).is_some()
        };
        assert!(at("2026-10-24T12:20:00+02:00", 2));
        assert!(!at("2026-10-24T12:20:00+02:00", 1));
        assert!(!at("2026-10-24T12:18:00+02:00", 2));
    }

    #[test]
    fn connections_are_read_without_walks() {
        let response = json!({ "verbindungen": [
            {
                "verbindung": { "verbindungsAbschnitte": [
                    { "typ": "FUSSWEG", "abgangsDatum": "2026-10-22T09:30:00+02:00" },
                    { "typ": "FAHRZEUG", "mitteltext": "ICE 604",
                      "abgangsDatum": "2026-10-22T09:37:00+02:00" }
                ] },
                "angebote": { "preise": { "gesamt": { "ab": { "waehrung": "EUR", "betrag": 53.99 } } } }
            },
            {
                "verbindung": { "verbindungsAbschnitte": [
                    { "typ": "FAHRZEUG", "mitteltext": "IC 307",
                      "abgangsDatum": "2026-11-19T10:43:00+01:00" }
                ] },
                "angebote": { "preise": { "istTeilpreis": true,
                    "gesamt": { "ab": { "waehrung": "EUR", "betrag": 34.9 } } } }
            },
            {
                "verbindung": { "verbindungsAbschnitte": [
                    { "typ": "FAHRZEUG", "mitteltext": "FLX 1234",
                      "abgangsDatum": "2026-10-22T10:18:00+02:00" }
                ] },
                "angebote": {}
            }
        ] });
        let connections = parse_connections(&response).unwrap();
        assert_eq!(connections[0].trains, ["ICE 604"]);
        assert_eq!(connections[0].price, Some(53.99));
        assert!(!connections[0].partial_price);
        assert!(connections[1].partial_price);
        assert_eq!(connections[2].price, None);
        let departure = DateTime::parse_from_rfc3339("2026-10-22T09:37:00+02:00")
            .unwrap()
            .timestamp();
        assert!(find_connection(&connections, departure, 1, &NO_TRAINS).is_some());
        assert!(find_connection(&connections, departure, 2, &NO_TRAINS).is_none());
    }

    #[test]
    fn traveller_ids_of_links_map_to_types() {
        assert_eq!(traveller_type("13"), Some("ERWACHSENER"));
        assert_eq!(traveller_type("3"), None);
    }
}
