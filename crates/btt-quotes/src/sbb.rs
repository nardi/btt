//! Prices from the SBB booking site.
//!
//! The site's page is a shell. Its configuration names the GraphQL service that the page then
//! asks for the trips of a search and, with the ids of those trips, for their prices. This
//! module follows the same two steps from a booking link such as
//! `https://www.sbb.ch/en?stops=Basel+SBB_I8500010~Bern_I8507000&day=2026-10-16&time=06_30&moment=dep&reduction=none`.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{QuoteError, QuoteStore, RequestLimiter};

/// What the client says about itself. The CDN of the site refuses user agents that do not have
/// the generic form `Mozilla/5.0 (compatible; ...)`, so the name of this client sits inside it.
const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (compatible; BigTrainTrip/",
    env!("CARGO_PKG_VERSION"),
    ")"
);

/// The name sent in the Apollo client headers, which the service requires.
const CLIENT_NAME: &str = "btt-quotes";

/// The version sent in the Apollo client headers. The service refuses versions below a minimum
/// that lies between 1.0.0 and 17.9.9, the range of its own web client.
const CLIENT_VERSION: &str = "18.3.0";

/// The least time between two requests to the site.
const MINIMUM_INTERVAL: Duration = Duration::from_secs(2);

/// How long requests stop after one fails.
const FAILURE_PAUSE: Duration = Duration::from_mins(15);

/// How long the trips and prices of a search are kept. Saver prices change with demand, so the
/// time is short.
const QUOTE_LIFETIME: Duration = Duration::from_mins(30);

/// How long the address of the price service is kept.
const ADDRESS_LIFETIME: Duration = Duration::from_hours(24);

/// The query for the trips of a search, with the fields the quote reads.
const TRIPS_QUERY: &str = "query Trips($input: TripInput!, $language: LanguageEnum!) { \
    trips(tripInput: $input, language: $language) { trips { id isBuyable legs { __typename \
    ... on PTRideLeg { start { id name } end { id name } departure { time } arrival { time } \
    serviceJourney { serviceProducts { name line number } } } } } } }";

/// The query for the prices of trips, as the page sends it.
const PRICES_QUERY: &str = "query TripPrices($processId: ID!, $input: TripPricesQueryInput!) { \
    tripPrices(processId: $processId, input: $input) { tripId tripPrices { price { amount \
    currency } travelClass afterSaleFlexibility } bookingSystem } }";

/// A station of a search, as the booking link names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Station {
    /// Name of the station.
    pub name: String,
    /// The UIC number of the station, such as 8500010 for Basel SBB.
    pub id: String,
}

/// Whether the time of a search is a departure or an arrival.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    /// Leave at or after the time.
    Departure,
    /// Arrive at or before the time.
    Arrival,
}

/// Decodes a part of an address: `+` is a space, and `%XX` is a byte.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
                if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                    decoded.push(byte);
                    index += 2;
                } else {
                    decoded.push(b'%');
                }
            }
            byte => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Encodes a station name for a booking link: a space is `+`, and anything that is not a letter
/// or digit of ASCII or one of `-._~` is `%XX`.
pub fn percent_encode(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        match byte {
            b' ' => encoded.push('+'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            // Writing to a string never fails.
            other => {
                let _ = write!(encoded, "%{other:02X}");
            }
        }
    }
    encoded
}

/// The search of a booking link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Search {
    /// Where the journey starts.
    pub origin: Station,
    /// Where the journey ends.
    pub destination: Station,
    /// The day of the journey.
    pub date: NaiveDate,
    /// The time of day.
    pub time: NaiveTime,
    /// Whether the time is a departure or an arrival.
    pub moment: Moment,
}

impl Search {
    /// Reads the search from a booking link. Only a search with no reduction is supported.
    ///
    /// # Errors
    ///
    /// Returns an error when the link lacks a parameter, has an unknown form of one, or asks for
    /// a reduction.
    pub fn from_link(link: &str) -> Result<Self, QuoteError> {
        let bad = || QuoteError::BadLink(link.to_owned());
        let query = link.split_once('?').ok_or_else(bad)?.1;
        let parameter = |name: &str| {
            query
                .split('&')
                .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
        };
        let station = |text: &str| {
            let (name, id) = text.rsplit_once("_I")?;
            Some(Station {
                name: percent_decode(name),
                id: id.to_owned(),
            })
        };
        let mut stops = parameter("stops").ok_or_else(bad)?.split('~');
        let origin = stops.next().and_then(station).ok_or_else(bad)?;
        let destination = stops.next().and_then(station).ok_or_else(bad)?;
        let date = parameter("day")
            .and_then(|day| day.parse().ok())
            .ok_or_else(bad)?;
        let time = parameter("time")
            .and_then(|time| NaiveTime::parse_from_str(&time.replace('_', ":"), "%H:%M").ok())
            .ok_or_else(bad)?;
        let moment = match parameter("moment") {
            Some("dep") | None => Moment::Departure,
            Some("arr") => Moment::Arrival,
            Some(_) => return Err(bad()),
        };
        if parameter("reduction").is_some_and(|reduction| reduction != "none") {
            return Err(bad());
        }
        Ok(Self {
            origin,
            destination,
            date,
            time,
            moment,
        })
    }

    /// Returns the booking link of the search, in the form the site uses.
    pub fn booking_link(&self) -> String {
        format!(
            "https://www.sbb.ch/en?stops={}_I{}~{}_I{}&day={}&time={}&moment={}&reduction=none",
            percent_encode(&self.origin.name),
            self.origin.id,
            percent_encode(&self.destination.name),
            self.destination.id,
            self.date,
            self.time.format("%H_%M"),
            match self.moment {
                Moment::Departure => "dep",
                Moment::Arrival => "arr",
            },
        )
    }

    /// Returns the key under which the quotes of the search are kept.
    pub fn cache_key(&self) -> String {
        format!("sbb:{}", self.booking_link())
    }

    /// Builds the GraphQL request for the trips of the search.
    pub fn trips_request(&self) -> Value {
        json!({
            "query": TRIPS_QUERY,
            "variables": {
                "input": {
                    "places": [
                        { "type": "ID", "value": self.origin.id },
                        { "type": "ID", "value": self.destination.id },
                    ],
                    "time": {
                        "date": self.date.to_string(),
                        "time": self.time.format("%H:%M").to_string(),
                        "type": match self.moment {
                            Moment::Departure => "DEPARTURE",
                            Moment::Arrival => "ARRIVAL",
                        },
                    },
                    "includeEconomic": false,
                    "directConnection": false,
                    "includeAccessibility": "NONE",
                    "includeNoticeAttributes": [],
                    "includeTransportModes": [
                        "HIGH_SPEED_TRAIN", "INTERCITY", "INTERREGIO", "REGIO", "URBAN_TRAIN",
                        "SPECIAL_TRAIN", "SHIP", "BUS", "TRAMWAY",
                        "CABLEWAY_GONDOLA_CHAIRLIFT_FUNICULAR",
                    ],
                    "includeUnsharp": false,
                    "occupancy": "ALL",
                    "walkSpeed": 100,
                },
                "language": "EN",
            },
        })
    }

    /// Builds the GraphQL request for the prices of trips of this search.
    pub fn prices_request(&self, process_id: Uuid, trip_ids: &[String]) -> Value {
        json!({
            "query": PRICES_QUERY,
            "variables": {
                "processId": process_id.to_string(),
                "input": {
                    "fromPlace": self.origin.id,
                    "toPlace": self.destination.id,
                    "travelClass": "ANY_CLASS",
                    "tripIds": trip_ids,
                    "passengers": [{ "reductions": ["NONE"] }],
                },
            },
        })
    }
}

/// Reads the address of the GraphQL service from the text of the booking page.
///
/// The page holds its configuration as JSON inside a script, so the quotes are escaped and the
/// entry looks like `\"apolloUrl\":\"https://graphql.www.sbb.ch/\"`.
pub fn service_address(page: &str) -> Option<String> {
    let after = page.split_once("apolloUrl")?.1;
    let start = after.find("https://")?;
    let rest = &after[start..];
    let end = rest.find(['\\', '"'])?;
    Some(rest[..end].to_owned())
}

/// A ride of a trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ride {
    /// The name of the first station.
    pub from: String,
    /// The name of the last station.
    pub to: String,
    /// Departure.
    pub departure: DateTime<FixedOffset>,
    /// Arrival.
    pub arrival: DateTime<FixedOffset>,
    /// Names of the products, such as `IC 6 10061`.
    pub products: Vec<String>,
}

/// A trip that the search found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trip {
    /// The id the price request takes.
    pub id: String,
    /// Whether the site sells a ticket for the trip.
    pub buyable: bool,
    /// The rides of the trip, which leave out walks and other legs.
    pub rides: Vec<Ride>,
}

/// A price for a class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassPrice {
    /// The price in the smallest unit of the currency, which is Rappen.
    pub price: Amount,
    /// `FIRST` or `SECOND`.
    #[serde(rename = "travelClass")]
    pub travel_class: String,
    /// How flexible the ticket is after the sale, such as `FULL_FLEXIBLE`.
    #[serde(rename = "afterSaleFlexibility")]
    pub after_sale_flexibility: String,
}

/// An amount of money.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Amount {
    /// The amount in the smallest unit of the currency.
    pub amount: u32,
    /// The currency, such as `CHF`.
    pub currency: String,
}

/// The prices of a trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TripPrices {
    /// The id of the trip.
    #[serde(rename = "tripId")]
    pub trip_id: String,
    /// A price for each class.
    #[serde(rename = "tripPrices")]
    pub prices: Vec<ClassPrice>,
    /// The system that sells the ticket.
    #[serde(rename = "bookingSystem")]
    pub booking_system: String,
}

/// Returns the `data` of a GraphQL response, or its errors as an error.
fn graphql_data(response: &Value) -> Result<&Value, QuoteError> {
    if let Some(errors) = response.get("errors") {
        return Err(QuoteError::Unexpected(errors.to_string()));
    }
    response
        .get("data")
        .ok_or_else(|| QuoteError::Unexpected("no data in the response".to_owned()))
}

/// Reads the trips from the response to `trips_request`.
///
/// # Errors
///
/// Returns an error when the response holds GraphQL errors or has an unexpected shape.
pub fn parse_trips(response: &Value) -> Result<Vec<Trip>, QuoteError> {
    let unexpected = |what: &str| QuoteError::Unexpected(what.to_owned());
    let trips = graphql_data(response)?
        .pointer("/trips/trips")
        .and_then(Value::as_array)
        .ok_or_else(|| unexpected("no trips list"))?;
    trips
        .iter()
        .map(|trip| {
            let id = trip["id"]
                .as_str()
                .ok_or_else(|| unexpected("trip without id"))?;
            let rides = trip["legs"]
                .as_array()
                .ok_or_else(|| unexpected("trip without legs"))?
                .iter()
                .filter(|leg| leg["__typename"] == "PTRideLeg")
                .map(|leg| {
                    let time = |path: &str| {
                        leg.pointer(path)
                            .and_then(Value::as_str)
                            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
                            .ok_or_else(|| unexpected("a ride without a time"))
                    };
                    let name = |path: &str| {
                        leg.pointer(path)
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                            .ok_or_else(|| unexpected("a ride without a station"))
                    };
                    Ok(Ride {
                        from: name("/start/name")?,
                        to: name("/end/name")?,
                        departure: time("/departure/time")?,
                        arrival: time("/arrival/time")?,
                        products: leg
                            .pointer("/serviceJourney/serviceProducts")
                            .and_then(Value::as_array)
                            .map(|products| {
                                products
                                    .iter()
                                    .filter_map(|product| product["name"].as_str())
                                    .map(str::to_owned)
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                })
                .collect::<Result<Vec<_>, QuoteError>>()?;
            Ok(Trip {
                id: id.to_owned(),
                buyable: trip["isBuyable"].as_bool().unwrap_or(false),
                rides,
            })
        })
        .collect()
}

/// Reads the prices from the response to `prices_request`.
///
/// # Errors
///
/// Returns an error when the response holds GraphQL errors or has an unexpected shape.
pub fn parse_prices(response: &Value) -> Result<Vec<TripPrices>, QuoteError> {
    let prices = graphql_data(response)?
        .get("tripPrices")
        .ok_or_else(|| QuoteError::Unexpected("no tripPrices".to_owned()))?;
    serde_json::from_value(prices.clone())
        .map_err(|error| QuoteError::Unexpected(error.to_string()))
}

/// A trip with its prices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quote {
    /// The trip.
    pub trip: Trip,
    /// Its prices, or `None` when the site gave none.
    pub prices: Option<TripPrices>,
}

/// Fetches quotes from the SBB site, keeps them for a while and spaces its requests out.
pub struct SbbService {
    /// The HTTP client, which keeps cookies between the page and the service.
    client: Client,
    /// Keeps the address of the service and the quotes of searches.
    store: Arc<dyn QuoteStore>,
    /// Spaces requests out and stops them after a failure.
    limiter: RequestLimiter,
}

impl SbbService {
    /// Creates a service that keeps its quotes in a store.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be built.
    pub fn new(store: Arc<dyn QuoteStore>) -> Result<Self, QuoteError> {
        Ok(Self {
            client: Client::builder()
                .user_agent(USER_AGENT)
                .cookie_store(true)
                .timeout(Duration::from_secs(20))
                .build()?,
            store,
            limiter: RequestLimiter::new(MINIMUM_INTERVAL, FAILURE_PAUSE),
        })
    }

    /// Returns the trips of a search with their prices. A search made in the last half hour is
    /// answered from the store without a request.
    ///
    /// # Errors
    ///
    /// Returns an error when requests are paused after a failure, a request fails or is
    /// refused, or a response is not what the requests of the page return. A failure pauses
    /// requests for 15 minutes.
    pub async fn quotes(&self, search: &Search) -> Result<Vec<Quote>, QuoteError> {
        let key = search.cache_key();
        if let Some(quotes) = self
            .store
            .load(&key, QUOTE_LIFETIME)
            .and_then(|document| serde_json::from_str(&document).ok())
        {
            return Ok(quotes);
        }
        let result = crate::with_one_retry(|| self.fetch(search)).await;
        match &result {
            Ok(quotes) => {
                if let Ok(document) = serde_json::to_string(quotes) {
                    self.store.save(&key, &document);
                }
            }
            Err(QuoteError::Paused | QuoteError::BadLink(_)) => {}
            Err(_) => self.limiter.report_failure().await,
        }
        result
    }

    /// Asks the site for the trips of a search and then for their prices.
    async fn fetch(&self, search: &Search) -> Result<Vec<Quote>, QuoteError> {
        let address = self.service_address(&search.booking_link()).await?;
        let trips = parse_trips(&self.post(&address, &search.trips_request()).await?)?;
        let ids: Vec<String> = trips.iter().map(|trip| trip.id.clone()).collect();
        let prices = parse_prices(
            &self
                .post_now(&address, &search.prices_request(Uuid::now_v7(), &ids))
                .await?,
        )?;
        Ok(trips
            .into_iter()
            .map(|trip| {
                let found = prices
                    .iter()
                    .find(|price| price.trip_id == trip.id)
                    .cloned();
                Quote {
                    trip,
                    prices: found,
                }
            })
            .collect())
    }

    /// Returns the address of the price service, from the store or else from the booking page.
    async fn service_address(&self, link: &str) -> Result<String, QuoteError> {
        if let Some(address) = self.store.load("sbb:service-address", ADDRESS_LIFETIME) {
            return Ok(address);
        }
        if !self.limiter.wait_for_turn().await {
            return Err(QuoteError::Paused);
        }
        let page = self.client.get(link).send().await?;
        if !page.status().is_success() {
            return Err(QuoteError::Refused {
                url: link.to_owned(),
                status: page.status().as_u16(),
            });
        }
        let address = service_address(&page.text().await?).ok_or(QuoteError::NoServiceAddress)?;
        self.store.save("sbb:service-address", &address);
        Ok(address)
    }

    /// Waits for the turn of a new search and posts its first GraphQL request.
    async fn post(&self, address: &str, body: &Value) -> Result<Value, QuoteError> {
        if !self.limiter.wait_for_turn().await {
            return Err(QuoteError::Paused);
        }
        self.post_now(address, body).await
    }

    /// Posts a GraphQL request at once and returns the JSON answer. The request for the prices
    /// of a search goes out this way, right after its trips arrive, as on the site itself. The
    /// spacing applies between searches, not within one.
    async fn post_now(&self, address: &str, body: &Value) -> Result<Value, QuoteError> {
        let response = self
            .client
            .post(address)
            .header("accept", "application/graphql-response+json")
            .header("apollographql-client-name", CLIENT_NAME)
            .header("apollographql-client-version", CLIENT_VERSION)
            .json(body)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(QuoteError::Refused {
                url: address.to_owned(),
                status: response.status().as_u16(),
            });
        }
        Ok(response.json().await?)
    }
}

/// Returns the quote of the trip that starts with a ride at the given departure, in seconds since
/// the Unix epoch, and has the given number of rides.
///
/// SBB can count the rides of a trip differently from the timetable, such as a regional train
/// that changes its number on the way and counts as two rides, or a change the plan makes that
/// SBB rides through. When no trip has the planned number of rides, the only trip that leaves at
/// the planned moment stands in for it, because it goes between the same stations at the same
/// time.
pub fn find_trip(
    quotes: &[Quote],
    first_departure_unix_seconds: i64,
    ride_count: usize,
) -> Option<&Quote> {
    let leaves_then = |quote: &&Quote| {
        quote
            .trip
            .rides
            .first()
            .is_some_and(|ride| ride.departure.timestamp() == first_departure_unix_seconds)
    };
    if let Some(exact) = quotes
        .iter()
        .filter(leaves_then)
        .find(|quote| quote.trip.rides.len() == ride_count)
    {
        return Some(exact);
    }
    let mut leaving = quotes.iter().filter(leaves_then);
    match (leaving.next(), leaving.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "https://www.sbb.ch/en?stops=Basel+SBB_I8500010~Bern_I8507000&day=2026-10-16&time=06_30&moment=dep&reduction=none";

    #[test]
    fn a_link_gives_its_search() {
        let search = Search::from_link(LINK).unwrap();
        assert_eq!(search.origin.name, "Basel SBB");
        assert_eq!(search.origin.id, "8500010");
        assert_eq!(search.destination.id, "8507000");
        assert_eq!(search.date.to_string(), "2026-10-16");
        assert_eq!(search.time.to_string(), "06:30:00");
        assert_eq!(search.moment, Moment::Departure);
    }

    #[test]
    fn names_with_umlauts_round_trip_through_a_link() {
        let search = Search::from_link(
            "https://www.sbb.ch/en?stops=Z%C3%BCrich+HB_I8503000~Bern_I8507000&day=2026-10-16&time=06_30&moment=dep&reduction=none",
        )
        .unwrap();
        assert_eq!(search.origin.name, "Zürich HB");
        assert_eq!(Search::from_link(&search.booking_link()).unwrap(), search);
    }

    #[test]
    fn a_link_with_a_reduction_is_not_supported() {
        let link = LINK.replace("reduction=none", "reduction=halbtax");
        assert!(matches!(
            Search::from_link(&link),
            Err(QuoteError::BadLink(_))
        ));
    }

    #[test]
    fn the_trips_request_has_the_shape_of_the_pages() {
        let request = Search::from_link(LINK).unwrap().trips_request();
        let input = &request["variables"]["input"];
        assert_eq!(input["places"][0]["value"], "8500010");
        assert_eq!(input["time"]["time"], "06:30");
        assert_eq!(input["time"]["type"], "DEPARTURE");
    }

    #[test]
    fn the_prices_request_names_the_trips() {
        let request = Search::from_link(LINK)
            .unwrap()
            .prices_request(Uuid::nil(), &["a".to_owned(), "b".to_owned()]);
        let input = &request["variables"]["input"];
        assert_eq!(input["fromPlace"], "8500010");
        assert_eq!(input["tripIds"], json!(["a", "b"]));
        assert_eq!(input["passengers"][0]["reductions"][0], "NONE");
    }

    #[test]
    fn the_service_address_comes_from_the_escaped_page_config() {
        let page = include_str!("../tests/fixtures/sbb_page_config_excerpt.txt");
        assert_eq!(
            service_address(page).as_deref(),
            Some("https://graphql.www.sbb.ch/")
        );
        assert_eq!(service_address("nothing here"), None);
    }

    #[test]
    fn recorded_responses_give_trips_with_prices() {
        let trips: Value =
            serde_json::from_str(include_str!("../tests/fixtures/sbb_trips.json")).unwrap();
        let prices: Value =
            serde_json::from_str(include_str!("../tests/fixtures/sbb_trip_prices.json")).unwrap();
        let trips = parse_trips(&trips).unwrap();
        let prices = parse_prices(&prices).unwrap();
        assert_eq!(trips.len(), 5);
        let first = &trips[0];
        assert_eq!(first.rides[0].from, "Basel SBB");
        assert_eq!(first.rides[0].to, "Bern");
        assert_eq!(
            first.rides[0].departure.to_rfc3339(),
            "2026-10-16T06:30:00+02:00"
        );
        assert_eq!(first.rides[0].products, ["IC 6 10061"]);
        let price = prices
            .iter()
            .find(|price| price.trip_id == first.id)
            .unwrap();
        let second_class = price
            .prices
            .iter()
            .find(|class| class.travel_class == "SECOND")
            .unwrap();
        assert_eq!(second_class.price.amount, 4200);
        assert_eq!(second_class.price.currency, "CHF");
        assert_eq!(price.booking_system, "NOVA");
        // The fourth trip is the cheapest, with a change of train.
        let cheapest = prices
            .iter()
            .flat_map(|price| &price.prices)
            .filter(|class| class.travel_class == "SECOND")
            .map(|class| class.price.amount)
            .min();
        assert_eq!(cheapest, Some(2500));
    }

    #[test]
    fn a_trip_is_found_by_its_first_departure_and_ride_count() {
        let trips: Value =
            serde_json::from_str(include_str!("../tests/fixtures/sbb_trips.json")).unwrap();
        let prices: Value =
            serde_json::from_str(include_str!("../tests/fixtures/sbb_trip_prices.json")).unwrap();
        let prices = parse_prices(&prices).unwrap();
        let quotes: Vec<Quote> = parse_trips(&trips)
            .unwrap()
            .into_iter()
            .map(|trip| Quote {
                prices: prices
                    .iter()
                    .find(|price| price.trip_id == trip.id)
                    .cloned(),
                trip,
            })
            .collect();
        // 06:58 in Basel on 16 October 2026 is 04:58 UTC.
        let departure = DateTime::parse_from_rfc3339("2026-10-16T06:58:00+02:00")
            .unwrap()
            .timestamp();
        let found = find_trip(&quotes, departure, 1).unwrap();
        assert_eq!(found.trip.rides[0].products, ["IC 6 959"]);
        // The only trip at 06:58 stands in for a plan that counts its rides differently.
        let counted_differently = find_trip(&quotes, departure, 2).unwrap();
        assert_eq!(counted_differently.trip.id, found.trip.id);
        assert!(find_trip(&quotes, departure + 60, 1).is_none());
    }

    #[test]
    fn graphql_errors_become_errors() {
        let response = json!({ "errors": [{ "message": "no" }] });
        assert!(matches!(
            parse_prices(&response),
            Err(QuoteError::Unexpected(_))
        ));
    }
}
