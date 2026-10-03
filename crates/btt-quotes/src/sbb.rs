//! Prices from the SBB booking site.
//!
//! The site's page is a shell. Its configuration names the GraphQL service that the page then
//! asks for the trips of a search and, with the ids of those trips, for their prices. This
//! module follows the same two steps from a booking link such as
//! `https://www.sbb.ch/en?stops=Basel+SBB_I8500010~Bern_I8507000&day=2026-10-16&time=06_30&moment=dep&reduction=none`.

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::QuoteError;

/// What the client says about itself.
const USER_AGENT: &str = concat!(
    "BigTrainTrip/",
    env!("CARGO_PKG_VERSION"),
    " (open-source train trip planner)"
);

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
                name: name.replace('+', " "),
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trip {
    /// The id the price request takes.
    pub id: String,
    /// Whether the site sells a ticket for the trip.
    pub buyable: bool,
    /// The rides of the trip, which leave out walks and other legs.
    pub rides: Vec<Ride>,
}

/// A price for a class.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Amount {
    /// The amount in the smallest unit of the currency.
    pub amount: u32,
    /// The currency, such as `CHF`.
    pub currency: String,
}

/// The prices of a trip.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    /// The trip.
    pub trip: Trip,
    /// Its prices, or `None` when the site gave none.
    pub prices: Option<TripPrices>,
}

/// Fetches quotes from the SBB site.
pub struct SbbClient {
    /// The HTTP client, which keeps cookies between the page and the service.
    client: Client,
}

impl SbbClient {
    /// Creates a client that says what it is.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be built.
    pub fn new() -> Result<Self, QuoteError> {
        Ok(Self {
            client: Client::builder()
                .user_agent(USER_AGENT)
                .cookie_store(true)
                .build()?,
        })
    }

    /// Opens the booking link, finds the price service in its page, and asks it for the trips of
    /// the search and then for their prices.
    ///
    /// # Errors
    ///
    /// Returns an error when the link is not supported, a request fails or is refused, or a
    /// response is not what the page's own requests return.
    pub async fn quote(&self, link: &str) -> Result<Vec<Quote>, QuoteError> {
        let search = Search::from_link(link)?;
        let page = self.client.get(link).send().await?;
        if !page.status().is_success() {
            return Err(QuoteError::Refused {
                url: link.to_owned(),
                status: page.status().as_u16(),
            });
        }
        let address = service_address(&page.text().await?).ok_or(QuoteError::NoServiceAddress)?;
        let trips = parse_trips(&self.post(&address, &search.trips_request()).await?)?;
        let ids: Vec<String> = trips.iter().map(|trip| trip.id.clone()).collect();
        let prices = parse_prices(
            &self
                .post(&address, &search.prices_request(Uuid::now_v7(), &ids))
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

    /// Posts a GraphQL request and returns the JSON answer.
    async fn post(&self, address: &str, body: &Value) -> Result<Value, QuoteError> {
        let response = self
            .client
            .post(address)
            .header("accept", "application/graphql-response+json")
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
    fn graphql_errors_become_errors() {
        let response = json!({ "errors": [{ "message": "no" }] });
        assert!(matches!(
            parse_prices(&response),
            Err(QuoteError::Unexpected(_))
        ));
    }
}
