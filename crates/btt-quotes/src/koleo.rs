//! Prices from Koleo, koleo.pl, which sells the trains of PKP Intercity and of most Polish
//! regional operators.
//!
//! Koleo's web page reads an API that answers plain requests with the headers of the page. A
//! station is found by its slug, such as `warszawa-centralna`, which gives the numeric id the
//! search takes. The search returns the connections from a moment on, without prices, and the
//! price of one connection is a request of its own. So only the planned connection is priced.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use chrono::Timelike;
use reqwest::{Client, RequestBuilder};
use serde_json::{Value, json};

use crate::QuoteError;
use crate::site::{ConnectionSearch, ConnectionSource, SiteConnection, check_status, local_time};

/// The address of the API.
const API_URL: &str = "https://api.koleo.pl/v2/main";

/// Koleo, read with plain requests.
#[derive(Debug, Default)]
pub struct Koleo {
    /// The numeric id of each station slug the app has looked up.
    station_ids: Mutex<HashMap<String, u64>>,
}

/// Adds the headers that Koleo's web page sends.
fn with_koleo_headers(request: RequestBuilder) -> RequestBuilder {
    request
        .header("accept", "application/json")
        .header("x-koleo-version", "2")
        .header("x-koleo-client", "Nuxt-1")
        .header("accept-eol-response-version", "1")
}

/// Returns the name of a train brand of Koleo by its id, for the train names of a connection.
fn brand_name(id: u64) -> &'static str {
    match id {
        1 => "TLK",
        2 => "EIC",
        3 => "REG",
        4 => "IR",
        5 => "RE",
        28 => "IC",
        29 => "EIP",
        47 | 52 => "LE",
        65 => "RJ",
        _ => "Train",
    }
}

/// Reads the connections of a search, without prices, with the id of each for its price.
///
/// # Errors
///
/// Returns an error when the answer is not a list of connections.
pub fn parse_connections(response: &Value) -> Result<Vec<(String, SiteConnection)>, QuoteError> {
    let connections = response
        .as_array()
        .ok_or_else(|| QuoteError::Unexpected("no list of connections in the answer".to_owned()))?;
    Ok(connections
        .iter()
        .filter_map(|connection| {
            let id = connection.get("uuid")?.as_str()?.to_owned();
            // Koleo gives some departures to the half minute, such as 15:43:30, and the
            // timetable to the minute.
            let departure = connection
                .get("departure")?
                .as_str()
                .and_then(local_time)?
                .with_second(0)?;
            let trains = connection
                .get("legs")?
                .as_array()?
                .iter()
                .filter(|leg| leg.get("train_nr").is_some())
                .map(|leg| {
                    let brand = leg
                        .get("commercial_brand_id")
                        .and_then(Value::as_u64)
                        .map_or("Train", brand_name);
                    let number = leg.get("train_nr").and_then(Value::as_u64).unwrap_or(0);
                    format!("{brand} {number}")
                })
                .collect();
            Some((
                id,
                SiteConnection {
                    departure,
                    trains,
                    second_class_price: None,
                    first_class_price: None,
                    currency: "PLN".to_owned(),
                    not_for_sale: false,
                },
            ))
        })
        .collect())
}

/// Reads the price in zloty for one adult on the normal fare from the answer of a price request,
/// or `None` when Koleo does not sell the connection.
pub fn parse_price(response: &Value) -> Option<f64> {
    let purchasable = response
        .get("purchasable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let price = response.get("price")?.as_str()?.parse().ok()?;
    purchasable.then_some(price)
}

impl Koleo {
    /// Returns the numeric id of a station slug, from what the app has looked up before or else
    /// from Koleo.
    async fn station_id(&self, client: &Client, slug: &str) -> Result<u64, QuoteError> {
        let known = self
            .station_ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(slug)
            .copied();
        if let Some(id) = known {
            return Ok(id);
        }
        let url = format!("{API_URL}/stations/by_slug/{slug}");
        let response = with_koleo_headers(client.get(&url)).send().await?;
        let station: Value = check_status(response, &url)?.json().await?;
        let id = station
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| QuoteError::Unexpected(format!("no id for the station {slug}")))?;
        self.station_ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(slug.to_owned(), id);
        Ok(id)
    }
}

impl ConnectionSource for Koleo {
    const NAME: &'static str = "koleo";

    async fn fetch(
        &self,
        client: &Client,
        search: &ConnectionSearch,
    ) -> Result<Vec<SiteConnection>, QuoteError> {
        let origin = self.station_id(client, &search.origin).await?;
        let destination = self.station_id(client, &search.destination).await?;
        let url = format!("{API_URL}/eol_connections/search");
        let body = json!({
            "start_id": origin,
            "end_id": destination,
            "departure_after": search.departure.format("%Y-%m-%dT%H:%M:%S").to_string(),
            // Koleo returns a selection of connections, which can leave out a slow direct train
            // in favour of faster ones with a change. A planned single train asks for direct
            // ones only.
            "only_direct": search.train_count == Some(1),
        });
        let response = with_koleo_headers(client.post(&url))
            .json(&body)
            .send()
            .await?;
        let mut connections = parse_connections(&check_status(response, &url)?.json().await?)?;
        // Only the planned connection gets a price, because each price is a request.
        if let Some((id, connection)) = connections
            .iter_mut()
            .find(|(_, connection)| search.is_planned(connection))
        {
            let url = format!("{API_URL}/eol_connections/{id}/price");
            let response = with_koleo_headers(client.get(&url)).send().await?;
            // Koleo has no price for some connections, such as ones it does not sell yet, and
            // answers those with 404. That is no failure of the site.
            let price = if response.status() == reqwest::StatusCode::NOT_FOUND {
                None
            } else {
                parse_price(&check_status(response, &url)?.json().await?)
            };
            connection.not_for_sale = price.is_none();
            connection.second_class_price = price;
        }
        Ok(connections
            .into_iter()
            .map(|(_, connection)| connection)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn connections_name_their_trains_by_brand_and_number() {
        let response = json!([
            { "uuid": "a", "departure": "2026-10-20T08:08:30+02:00",
              "legs": [{ "train_nr": 5332, "commercial_brand_id": 28 }] },
            { "uuid": "b", "departure": "2026-10-20T08:40:00+02:00",
              "legs": [{ "train_nr": 5300, "commercial_brand_id": 29 },
                       { "leg_type": "walk" },
                       { "train_nr": 91234, "commercial_brand_id": 3 }] }
        ]);
        let connections = parse_connections(&response).unwrap();
        assert_eq!(connections[0].0, "a");
        assert_eq!(connections[0].1.trains, ["IC 5332"]);
        assert_eq!(
            connections[0].1.departure.to_string(),
            "2026-10-20 08:08:00"
        );
        assert_eq!(connections[1].1.trains, ["EIP 5300", "REG 91234"]);
        assert_eq!(connections[1].1.second_class_price, None);
    }

    #[test]
    fn a_price_counts_only_when_koleo_sells_the_connection() {
        let sold = json!({ "price": "70.0", "purchasable": true });
        assert_eq!(parse_price(&sold), Some(70.0));
        let unsold = json!({ "price": "70.0", "purchasable": false });
        assert_eq!(parse_price(&unsold), None);
    }
}
