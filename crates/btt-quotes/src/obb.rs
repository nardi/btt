//! Prices from the ÖBB shop.
//!
//! The shop's own service answers plain requests with an anonymous token and a session that the
//! token opens. A search then takes three requests in a row, as the shop's page makes them: an
//! entry point that resolves the two station numbers into the shop's station records, the
//! timetable between those stations, and the prices of its connections. The token is kept and
//! fetched again shortly before it runs out.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDateTime;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::{QuoteError, QuoteStore, RequestLimiter};

/// What the client says about itself, in the generic form the booking sites accept.
const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (compatible; BigTrainTrip/",
    env!("CARGO_PKG_VERSION"),
    ")"
);

/// The address of the shop.
const SHOP: &str = "https://shop.oebbtickets.at";

/// The least time between two searches. The requests of one search follow each other at once,
/// as the shop's page makes them.
const MINIMUM_INTERVAL: Duration = Duration::from_secs(2);

/// How long searches stop after one fails.
const FAILURE_PAUSE: Duration = Duration::from_mins(15);

/// How long the connections of a search are kept. Saver prices change with demand, so the time
/// is short.
const QUOTE_LIFETIME: Duration = Duration::from_mins(30);

/// How long a token is used. The shop's tokens run for five minutes.
const TOKEN_LIFETIME: Duration = Duration::from_mins(4);

/// A search on the shop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Search {
    /// ÖBB number of the departure station, such as `8100002` for Salzburg Hbf.
    pub origin: u64,
    /// ÖBB number of the arrival station.
    pub destination: u64,
    /// The local time to leave at or after.
    pub departure: NaiveDateTime,
}

impl Search {
    /// Returns the key under which the connections of the search are kept.
    fn cache_key(&self) -> String {
        format!(
            "obb:{}:{}:{}",
            self.origin, self.destination, self.departure
        )
    }
}

/// A connection the shop found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    /// Local departure of the first train.
    pub departure: NaiveDateTime,
    /// The trains, such as `RJX 187`.
    pub trains: Vec<String>,
    /// The lowest price in euro, or `None` when the shop sells no ticket for it.
    pub price: Option<f64>,
    /// The kind of the lowest price, such as `Sparschiene ticket`, when the shop names one.
    pub note: Option<String>,
}

/// One kind of accommodation on a night train, with its price in each tariff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Accommodation {
    /// The kind: `SEAT`, `COUCHETTE` or `BERTH`.
    pub kind: String,
    /// The name of the compartment in English, such as `Mini Cabin`.
    pub name: String,
    /// The price in euro of each tariff, such as `NON-FLEX`, by the name of the tariff.
    pub prices: BTreeMap<String, f64>,
}

/// The offers of the shop for the accommodation of one night train.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NightOffers {
    /// The train, such as `NJ 466`.
    pub train: String,
    /// The accommodations with a price.
    pub accommodations: Vec<Accommodation>,
}

impl NightOffers {
    /// Returns whether the shop sells seats on the train. Some night trains sell beds only.
    pub fn sells_seats(&self) -> bool {
        self.accommodations
            .iter()
            .any(|accommodation| accommodation.kind == "SEAT")
    }

    /// Returns the lowest price in euro of a kind of accommodation over all tariffs. On a train
    /// without seats this is the price of the whole ticket with the bed.
    pub fn lowest_whole_price(&self, kind: &str) -> Option<f64> {
        self.lowest_price(kind, true)
    }

    /// Returns the lowest price in euro of a kind of accommodation, `SEAT`, `COUCHETTE` or
    /// `BERTH`.
    ///
    /// With a pass, the offers are those of a pass holder, and the price is the lowest of them.
    /// Without one, the price is the extra over a seat: the lowest difference, over the tariffs,
    /// between the kind and a seat of the same tariff. The ticket of a night train holds a seat,
    /// so the accommodation is what the traveller adds to it. A train without seats has none.
    pub fn lowest_price(&self, kind: &str, with_pass: bool) -> Option<f64> {
        let lowest = |kind: &str, tariff: Option<&str>| {
            self.accommodations
                .iter()
                .filter(|accommodation| accommodation.kind == kind)
                .flat_map(|accommodation| {
                    accommodation.prices.iter().filter(move |(name, _)| {
                        tariff.is_none_or(|wanted| name.as_str() == wanted)
                    })
                })
                .map(|(_, price)| *price)
                .reduce(f64::min)
        };
        if with_pass || kind == "SEAT" {
            return lowest(kind, None);
        }
        let tariffs: std::collections::BTreeSet<&str> = self
            .accommodations
            .iter()
            .flat_map(|accommodation| accommodation.prices.keys().map(String::as_str))
            .collect();
        tariffs
            .into_iter()
            .filter_map(|tariff| {
                let bed = lowest(kind, Some(tariff))?;
                let seat = lowest("SEAT", Some(tariff))?;
                (bed > seat).then_some(bed - seat)
            })
            .reduce(f64::min)
    }
}

/// The card of the Interrail Global Pass in the list of cards of the shop.
fn interrail_card() -> Value {
    json!({
        "cardId": 2391,
        "name": "Interrail Globalpass",
        "numberRequired": false,
        "isValidated": true,
        "isSelectable": true,
        "isDiscountCard": false,
    })
}

/// Returns the id of the connection of the timetable that is the single train at the given
/// departure.
pub fn night_connection_id(
    timetable: &Value,
    departure: NaiveDateTime,
    train: &str,
) -> Option<String> {
    timetable
        .get("connections")?
        .as_array()?
        .iter()
        .find_map(|connection| {
            let sections = connection.get("sections")?.as_array()?;
            let [section] = sections.as_slice() else {
                return None;
            };
            let category = section.get("category")?;
            let name = format!(
                "{} {}",
                category.get("name")?.as_str()?,
                category.get("number")?.as_str()?
            );
            let leaves = NaiveDateTime::parse_from_str(
                connection.pointer("/from/departure")?.as_str()?.get(..19)?,
                "%Y-%m-%dT%H:%M:%S",
            )
            .ok()?;
            (name == train && leaves == departure)
                .then(|| connection.get("id")?.as_str().map(str::to_owned))?
        })
}

/// Reads the accommodations of a train and their prices from the offers answer of the shop. A
/// compartment that the shop lists without a price is left out.
pub fn parse_night_offers(offers: &Value) -> Vec<Accommodation> {
    let Some(section) = offers.pointer("/offerSections/0") else {
        return Vec::new();
    };
    let compartments: BTreeMap<u64, (String, String)> = section
        .pointer("/media/compartments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|compartment| {
            Some((
                compartment.get("id")?.as_u64()?,
                (
                    compartment.get("accommodationType")?.as_str()?.to_owned(),
                    compartment.pointer("/name/en")?.as_str()?.trim().to_owned(),
                ),
            ))
        })
        .collect();
    let mut found: BTreeMap<u64, Accommodation> = BTreeMap::new();
    let tariffs = section
        .pointer("/travelClasses/0/offers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    for offer in tariffs {
        let tariff = offer
            .pointer("/flexibility/en")
            .and_then(Value::as_str)
            .unwrap_or("UNKNOWN");
        let listed = offer
            .pointer("/reservation/compartments")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for compartment in listed {
            let (Some(id), Some(price)) = (
                compartment.get("id").and_then(Value::as_u64),
                compartment.get("price").and_then(Value::as_f64),
            ) else {
                continue;
            };
            let Some((kind, name)) = compartments.get(&id) else {
                continue;
            };
            found
                .entry(id)
                .or_insert_with(|| Accommodation {
                    kind: kind.clone(),
                    name: name.clone(),
                    prices: BTreeMap::new(),
                })
                .prices
                .insert(tariff.to_owned(), price);
        }
    }
    found.into_values().collect()
}

/// Reads the connections from the timetable answer and the prices answer of the shop.
///
/// # Errors
///
/// Returns an error when the timetable answer has no list of connections.
pub fn parse_connections(timetable: &Value, prices: &Value) -> Result<Vec<Connection>, QuoteError> {
    let connections = timetable
        .get("connections")
        .and_then(Value::as_array)
        .ok_or_else(|| QuoteError::Unexpected("no connections in the timetable".to_owned()))?;
    let offers = prices
        .get("offers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(connections
        .iter()
        .filter_map(|connection| {
            let id = connection.get("id")?.as_str()?;
            let departure = connection
                .pointer("/from/departure")?
                .as_str()
                .and_then(|text| {
                    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f").ok()
                })?;
            let trains = connection
                .get("sections")?
                .as_array()?
                .iter()
                .filter_map(|section| {
                    let category = section.get("category")?;
                    let name = category.get("name")?.as_str()?;
                    let number = category.get("number")?.as_str()?;
                    Some(format!("{name} {number}"))
                })
                .collect();
            let offer = offers
                .iter()
                .find(|offer| offer.get("connectionId").and_then(Value::as_str) == Some(id));
            let price = offer
                .filter(|offer| offer.get("offerError").and_then(Value::as_bool) != Some(true))
                .and_then(|offer| offer.get("price")?.as_f64());
            let note = offer
                .and_then(|offer| offer.pointer("/specialNote/en")?.as_str())
                .map(str::to_owned);
            Some(Connection {
                departure,
                trains,
                price,
                note,
            })
        })
        .collect())
}

/// Returns the current time in UTC as the shop writes it, such as `2026-10-09T17:00:00`.
fn now_text() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0));
    chrono::DateTime::from_timestamp(seconds, 0)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string()
}

/// Returns the passengers of a request: one adult, with the Interrail card when `with_pass`.
fn passengers(with_pass: bool) -> Value {
    json!([{
        "me": false, "remembered": false,
        "cards": if with_pass { vec![interrail_card()] } else { Vec::new() },
        "relations": [],
        "challengedFlags": {
            "hasHandicappedPass": false, "hasAssistanceDog": false,
            "hasWheelchair": false, "hasAttendant": false,
        },
        "id": 1, "type": "ADULT",
    }])
}

/// Builds the timetable request of a search from the answer of the entry point. The shop takes
/// the two stations of that answer, and a travel action too when the entry point returns one.
///
/// # Errors
///
/// Returns an error when the answer names neither the stations nor a travel action.
pub fn timetable_request(entry: &Value, search: &Search) -> Result<Value, QuoteError> {
    let mut request = json!({
        "datetimeDeparture": search.departure.format("%Y-%m-%dT%H:%M:00.000").to_string(),
        "filter": {
            "regionaltrains": false, "direct": false, "wheelchair": false,
            "bikes": false, "trains": false, "motorail": false, "connections": [],
        },
        "passengers": passengers(false),
        "count": 5,
        "sortType": "DEPARTURE",
    });
    let stations = entry.get("from").zip(entry.get("to"));
    let travel_action = entry.get("travelActionId").filter(|id| id.is_string());
    if stations.is_none() && travel_action.is_none() {
        return Err(QuoteError::Unexpected(
            "the entry point names no stations".to_owned(),
        ));
    }
    if let Some((from, to)) = stations {
        request["from"] = from.clone();
        request["to"] = to.clone();
    }
    if let Some(id) = travel_action {
        request["travelActionId"] = id.clone();
    }
    Ok(request)
}

/// Returns the connection that leaves at the given local time with the given number of trains.
pub fn find_connection(
    connections: &[Connection],
    departure: NaiveDateTime,
    train_count: usize,
) -> Option<&Connection> {
    connections.iter().find(|connection| {
        connection.departure == departure && connection.trains.len() == train_count
    })
}

/// Fetches connections and prices from the ÖBB shop, keeps them for a while and spaces its
/// searches out.
pub struct ObbService {
    /// The HTTP client.
    client: Client,
    /// Keeps the connections of searches.
    store: Arc<dyn QuoteStore>,
    /// Spaces searches out and stops them after a failure.
    limiter: RequestLimiter,
    /// The anonymous token and when it was fetched.
    token: Mutex<Option<(String, Instant)>>,
}

impl ObbService {
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
            token: Mutex::new(None),
        })
    }

    /// Returns the connections of a search with their prices. A search made in the last half
    /// hour is answered from the store.
    ///
    /// # Errors
    ///
    /// Returns an error when searches are paused after a failure, a request fails or is refused,
    /// or an answer has an unexpected shape. A failure pauses searches for 15 minutes.
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
            Err(_) => {
                *self.token.lock().await = None;
                self.limiter.report_failure().await;
            }
        }
        result
    }

    /// Returns the accommodations of a night train and their prices, or `None` when the shop
    /// lists no such train at the departure. With a pass, the prices are those of the holder of
    /// the Interrail Global Pass, who pays the accommodation with its reservation and no ticket.
    /// An answer is kept for half an hour.
    ///
    /// # Errors
    ///
    /// Returns an error when searches are paused after a failure, a request fails or is refused,
    /// or an answer has an unexpected shape. A failure pauses searches for 15 minutes.
    pub async fn night_offers(
        &self,
        search: &Search,
        train: &str,
        with_pass: bool,
    ) -> Result<Option<NightOffers>, QuoteError> {
        let key =
            format!("{}:{train}:{with_pass}", search.cache_key()).replacen("obb:", "obb-night:", 1);
        if let Some(offers) = self
            .store
            .load(&key, QUOTE_LIFETIME)
            .and_then(|document| serde_json::from_str(&document).ok())
        {
            return Ok(Some(offers));
        }
        let result =
            crate::with_one_retry(|| self.fetch_night_offers(search, train, with_pass)).await;
        match &result {
            Ok(Some(offers)) => {
                if let Ok(document) = serde_json::to_string(offers) {
                    self.store.save(&key, &document);
                }
            }
            Ok(None) | Err(QuoteError::Paused) => {}
            Err(_) => {
                *self.token.lock().await = None;
                self.limiter.report_failure().await;
            }
        }
        result
    }

    /// Makes the requests for the offers of a night train.
    async fn fetch_night_offers(
        &self,
        search: &Search,
        train: &str,
        with_pass: bool,
    ) -> Result<Option<NightOffers>, QuoteError> {
        if !self.limiter.wait_for_turn().await {
            return Err(QuoteError::Paused);
        }
        let token = self.token().await?;
        let entry = self
            .send(
                &token,
                "POST",
                "/api/hafas/v2/externalEntrypoint",
                Some(json!({
                    "cref": "oebb-startseite",
                    "stations": {
                        "from": { "number": search.origin },
                        "to": { "number": search.destination },
                    },
                })),
            )
            .await?;
        let timetable = match self
            .send(
                &token,
                "POST",
                "/api/hafas/v4/timetable",
                Some(timetable_request(&entry, search)?),
            )
            .await
        {
            Ok(timetable) => timetable,
            Err(QuoteError::Refused { status: 422, .. }) => return Ok(None),
            Err(error) => return Err(error),
        };
        let Some(connection_id) = night_connection_id(&timetable, search.departure, train) else {
            return Ok(None);
        };
        let offers = self
            .send(
                &token,
                "POST",
                "/api/offer/v6/offers",
                Some(json!({
                    "selection": { "connectionId": connection_id },
                    "passengers": passengers(with_pass),
                    "datetime": now_text(),
                })),
            )
            .await?;
        let accommodations = parse_night_offers(&offers);
        Ok((!accommodations.is_empty()).then(|| NightOffers {
            train: train.to_owned(),
            accommodations,
        }))
    }

    /// Makes the requests of one search.
    async fn fetch(&self, search: &Search) -> Result<Vec<Connection>, QuoteError> {
        if !self.limiter.wait_for_turn().await {
            return Err(QuoteError::Paused);
        }
        let token = self.token().await?;
        let entry = self
            .send(
                &token,
                "POST",
                "/api/hafas/v2/externalEntrypoint",
                Some(json!({
                    "cref": "oebb-startseite",
                    "stations": {
                        "from": { "number": search.origin },
                        "to": { "number": search.destination },
                    },
                })),
            )
            .await?;
        let timetable = match self
            .send(
                &token,
                "POST",
                "/api/hafas/v4/timetable",
                Some(timetable_request(&entry, search)?),
            )
            .await
        {
            Ok(timetable) => timetable,
            // The shop answers 422 for a search it cannot run, such as one between stations
            // without a connection that day. That says nothing about the shop itself.
            Err(QuoteError::Refused { status: 422, .. }) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let query: Vec<String> = timetable
            .get("connections")
            .and_then(Value::as_array)
            .map(|connections| {
                connections
                    .iter()
                    .filter_map(|connection| connection.get("id")?.as_str())
                    .map(|id| format!("connectionIds%5B%5D={id}"))
                    .collect()
            })
            .unwrap_or_default();
        let prices = if query.is_empty() {
            Value::Null
        } else {
            self.send(
                &token,
                "GET",
                &format!("/api/offer/v1/prices?{}", query.join("&")),
                None,
            )
            .await?
        };
        parse_connections(&timetable, &prices)
    }

    /// Returns a token that is still good, fetching a new one when needed.
    async fn token(&self) -> Result<String, QuoteError> {
        let mut token = self.token.lock().await;
        if let Some((value, fetched)) = token.as_ref()
            && fetched.elapsed() < TOKEN_LIFETIME
        {
            return Ok(value.clone());
        }
        let url = format!("{SHOP}/api/domain/v1/anonymousToken");
        let response = self
            .client
            .get(&url)
            .header("accept", "application/json")
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(QuoteError::Refused {
                url,
                status: response.status().as_u16(),
            });
        }
        let answer: Value = response.json().await?;
        let value = answer
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| QuoteError::Unexpected("no access token".to_owned()))?
            .to_owned();
        // The shop opens a session for the token, and refuses the entry point with status 440
        // until it has one.
        self.send(
            &value,
            "POST",
            "/api/domain/v1/initUserData",
            Some(json!({})),
        )
        .await?;
        *token = Some((value.clone(), Instant::now()));
        Ok(value)
    }

    /// Sends one request of the shop with its headers and returns the JSON answer.
    async fn send(
        &self,
        token: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, QuoteError> {
        let url = format!("{SHOP}{path}");
        let request = if method == "POST" {
            self.client.post(&url).json(&body.unwrap_or(Value::Null))
        } else {
            self.client.get(&url)
        };
        let response = request
            .header("accept", "application/json")
            .header("Channel", "inet")
            .header("Lang", "en")
            .header("ClientId", "2")
            .header("AccessToken", token)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(QuoteError::Refused {
                url,
                status: response.status().as_u16(),
            });
        }
        Ok(response.json().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed offers answer of the shop for a Nightjet, with a seat, a couchette and a berth.
    fn offers_answer() -> Value {
        let tariff = |name: &str, seat: f64, couchette: f64, berth: f64| {
            json!({
                "flexibility": { "en": name },
                "reservation": { "compartments": [
                    { "id": 5_000_120, "price": seat },
                    { "id": 5_000_111, "price": couchette },
                    { "id": 5_000_172, "price": berth },
                    { "id": 5_000_999, "price": 1.0 }
                ] }
            })
        };
        json!({ "offerSections": [{
            "media": { "compartments": [
                { "id": 5_000_120, "accommodationType": "SEAT",
                  "name": { "en": "Comfort seat 2nd class" } },
                { "id": 5_000_111, "accommodationType": "COUCHETTE",
                  "name": { "en": "Mini Cabin" } },
                { "id": 5_000_172, "accommodationType": "BERTH",
                  "name": { "en": "Comfort compartment for 2 passengers " } }
            ] },
            "travelClasses": [{ "offers": [
                tariff("NON-FLEX", 34.9, 114.9, 179.9),
                tariff("SEMI-FLEX", 69.9, 134.9, 209.9)
            ] }]
        }] })
    }

    #[test]
    fn offers_list_each_accommodation_with_its_price_per_tariff() {
        let found = parse_night_offers(&offers_answer());
        let kinds: Vec<&str> = found
            .iter()
            .map(|accommodation| accommodation.kind.as_str())
            .collect();
        // The compartments come in the order of their ids, and the one that the media table
        // does not describe is left out.
        assert_eq!(kinds, ["COUCHETTE", "SEAT", "BERTH"]);
        assert_eq!(found[0].name, "Mini Cabin");
        assert!((found[0].prices["NON-FLEX"] - 114.9).abs() < 1e-9);
        assert!((found[0].prices["SEMI-FLEX"] - 134.9).abs() < 1e-9);
        assert_eq!(found[2].name, "Comfort compartment for 2 passengers");
        assert!(parse_night_offers(&json!({})).is_empty());
    }

    #[test]
    fn the_extra_over_a_seat_is_taken_within_one_tariff() {
        let offers = NightOffers {
            train: "NJ 466".to_owned(),
            accommodations: parse_night_offers(&offers_answer()),
        };
        // Sparschiene: 114.90 - 34.90 and semi-flex: 134.90 - 69.90. The lower extra wins.
        let couchette = offers.lowest_price("COUCHETTE", false).unwrap();
        assert!((couchette - 65.0).abs() < 1e-9, "{couchette}");
        let berth = offers.lowest_price("BERTH", false).unwrap();
        assert!((berth - 140.0).abs() < 1e-9, "{berth}");
        // A pass holder pays the lowest price listed, which holds the reservation.
        let with_pass = offers.lowest_price("COUCHETTE", true).unwrap();
        assert!((with_pass - 114.9).abs() < 1e-9, "{with_pass}");
        // A train without seats has no extra over one.
        let without_seats = NightOffers {
            train: "NJ 402".to_owned(),
            accommodations: offers
                .accommodations
                .iter()
                .filter(|accommodation| accommodation.kind != "SEAT")
                .cloned()
                .collect(),
        };
        assert_eq!(without_seats.lowest_price("COUCHETTE", false), None);
        assert!(without_seats.lowest_price("COUCHETTE", true).is_some());
        // The beds of such a train sell at the price of the whole ticket.
        assert!(offers.sells_seats());
        assert!(!without_seats.sells_seats());
        let whole = without_seats.lowest_whole_price("COUCHETTE").unwrap();
        assert!((whole - 114.9).abs() < 1e-9, "{whole}");
    }

    #[test]
    fn a_night_train_is_found_by_its_name_and_departure() {
        let connection = |id: &str, name: &str, departure: &str, sections: usize| {
            let section = json!({ "category": { "name": name, "number": "466" } });
            json!({
                "id": id,
                "from": { "departure": departure },
                "sections": vec![section; sections]
            })
        };
        let timetable = json!({ "connections": [
            connection("a", "NJ", "2026-10-20T21:39:00.000", 2),
            connection("b", "NJ", "2026-10-20T23:00:00.000", 1),
            connection("c", "NJ", "2026-10-20T21:39:00.000", 1),
        ] });
        let departure = chrono::NaiveDate::from_ymd_opt(2026, 10, 20)
            .unwrap()
            .and_hms_opt(21, 39, 0)
            .unwrap();
        assert_eq!(
            night_connection_id(&timetable, departure, "NJ 466").as_deref(),
            Some("c")
        );
        assert_eq!(night_connection_id(&timetable, departure, "NJ 467"), None);
    }

    #[test]
    fn a_pass_holder_asks_with_the_interrail_card() {
        assert_eq!(passengers(true)[0]["cards"][0]["cardId"], 2391);
        assert!(passengers(false)[0]["cards"].as_array().unwrap().is_empty());
    }

    #[test]
    fn the_timetable_takes_the_stations_of_the_entry_point() {
        let search = Search {
            origin: 8_100_002,
            destination: 8_101_001,
            departure: chrono::NaiveDate::from_ymd_opt(2026, 10, 20)
                .unwrap()
                .and_hms_opt(8, 0, 0)
                .unwrap(),
        };
        let entry = json!({
            "from": { "number": 8_100_002, "name": "Salzburg Hbf" },
            "to": { "number": 1_270_504, "name": "Brixlegg Bahnhof" }
        });
        let request = timetable_request(&entry, &search).unwrap();
        assert_eq!(request["from"]["name"], "Salzburg Hbf");
        assert_eq!(request["to"]["number"], 1_270_504);
        assert_eq!(request["datetimeDeparture"], "2026-10-20T08:00:00.000");
        assert!(request.get("travelActionId").is_none());
        let with_action = json!({ "travelActionId": "abc" });
        assert_eq!(
            timetable_request(&with_action, &search).unwrap()["travelActionId"],
            "abc"
        );
        assert!(timetable_request(&json!({}), &search).is_err());
    }

    #[test]
    fn connections_take_their_prices_by_id() {
        let timetable = json!({ "connections": [
            { "id": "a", "from": { "departure": "2026-10-22T14:56:00.000" },
              "sections": [{ "category": { "name": "RJX", "number": "19982" } }] },
            { "id": "b", "from": { "departure": "2026-10-22T15:28:00.000" },
              "sections": [
                  { "category": { "name": "IC", "number": "648" } },
                  { "type": "walk" },
                  { "category": { "name": "RJX", "number": "187" } }
              ] }
        ] });
        let prices = json!({ "offers": [
            { "connectionId": "a", "price": 19.9, "offerError": false,
              "specialNote": { "en": "Sparschiene ticket" } },
            { "connectionId": "b", "offerError": true }
        ] });
        let connections = parse_connections(&timetable, &prices).unwrap();
        assert_eq!(connections[0].trains, ["RJX 19982"]);
        assert_eq!(connections[0].price, Some(19.9));
        assert_eq!(connections[0].note.as_deref(), Some("Sparschiene ticket"));
        assert_eq!(connections[1].trains, ["IC 648", "RJX 187"]);
        assert_eq!(connections[1].price, None);
        let departure =
            NaiveDateTime::parse_from_str("2026-10-22T14:56:00", "%Y-%m-%dT%H:%M:%S").unwrap();
        assert!(find_connection(&connections, departure, 1).is_some());
    }
}
