//! Prices from European Sleeper, booking.europeansleeper.eu.
//!
//! The booking site's service answers plain requests without a login. A search for a day lists the
//! trains between two stations, and the availability of a train lists the lowest price of every
//! kind of place for the travellers in the request. The service knows the Interrail reservation
//! as a traveller type of its own, so a pass holder is asked as that type.
//!
//! The main site lists last minute fares on pages of its own. They are a bed in a shared
//! compartment, with the ticket, at a fixed low price for trains one to three weeks away.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use chrono::{NaiveDate, NaiveDateTime};
use regex::Regex;
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

/// The address of the booking service.
const SERVICE: &str = "https://europeansleeperprod-api.azurewebsites.net";

/// The address of the main site, which holds the last minute pages.
const SITE: &str = "https://www.europeansleeper.eu";

/// The last minute pages of the main site, by their path.
const LAST_MINUTE_PAGES: &[&str] = &[
    "last-minutes-hamburg-berlin",
    "last-minutes-brussels-paris",
    "last-minutes-amsterdam-brussels",
    "last-minutes-berlin-prague",
];

/// The traveller type of an adult in the booking service.
const ADULT: u32 = 72;

/// The traveller type of an Interrail pass holder, who pays the reservation only.
const INTERRAIL: u32 = 44;

/// The least time between two requests.
const MINIMUM_INTERVAL: Duration = Duration::from_secs(1);

/// How long requests stop after one fails.
const FAILURE_PAUSE: Duration = Duration::from_mins(15);

/// How long the offers of a search are kept. Prices change with the places that are left.
const QUOTE_LIFETIME: Duration = Duration::from_mins(30);

/// The most days before a departure that the last minute pages are asked. The pages list trains
/// one to three weeks away, so a train further off is not on them.
const LAST_MINUTE_DAYS: i64 = 35;

/// The most minutes by which a train of the service may differ from the planned departure and
/// still be the planned train.
const DEPARTURE_TOLERANCE_MINUTES: i64 = 5;

/// A search on the booking service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Search {
    /// The code of the departure station in the service, such as `8400058` for Amsterdam CS.
    pub origin: String,
    /// The code of the arrival station in the service.
    pub destination: String,
    /// The planned local time of the departure.
    pub departure: NaiveDateTime,
}

impl Search {
    /// Returns the key under which the offers of the search are kept.
    fn cache_key(&self) -> String {
        format!(
            "european-sleeper:{}:{}:{}",
            self.origin, self.destination, self.departure
        )
    }
}

/// The kind of place that a train sells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceKind {
    /// A seat in a seating car.
    Seat,
    /// A bed in a shared compartment of five.
    Couchette,
    /// A bed in a compartment of two or three, which the service calls a berth or comfort place.
    Berth,
}

/// The lowest price in euro of each kind of place, for one traveller.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlacePrices {
    /// The lowest price of a seat.
    pub seat: Option<f64>,
    /// The lowest price of a bed in a shared compartment of five.
    pub couchette: Option<f64>,
    /// The lowest price of a bed in a compartment of two or three.
    pub berth: Option<f64>,
}

impl PlacePrices {
    /// Returns the price of a kind of place.
    pub fn of(&self, kind: PlaceKind) -> Option<f64> {
        match kind {
            PlaceKind::Seat => self.seat,
            PlaceKind::Couchette => self.couchette,
            PlaceKind::Berth => self.berth,
        }
    }

    /// Keeps the lower of the held price and a new one for a kind of place.
    fn offer(&mut self, kind: PlaceKind, price: f64) {
        let slot = match kind {
            PlaceKind::Seat => &mut self.seat,
            PlaceKind::Couchette => &mut self.couchette,
            PlaceKind::Berth => &mut self.berth,
        };
        *slot = Some(slot.map_or(price, |held| held.min(price)));
    }
}

/// What a train sells between two stations, with its prices for one traveller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SleeperOffers {
    /// The train, such as `ES 453`.
    pub train: String,
    /// The local time the train leaves the departure station.
    pub departure: NaiveDateTime,
    /// Whether the service sells the train.
    pub bookable: bool,
    /// The lowest price of each kind of place for an adult. It holds the ticket and the place.
    pub adult: PlacePrices,
    /// The lowest price of each kind of place for an Interrail pass holder. It holds the
    /// reservation of the place, and no ticket.
    pub pass: PlacePrices,
    /// The last minute fare for a bed in a shared compartment, with the ticket, when the train
    /// has one.
    pub last_minute: Option<f64>,
}

impl SleeperOffers {
    /// Returns the price that an adult pays for a kind of place with the ticket, with a last
    /// minute fare counted for a bed in a shared compartment.
    pub fn whole_price(&self, kind: PlaceKind) -> Option<f64> {
        let regular = self.adult.of(kind);
        match (kind, self.last_minute) {
            (PlaceKind::Couchette, Some(deal)) => {
                Some(regular.map_or(deal, |price| price.min(deal)))
            }
            _ => regular,
        }
    }

    /// Returns the lowest price of a ticket for an adult, which is the cheapest place the train
    /// sells. A traveller who wants a seat or a bed pays at least this.
    pub fn cheapest_ticket(&self) -> Option<f64> {
        [PlaceKind::Seat, PlaceKind::Couchette, PlaceKind::Berth]
            .into_iter()
            .filter_map(|kind| self.whole_price(kind))
            .reduce(f64::min)
    }

    /// Returns what a traveller pays on top of the cheapest ticket for a kind of place, which is
    /// the accommodation of a traveller without a pass.
    pub fn extra_over_ticket(&self, kind: PlaceKind) -> Option<f64> {
        Some((self.whole_price(kind)? - self.cheapest_ticket()?).max(0.0))
    }

    /// Returns what a pass holder pays for a kind of place. It is the reservation, or the price of
    /// the same place with the ticket when that is lower, because a last minute fare can cost less
    /// than the reservation and the pass is then not needed for the train.
    pub fn pass_price(&self, kind: PlaceKind) -> Option<f64> {
        match (self.pass.of(kind), self.whole_price(kind)) {
            (Some(reservation), Some(whole)) => Some(reservation.min(whole)),
            (reservation, whole) => reservation.or(whole),
        }
    }

    /// Returns whether a pass holder does better without the pass on a kind of place, so the
    /// price of the place with the ticket is below the reservation.
    pub fn is_cheaper_without_pass(&self, kind: PlaceKind) -> bool {
        match (self.pass.of(kind), self.whole_price(kind)) {
            (Some(reservation), Some(whole)) => whole < reservation,
            _ => false,
        }
    }
}

/// A train of a day search, which the availability request names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripReference {
    /// The route of the train in the service.
    pub route_id: String,
    /// The number of the train, such as `453`.
    pub train_number: String,
    /// The day the train starts its run, as `YYYY-MM-DD`.
    pub travel_date: String,
    /// The local time the train leaves the departure station.
    pub departure: NaiveDateTime,
    /// Whether the service sells the train.
    pub bookable: bool,
}

/// Returns the train of a day search that leaves at the planned time, within a few minutes.
pub fn find_trip(answer: &Value, departure: NaiveDateTime) -> Option<TripReference> {
    answer
        .get("routeSearchResults")?
        .as_array()?
        .iter()
        .flat_map(|route| {
            route
                .get("searchResultOutward")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|result| {
            let trip = result.get("trip").filter(|trip| !trip.is_null())?;
            let leaves = NaiveDateTime::parse_from_str(
                trip.get("departureTime")?.as_str()?.get(..19)?,
                "%Y-%m-%dT%H:%M:%S",
            )
            .ok()?;
            Some(TripReference {
                route_id: result.get("routeId")?.as_str()?.to_owned(),
                train_number: trip.get("trainNumber")?.as_str()?.to_owned(),
                travel_date: trip.get("travelDate")?.as_str()?.to_owned(),
                departure: leaves,
                bookable: trip
                    .get("bookable")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            })
        })
        .filter(|trip| {
            (trip.departure - departure).num_minutes().abs() <= DEPARTURE_TOLERANCE_MINUTES
        })
        .min_by_key(|trip| (trip.departure - departure).num_minutes().abs())
}

/// Returns the kind of place of a place type key of the service, or `None` for a place that is
/// not priced per traveller. A private compartment is priced for the whole compartment.
fn place_kind(key: &str) -> Option<PlaceKind> {
    if key.contains("private") {
        return None;
    }
    if key.starts_with("seat") {
        Some(PlaceKind::Seat)
    } else if key.starts_with("couchette") {
        Some(PlaceKind::Couchette)
    } else if key.starts_with("berth") || key.starts_with("comfort") {
        Some(PlaceKind::Berth)
    } else {
        None
    }
}

/// Reads the lowest price of each kind of place from an availability answer. A place that is
/// sold out or has no price is left out.
pub fn parse_availability(answer: &Value) -> PlacePrices {
    let mut prices = PlacePrices::default();
    let classes = answer
        .pointer("/availabilityResult/availability/priceClasses")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    for class in classes {
        let Some(kind) = class
            .get("placeTypeKey")
            .and_then(Value::as_str)
            .and_then(place_kind)
        else {
            continue;
        };
        let fares = class
            .get("fareTypes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|fare| fare.get("bookable").and_then(Value::as_bool) == Some(true))
            .filter_map(|fare| fare.pointer("/prices/eur")?.as_f64());
        if let Some(lowest) = fares.reduce(f64::min) {
            prices.offer(kind, lowest);
        }
    }
    prices
}

/// A last minute page of the main site.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LastMinutePage {
    /// The codes of the stations a train of the page can start from.
    pub origins: Vec<String>,
    /// The codes of the stations a train of the page can end at.
    pub destinations: Vec<String>,
    /// The fares of the page.
    pub deals: Vec<LastMinuteDeal>,
}

/// A last minute fare of one day.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LastMinuteDeal {
    /// The day the train leaves.
    pub date: NaiveDate,
    /// The price per traveller of a bed in a shared compartment, with the ticket.
    pub classic: f64,
}

impl LastMinutePage {
    /// Returns the last minute fare for a bed on a train from one station to another on a day.
    pub fn fare(&self, origin: &str, destination: &str, date: NaiveDate) -> Option<f64> {
        let listed =
            |stations: &[String], code: &str| stations.iter().any(|station| station == code);
        (listed(&self.origins, origin) && listed(&self.destinations, destination))
            .then(|| {
                self.deals
                    .iter()
                    .filter(|deal| deal.date == date)
                    .map(|deal| deal.classic)
                    .reduce(f64::min)
            })
            .flatten()
    }
}

/// A station select of a last minute page, with its id and its options.
static SELECT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<select[^>]*id="([a-z-]+)".*?</select>"#).expect("the pattern is valid")
});

/// A station option of a select, with the code of the station.
static OPTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<option value="(\d+)""#).expect("the pattern is valid"));

/// A tab of a last minute page, with its number and its name.
static TAB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)name="div-tab-(\d+)">\s*([^<]*?)\s*</a>"#).expect("the pattern is valid")
});

/// A fare of a last minute page, with the day and the price.
static DEAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)(\w{3} \d{1,2} \w{3} \d{2})\s*<button[^>]*>\s*&euro;(\d+(?:\.\d+)?)")
        .expect("the pattern is valid")
});

/// Reads the codes of the stations in the select with an id on a last minute page.
fn station_codes(html: &str, select_id: &str) -> Vec<String> {
    let mut codes: Vec<String> = SELECT
        .captures_iter(html)
        .filter(|select| &select[1] == select_id)
        .flat_map(|select| {
            OPTION
                .captures_iter(&select[0])
                .map(|option| option[1].to_owned())
                .collect::<Vec<_>>()
        })
        .collect();
    codes.sort_unstable();
    codes.dedup();
    codes
}

/// Reads the stations and the fares of a last minute page. Only the fares of a shared compartment
/// count. The page also sells a private compartment, which is priced for up to five travellers.
pub fn parse_last_minute(html: &str) -> LastMinutePage {
    let shared_tabs: Vec<String> = TAB
        .captures_iter(html)
        .filter(|tab| !tab[2].to_lowercase().contains("private"))
        .map(|tab| format!(r#"id="div-tab-{}""#, &tab[1]))
        .collect();
    let mut deals = Vec::new();
    for marker in &shared_tabs {
        let Some(start) = html.find(marker.as_str()) else {
            continue;
        };
        // A tab runs to the start of the next one, or to the end of the page.
        let rest = &html[start + marker.len()..];
        let end = rest.find(r#"id="div-tab-"#).unwrap_or(rest.len());
        for fare in DEAL.captures_iter(&rest[..end]) {
            let date = NaiveDate::parse_from_str(&fare[1], "%a %d %b %y").ok();
            let price = fare[2].parse().ok();
            if let (Some(date), Some(classic)) = (date, price) {
                deals.push(LastMinuteDeal { date, classic });
            }
        }
    }
    LastMinutePage {
        origins: station_codes(html, "departure-station"),
        destinations: station_codes(html, "arrival-station"),
        deals,
    }
}

/// Returns today's date in UTC.
fn today() -> NaiveDate {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0));
    chrono::DateTime::from_timestamp(seconds, 0)
        .unwrap_or_default()
        .date_naive()
}

/// Fetches the offers of European Sleeper trains, keeps them for a while and spaces its requests
/// out.
pub struct EuropeanSleeperService {
    /// The HTTP client.
    client: Client,
    /// Keeps the offers of searches and the last minute pages.
    store: Arc<dyn QuoteStore>,
    /// Spaces requests out and stops them after a failure.
    limiter: RequestLimiter,
}

impl EuropeanSleeperService {
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

    /// Returns what the train of a search sells with its prices, or `None` when the service lists
    /// no train at the planned departure. An answer is kept for half an hour.
    ///
    /// # Errors
    ///
    /// Returns an error when requests are paused after a failure, a request fails or is refused,
    /// or an answer has an unexpected shape. A failure pauses requests for 15 minutes.
    pub async fn offers(&self, search: &Search) -> Result<Option<SleeperOffers>, QuoteError> {
        let key = search.cache_key();
        if let Some(offers) = self
            .store
            .load(&key, QUOTE_LIFETIME)
            .and_then(|document| serde_json::from_str(&document).ok())
        {
            return Ok(Some(offers));
        }
        let result = crate::with_one_retry(|| self.fetch_offers(search)).await;
        match &result {
            Ok(Some(offers)) => {
                if let Ok(document) = serde_json::to_string(offers) {
                    self.store.save(&key, &document);
                }
            }
            Ok(None) | Err(QuoteError::Paused) => {}
            Err(_) => self.limiter.report_failure().await,
        }
        result
    }

    /// Posts a request to the booking service and returns its answer.
    async fn post(&self, path: &str, body: Value) -> Result<Value, QuoteError> {
        if !self.limiter.wait_for_turn().await {
            return Err(QuoteError::Paused);
        }
        let url = format!("{SERVICE}{path}");
        let response = self
            .client
            .post(&url)
            .header("accept", "application/json")
            .header("origin", "https://booking.europeansleeper.eu")
            .json(&body)
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

    /// Asks for the availability of a train for a kind of traveller.
    async fn availability(
        &self,
        search: &Search,
        trip: &TripReference,
        traveller_type: u32,
    ) -> Result<PlacePrices, QuoteError> {
        let answer = self
            .post(
                "/api/search/availability",
                json!({
                    "trainRouteId": trip.route_id,
                    "fromLocationId": search.origin,
                    "toLocationId": search.destination,
                    "passengerTypes": [traveller_type],
                    "trainNumber": trip.train_number,
                    "travelDate": trip.travel_date,
                    "bicycleCount": 0,
                    "petsCount": 0,
                    "connectingTrainSelection": null,
                }),
            )
            .await?;
        Ok(parse_availability(&answer))
    }

    /// Makes the requests for the offers of a train.
    async fn fetch_offers(&self, search: &Search) -> Result<Option<SleeperOffers>, QuoteError> {
        let day = search.departure.date();
        let found = match self
            .post(
                "/api/search/trains",
                json!({
                    "fromLocationId": search.origin,
                    "toLocationId": search.destination,
                    "departureDate": format!("{day}T00:00:00.000Z"),
                    "passengerTypes": [ADULT],
                    "bicycleCount": 0,
                    "petsCount": 0,
                    "returnDate": null,
                }),
            )
            .await
        {
            Ok(found) => found,
            // The service refuses a pair of stations that no train of a day connects, which is
            // no reason to stop asking it about other trains.
            Err(QuoteError::Refused {
                status: 400 | 422, ..
            }) => return Ok(None),
            Err(error) => return Err(error),
        };
        let Some(trip) = find_trip(&found, search.departure) else {
            return Ok(None);
        };
        let adult = self.availability(search, &trip, ADULT).await?;
        let pass = self.availability(search, &trip, INTERRAIL).await?;
        let last_minute = self.last_minute_fare(search).await;
        Ok(Some(SleeperOffers {
            train: format!("ES {}", trip.train_number),
            departure: trip.departure,
            bookable: trip.bookable,
            adult,
            pass,
            last_minute,
        }))
    }

    /// Returns the last minute fare for the train of a search, or `None` when the train is too far
    /// away for the pages, has no fare, or the pages cannot be read. A page that fails to load
    /// only costs the fare, not the offers of the train.
    async fn last_minute_fare(&self, search: &Search) -> Option<f64> {
        let date = search.departure.date();
        let days = (date - today()).num_days();
        if !(0..=LAST_MINUTE_DAYS).contains(&days) {
            return None;
        }
        let mut lowest: Option<f64> = None;
        for page in LAST_MINUTE_PAGES {
            let Ok(page) = self.last_minute_page(page).await else {
                continue;
            };
            if let Some(fare) = page.fare(&search.origin, &search.destination, date) {
                lowest = Some(lowest.map_or(fare, |held| held.min(fare)));
            }
        }
        lowest
    }

    /// Returns a last minute page, from the store when it was read in the last half hour.
    async fn last_minute_page(&self, path: &str) -> Result<LastMinutePage, QuoteError> {
        let key = format!("european-sleeper-page:{path}");
        if let Some(page) = self
            .store
            .load(&key, QUOTE_LIFETIME)
            .and_then(|document| serde_json::from_str(&document).ok())
        {
            return Ok(page);
        }
        if !self.limiter.wait_for_turn().await {
            return Err(QuoteError::Paused);
        }
        let url = format!("{SITE}/{path}");
        let response = self.client.get(&url).send().await?;
        if !response.status().is_success() {
            return Err(QuoteError::Refused {
                url,
                status: response.status().as_u16(),
            });
        }
        let page = parse_last_minute(&response.text().await?);
        if let Ok(document) = serde_json::to_string(&page) {
            self.store.save(&key, &document);
        }
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(date: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(date, "%Y-%m-%dT%H:%M").unwrap()
    }

    fn fares(first: f64, rest: &[(&str, f64)]) -> Value {
        let fare = |id: &str, price: f64, bookable: bool| json!({ "id": id, "bookable": bookable, "prices": { "eur": price } });
        let mut classes = vec![json!({
            "placeTypeKey": "seat-second-class",
            "fareTypes": [fare("easy-night", first, true), fare("flex-night", first + 40.0, true)],
        })];
        for (key, price) in rest {
            classes.push(json!({
                "placeTypeKey": key,
                "fareTypes": [fare("easy-night", *price, true), fare("sold-out", 1.0, false)],
            }));
        }
        json!({ "availabilityResult": { "availability": { "priceClasses": classes } } })
    }

    #[test]
    fn availability_gives_the_lowest_bookable_price_of_each_kind() {
        let answer = fares(
            109.99,
            &[
                ("couchette-5", 219.99),
                ("couchette-5-women-only", 229.99),
                ("couchette-5-private", 1099.99),
                ("berth-double", 369.99),
                ("comfort-triple", 239.99),
                ("comfort-single", 500.0),
            ],
        );
        let prices = parse_availability(&answer);
        assert_eq!(prices.seat, Some(109.99));
        // The price of a private compartment is for all its places, so it is no price per bed.
        assert_eq!(prices.couchette, Some(219.99));
        // A fare that is not bookable does not count.
        assert_eq!(prices.berth, Some(239.99));
        // A train that lists no classes sells nothing.
        assert_eq!(parse_availability(&json!({})), PlacePrices::default());
    }

    #[test]
    fn the_trip_is_the_train_at_the_planned_departure() {
        let result = |route: &str, number: &str, time: &str| {
            json!({
                "routeId": route,
                "departureDateTime": time,
                "trip": { "trainNumber": number, "travelDate": &time[..10],
                          "departureTime": time, "bookable": true },
            })
        };
        let answer = json!({ "routeSearchResults": [{ "searchResultOutward": [
            { "routeId": "r", "departureDateTime": "2026-12-17T00:00:00", "trip": null },
            result("a", "453", "2026-12-18T22:34:00"),
            result("b", "463", "2026-12-18T19:22:00"),
        ] }] });
        let trip = find_trip(&answer, at("2026-12-18T22:34")).unwrap();
        assert_eq!(
            (trip.route_id.as_str(), trip.train_number.as_str()),
            ("a", "453")
        );
        assert_eq!(trip.travel_date, "2026-12-18");
        // A planned time a few minutes off still finds the train, and a far one does not.
        assert!(find_trip(&answer, at("2026-12-18T22:36")).is_some());
        assert!(find_trip(&answer, at("2026-12-18T21:00")).is_none());
        assert!(find_trip(&json!({}), at("2026-12-18T22:34")).is_none());
    }

    fn offers() -> SleeperOffers {
        SleeperOffers {
            train: "ES 453".to_owned(),
            departure: at("2026-12-18T22:34"),
            bookable: true,
            adult: PlacePrices {
                seat: Some(109.99),
                couchette: Some(219.99),
                berth: Some(239.99),
            },
            pass: PlacePrices {
                seat: Some(11.0),
                couchette: Some(64.0),
                berth: Some(79.0),
            },
            last_minute: None,
        }
    }

    #[test]
    fn an_adult_pays_the_extra_over_the_cheapest_ticket_for_a_bed() {
        let offers = offers();
        assert_eq!(offers.cheapest_ticket(), Some(109.99));
        let couchette = offers.extra_over_ticket(PlaceKind::Couchette).unwrap();
        assert!((couchette - 110.0).abs() < 1e-9, "{couchette}");
        let berth = offers.extra_over_ticket(PlaceKind::Berth).unwrap();
        assert!((berth - 130.0).abs() < 1e-9, "{berth}");
        // A seat has no extra over itself.
        assert_eq!(offers.extra_over_ticket(PlaceKind::Seat), Some(0.0));
    }

    #[test]
    fn a_last_minute_fare_is_the_cheapest_ticket_and_holds_the_bed() {
        let offers = SleeperOffers {
            last_minute: Some(40.0),
            ..offers()
        };
        assert_eq!(offers.cheapest_ticket(), Some(40.0));
        // The bed is in the fare, so the extra for it is nothing, and a berth costs its price less
        // the fare.
        assert_eq!(offers.extra_over_ticket(PlaceKind::Couchette), Some(0.0));
        let berth = offers.extra_over_ticket(PlaceKind::Berth).unwrap();
        assert!((berth - 199.99).abs() < 1e-9, "{berth}");
    }

    #[test]
    fn a_pass_holder_pays_the_reservation_unless_the_ticket_is_cheaper() {
        let regular = offers();
        assert_eq!(regular.pass_price(PlaceKind::Couchette), Some(64.0));
        assert!(!regular.is_cheaper_without_pass(PlaceKind::Couchette));
        let discounted = SleeperOffers {
            last_minute: Some(40.0),
            ..offers()
        };
        // The last minute fare of 40 euro is below the reservation of 64, so the pass is not needed.
        assert_eq!(discounted.pass_price(PlaceKind::Couchette), Some(40.0));
        assert!(discounted.is_cheaper_without_pass(PlaceKind::Couchette));
        assert_eq!(discounted.pass_price(PlaceKind::Berth), Some(79.0));
    }

    /// A trimmed last minute page with a shared and a private compartment tab.
    const PAGE: &str = r#"
        <select class="input" id="departure-station" name="departure-station" required>
          <option value>Choose a station</option>
          <optgroup label="Popular"><option value="8800104">Bruxelles-Midi</option></optgroup>
          <optgroup label="Belgium"><option value="8800104">Bruxelles-Midi</option>
            <option value="8800410">Liège</option></optgroup>
        </select>
        <select class="input" id="arrival-station" name="arrival-station" required>
          <option value>Choose a station</option><option value="8010100">Berlin</option>
        </select>
        <a class="tab" name="div-tab-0">CLASSIC</a>
        <a class="tab" name="div-tab-1">CLASSIC PRIVATE</a>
        <div class="div-tab" id="div-tab-0">
          <li> Tue 13 Oct 26 <button name="price" value="1326"> &euro;40 p.p. </button></li>
          <li> Sun 18 Oct 26 <button name="price" value="1327"> &euro;45 p.p. </button></li>
        </div>
        <div class="div-tab" id="div-tab-1">
          <li> Tue 13 Oct 26 <button name="price" value="1329"> &euro;160 </button></li>
        </div>"#;

    #[test]
    fn a_last_minute_page_lists_its_stations_and_the_fares_of_the_shared_compartment() {
        let page = parse_last_minute(PAGE);
        assert_eq!(page.origins, ["8800104", "8800410"]);
        assert_eq!(page.destinations, ["8010100"]);
        let tuesday = NaiveDate::from_ymd_opt(2026, 10, 13).unwrap();
        let sunday = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        // The private compartment of 160 euro is not a fare for one traveller.
        assert_eq!(page.fare("8800104", "8010100", tuesday), Some(40.0));
        assert_eq!(page.fare("8800410", "8010100", sunday), Some(45.0));
        // Another station pair, or another day, has no fare.
        assert_eq!(page.fare("8400058", "8010100", tuesday), None);
        assert_eq!(
            page.fare("8800104", "8010100", tuesday.succ_opt().unwrap()),
            None
        );
    }
}
