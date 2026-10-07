//! Prices from DSB, dsb.dk, for trips inside Denmark.
//!
//! The search of the site's domestic journey planner answers plain requests that carry the
//! site's own `Origin`. One request takes the UIC numbers of two stations and a departure time,
//! and returns the next trips with every offer for them.

use reqwest::Client;
use serde_json::{Value, json};

use crate::QuoteError;
use crate::site::{
    ConnectionSearch, ConnectionSource, SiteConnection, check_status, local_time, lowest,
};

/// The address of the search.
const SEARCH_URL: &str = "https://www.dsb.dk/api/domestic/v1/search/";

/// The origin of the site's own pages, which the search requires.
const ORIGIN: &str = "https://www.dsb.dk";

/// DSB, read with plain requests.
#[derive(Debug, Default)]
pub struct Dsb;

/// Builds the search that the site's page sends, for trips from the planned departure on.
pub fn request(search: &ConnectionSearch) -> Value {
    let location = |code: &str| {
        let number: u64 = code.parse().unwrap_or_default();
        json!({
            "name": code,
            "type": "STOP",
            "id": format!("A=1@L={code}@"),
            "transports": [],
            "zones": [],
            "stopId": number,
        })
    };
    json!({
        "offerCollection": {
            "anonymousPassengerSpecifications": [
                { "travelerType": "TT_ADULT", "externalRef": "passenger_1" }
            ],
            "offerSearchCriteria": {
                "requestedOfferParts": ["ADMISSION"],
                "requestedOfferTypes": ["OT_ORANGE", "OT_ORANGEFRI", "OT_SINGLE"],
                "customerGroups": ["B2C"],
            },
        },
        "tripSearch": {
            "strategy": {
                "departureLocation": location(&search.origin),
                "arrivalLocation": location(&search.destination),
                "departureTime": search
                    .departure_moment
                    .format("%Y-%m-%dT%H:%M:00.000Z")
                    .to_string(),
                "searchStrategy": "STRATEGY_DEPARTURE_TIME",
                "tripsBefore": 0,
                "tripsAfter": 3,
            },
            "meansOfTransportationClasses": [
                "INTERCITY_TRAIN", "INTERCITY_EXPRESS_TRAIN", "REGIONAL_TRAIN", "OTHER_TRAINS",
                "S_TRAIN",
            ],
            "flags": {
                "polyline": false,
                "polylineBeforeAfter": false,
                "realTimeInformation": false,
                "turnByTurnNavigation": false,
            },
        },
    })
}

/// Reads the trips and the lowest price of each class from an answer of the search.
///
/// # Errors
///
/// Returns an error when the answer has no list of trips.
pub fn parse_connections(response: &Value) -> Result<Vec<SiteConnection>, QuoteError> {
    let trips = response
        .pointer("/value/tripOffers")
        .and_then(Value::as_array)
        .ok_or_else(|| QuoteError::Unexpected("no trips in the answer".to_owned()))?;
    Ok(trips
        .iter()
        .filter_map(|trip| {
            let legs: Vec<&Value> = trip
                .pointer("/trip/tripLegs")?
                .as_array()?
                .iter()
                .filter(|leg| leg.get("type").and_then(Value::as_str) == Some("PUBLIC_TRANSPORT"))
                .collect();
            let departure = legs
                .first()?
                .pointer("/stops/0/plannedTimeAndTrackInfo/departureTime")?
                .as_str()
                .and_then(local_time)?;
            let trains = legs
                .iter()
                .filter_map(|leg| {
                    leg.pointer("/transports/0/name")?
                        .as_str()
                        .map(str::to_owned)
                })
                .collect();
            let offers: Vec<&Value> = trip
                .get("offers")
                .and_then(Value::as_array)
                .map(|offers| {
                    offers
                        .iter()
                        .filter(|offer| {
                            offer.pointer("/status/category").and_then(Value::as_str)
                                == Some("SELLABLE")
                        })
                        .collect()
                })
                .unwrap_or_default();
            let price_of = |offer: &Value| {
                let amount = offer
                    .pointer("/offerSummary/minimalPrice/amount")?
                    .as_f64()?;
                let scale = offer
                    .pointer("/offerSummary/minimalPrice/scale")?
                    .as_i64()?;
                Some(amount / 10_f64.powi(i32::try_from(scale).ok()?))
            };
            let class_price = |standard: bool| {
                lowest(offers.iter().filter_map(|offer| {
                    let class = offer
                        .pointer("/offerSummary/overallServiceClass/type")
                        .and_then(Value::as_str);
                    ((class == Some("STANDARD")) == standard)
                        .then(|| price_of(offer))
                        .flatten()
                }))
            };
            Some(SiteConnection {
                departure,
                trains,
                second_class_price: class_price(true),
                first_class_price: class_price(false),
                currency: "DKK".to_owned(),
                not_for_sale: offers.is_empty(),
            })
        })
        .collect())
}

impl ConnectionSource for Dsb {
    const NAME: &'static str = "dsb";

    async fn fetch(
        &self,
        client: &Client,
        search: &ConnectionSearch,
    ) -> Result<Vec<SiteConnection>, QuoteError> {
        let response = client
            .get(SEARCH_URL)
            .query(&[("input", request(search).to_string())])
            .header("accept", "application/json")
            .header("origin", ORIGIN)
            .send()
            .await?;
        parse_connections(&check_status(response, SEARCH_URL)?.json().await?)
    }
}
