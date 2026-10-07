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
            Some(Connection {
                departure,
                trains,
                price,
            })
        })
        .collect())
}

/// Returns the connection that leaves at the given moment, in seconds since the Unix epoch, with
/// the given number of trains.
pub fn find_connection(
    connections: &[Connection],
    departure_unix_seconds: i64,
    train_count: usize,
) -> Option<&Connection> {
    connections.iter().find(|connection| {
        connection.departure.timestamp() == departure_unix_seconds
            && connection.trains.len() == train_count
    })
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
        let result = self.fetch(search).await;
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
                    { "typ": "FAHRZEUG", "mitteltext": "FLX 1234",
                      "abgangsDatum": "2026-10-22T10:18:00+02:00" }
                ] },
                "angebote": {}
            }
        ] });
        let connections = parse_connections(&response).unwrap();
        assert_eq!(connections[0].trains, ["ICE 604"]);
        assert_eq!(connections[0].price, Some(53.99));
        assert_eq!(connections[1].price, None);
        let departure = DateTime::parse_from_rfc3339("2026-10-22T09:37:00+02:00")
            .unwrap()
            .timestamp();
        assert!(find_connection(&connections, departure, 1).is_some());
        assert!(find_connection(&connections, departure, 2).is_none());
    }

    #[test]
    fn traveller_ids_of_links_map_to_types() {
        assert_eq!(traveller_type("13"), Some("ERWACHSENER"));
        assert_eq!(traveller_type("3"), None);
    }
}
