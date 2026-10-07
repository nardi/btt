//! Prices from Eurostar, eurostar.com.
//!
//! The site's GraphQL gateway answers plain requests. A search takes the UIC numbers of two
//! stations, such as 7015400 for London St Pancras, and a date, and returns the trains of the
//! day with the fares of each class of service.

use reqwest::Client;
use serde_json::{Value, json};

use crate::QuoteError;
use crate::site::{ConnectionSearch, ConnectionSource, SiteConnection, check_status, lowest};

/// The address of the gateway.
const GATEWAY_URL: &str = "https://site-api.eurostar.com/gateway";

/// The search query of the site's pages.
const SEARCH_QUERY: &str = include_str!("eurostar_search.graphql");

/// Eurostar, read with plain requests.
#[derive(Debug, Default)]
pub struct Eurostar;

/// Returns the link of a search on the site for two stations and a date.
pub fn search_link(origin: &str, destination: &str, date: chrono::NaiveDate) -> String {
    format!(
        "https://www.eurostar.com/search/uk-en?adult=1&origin={origin}&destination={destination}\
         &outbound={date}"
    )
}

/// Builds the search that the site's page sends.
pub fn request(search: &ConnectionSearch) -> Value {
    json!({
        "operationName": "NewBookingSearch",
        "query": SEARCH_QUERY,
        "variables": {
            "productFamilies": ["PUB"],
            "contractCode": "EIL_ALL",
            "isAftersales": false,
            "hasInbound": false,
            "multipleFlexibility": true,
            "showAllSummatedFares": false,
            "prioritiseShortHaulODTrains": true,
            "hideExternalCarrierTrains": true,
            "hideDirectExternalCarrierTrains": true,
            "origin": search.origin,
            "destination": search.destination,
            "currency": "EUR",
            "outbound": search.departure.format("%Y-%m-%d").to_string(),
            "adult": 1,
            "child": 0,
            "senior": 0,
            "infant": 0,
            "youth": 0,
            "filteredClassesOfService": [
                "STD", "SP", "BP", "ER_STANDARD", "COMFORT", "PREMIUM", "STANDARD", "PLUS",
                "PREMIER",
            ],
            "seniorsAges": [],
            "childAges": [],
            "youthAges": [],
        },
    })
}

/// Reads the trains of the day and the lowest fare of each class from an answer. Eurostar
/// Standard counts as the second class and Eurostar Plus as the first.
///
/// # Errors
///
/// Returns an error when the answer has no list of journeys.
pub fn parse_connections(response: &Value) -> Result<Vec<SiteConnection>, QuoteError> {
    let journeys = response
        .pointer("/data/journeySearch/outbound/journeys")
        .and_then(Value::as_array)
        .ok_or_else(|| QuoteError::Unexpected("no journeys in the answer".to_owned()))?;
    Ok(journeys
        .iter()
        .filter_map(|journey| {
            let date = journey.pointer("/timing/date")?.as_str()?;
            let time = journey.pointer("/timing/departureTime")?.as_str()?;
            let departure =
                chrono::NaiveDateTime::parse_from_str(&format!("{date} {time}"), "%Y-%m-%d %H:%M")
                    .ok()?;
            let fares = journey.get("fares").and_then(Value::as_array);
            let trains = fares
                .and_then(|fares| fares.first())
                .and_then(|fare| fare.get("legs")?.as_array())
                .map(|legs| {
                    legs.iter()
                        .filter_map(|leg| leg.get("serviceName")?.as_str())
                        .map(|number| format!("Eurostar {number}"))
                        .collect()
                })
                .unwrap_or_default();
            let class_price = |classes: &[&str]| {
                lowest(fares.into_iter().flatten().filter_map(|fare| {
                    let class = fare.pointer("/classOfService/code")?.as_str()?;
                    let seats = fare.get("seats").and_then(Value::as_u64).unwrap_or(0);
                    (classes.contains(&class) && seats > 0)
                        .then(|| fare.pointer("/prices/displayPrice")?.as_f64())
                        .flatten()
                }))
            };
            let second_class_price = class_price(&["STANDARD", "ER_STANDARD", "STD"]);
            let first_class_price = class_price(&["PLUS", "COMFORT"]);
            Some(SiteConnection {
                departure,
                trains,
                not_for_sale: second_class_price.is_none() && first_class_price.is_none(),
                second_class_price,
                first_class_price,
                currency: "EUR".to_owned(),
            })
        })
        .collect())
}

impl ConnectionSource for Eurostar {
    const NAME: &'static str = "eurostar";

    async fn fetch(
        &self,
        client: &Client,
        search: &ConnectionSearch,
    ) -> Result<Vec<SiteConnection>, QuoteError> {
        let response = client
            .post(GATEWAY_URL)
            .header("accept", "application/json")
            .header("accept-language", "en-GB")
            .header("x-market-code", "uk")
            .header("x-platform", "web")
            .header("origin", "https://www.eurostar.com")
            .json(&request(search))
            .send()
            .await?;
        parse_connections(&check_status(response, GATEWAY_URL)?.json().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journeys_take_the_lowest_fare_with_seats_of_each_class() {
        let fare = |class: &str, price: f64, seats: u64| {
            json!({ "classOfService": { "code": class }, "seats": seats,
                    "prices": { "displayPrice": price },
                    "legs": [{ "serviceName": "9080" }] })
        };
        let response = json!({ "data": { "journeySearch": { "outbound": { "journeys": [{
            "timing": { "date": "2026-11-20", "departureTime": "06:01" },
            "fares": [fare("STANDARD", 151.0, 63), fare("STANDARD", 79.0, 0),
                      fare("PLUS", 165.0, 3), fare("PREMIER", 379.0, 43)]
        }] } } } });
        let connections = parse_connections(&response).unwrap();
        assert_eq!(connections[0].trains, ["Eurostar 9080"]);
        assert_eq!(connections[0].second_class_price, Some(151.0));
        assert_eq!(connections[0].first_class_price, Some(165.0));
    }
}
