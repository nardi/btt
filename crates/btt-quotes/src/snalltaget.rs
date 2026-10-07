//! Prices from Snälltåget, snalltaget.se.
//!
//! The site's service answers plain requests with a token that the site hands out to anyone
//! for half an hour. A search takes the site's station codes, such as 740000001 for Stockholm C
//! or `Berlin` for Berlin Hbf, and a date, and returns the routes of the day with their bundles
//! of products and prices.

use std::time::{Duration, Instant};

use reqwest::Client;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::QuoteError;
use crate::site::{
    ConnectionSearch, ConnectionSource, SiteConnection, check_status, local_time, lowest,
};

/// The address that hands out tokens.
const TOKEN_URL: &str = "https://www.snalltaget.se/token/v2";

/// The address of the journey search.
const SEARCH_URL: &str = "https://apiv2.snalltaget.se/orientation/searchjourney";

/// How long before its end a token is replaced.
const TOKEN_MARGIN: Duration = Duration::from_mins(2);

/// Snälltåget, read with plain requests.
#[derive(Debug, Default)]
pub struct Snalltaget {
    /// The token of the service and when it ends.
    token: Mutex<Option<(String, Instant)>>,
}

impl Snalltaget {
    /// Returns a token, fetching a new one when there is none or it ends soon.
    async fn token(&self, client: &Client) -> Result<String, QuoteError> {
        let mut token = self.token.lock().await;
        if let Some((value, ends)) = token.as_ref()
            && Instant::now() + TOKEN_MARGIN < *ends
        {
            return Ok(value.clone());
        }
        let response = client
            .get(TOKEN_URL)
            .header("accept", "application/json")
            .send()
            .await?;
        let answer: Value = check_status(response, TOKEN_URL)?.json().await?;
        let value = answer
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| QuoteError::Unexpected("no token in the answer".to_owned()))?
            .to_owned();
        let lifetime = answer
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(1800);
        *token = Some((
            value.clone(),
            Instant::now() + Duration::from_secs(lifetime),
        ));
        Ok(value)
    }
}

/// Reads the routes and the lowest price of each class from an answer of the search.
///
/// # Errors
///
/// Returns an error when the answer has no list of routes.
pub fn parse_connections(response: &Value) -> Result<Vec<SiteConnection>, QuoteError> {
    let routes = response
        .pointer("/offer/travels/0/routes")
        .and_then(Value::as_array)
        .ok_or_else(|| QuoteError::Unexpected("no routes in the answer".to_owned()))?;
    let class_of_family = |family_id: &str| {
        response
            .pointer("/offer/productFamilies")?
            .as_array()?
            .iter()
            .find(|family| family.get("id").and_then(Value::as_str) == Some(family_id))?
            .get("travelClass")?
            .as_str()
    };
    Ok(routes
        .iter()
        .filter_map(|route| {
            let legs = route.get("legs")?.as_array()?;
            let departure = legs
                .first()?
                .pointer("/departureStation/departureTimestamp")?
                .as_str()
                .and_then(local_time)?;
            let trains = legs
                .iter()
                .filter_map(|leg| {
                    let name = leg.pointer("/serviceType/name")?.as_str()?;
                    let number = leg.get("serviceName")?.as_str()?;
                    Some(format!("{name} {number}"))
                })
                .collect();
            let bundles = route.get("bundles").and_then(Value::as_array);
            let class_price = |class: &str| {
                lowest(bundles.into_iter().flatten().filter_map(|bundle| {
                    let family = bundle.get("productFamilyId")?.as_str()?;
                    (class_of_family(family) == Some(class))
                        .then(|| bundle.get("price")?.as_f64())
                        .flatten()
                }))
            };
            let available = route
                .pointer("/bundleAvailability/availability")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            Some(SiteConnection {
                departure,
                trains,
                second_class_price: class_price("SECOND"),
                first_class_price: class_price("FIRST"),
                currency: "SEK".to_owned(),
                not_for_sale: !available,
            })
        })
        .collect())
}

impl ConnectionSource for Snalltaget {
    const NAME: &'static str = "snalltaget";

    async fn fetch(
        &self,
        client: &Client,
        search: &ConnectionSearch,
    ) -> Result<Vec<SiteConnection>, QuoteError> {
        let token = self.token(client).await?;
        let body = json!({
            "origin": search.origin,
            "destination": search.destination,
            "departure": search.departure.format("%Y-%m-%d").to_string(),
            "oppositedate": null,
            "passengers": [{ "type": "AD" }],
            "travelWithpet": false,
        });
        let response = client
            .post(SEARCH_URL)
            .bearer_auth(token)
            .header("accept", "application/json")
            .header("accept-language", "en")
            .header("origin", "https://www.snalltaget.se")
            .json(&body)
            .send()
            .await?;
        parse_connections(&check_status(response, SEARCH_URL)?.json().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_take_the_lowest_bundle_of_each_class() {
        let response = json!({ "offer": {
            "productFamilies": [
                { "id": "SPNF", "travelClass": "SECOND" },
                { "id": "FCSFF", "travelClass": "FIRST" }
            ],
            "travels": [{ "routes": [{
                "legs": [
                    { "serviceType": { "name": "Snälltåget" }, "serviceName": "3943",
                      "departureStation": { "departureTimestamp": "2026-11-19T16:25:00" } },
                    { "serviceType": { "name": "Snälltåget" }, "serviceName": "301",
                      "departureStation": { "departureTimestamp": "2026-11-19T21:55:00" } }
                ],
                "bundles": [
                    { "productFamilyId": "SPNF", "price": 698 },
                    { "productFamilyId": "SPNF", "price": 1346 },
                    { "productFamilyId": "FCSFF", "price": 1890 }
                ],
                "bundleAvailability": { "availability": true }
            }] }]
        } });
        let connections = parse_connections(&response).unwrap();
        assert_eq!(connections[0].trains, ["Snälltåget 3943", "Snälltåget 301"]);
        assert_eq!(connections[0].second_class_price, Some(698.0));
        assert_eq!(connections[0].first_class_price, Some(1890.0));
    }
}
