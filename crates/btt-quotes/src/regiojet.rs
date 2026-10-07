//! Prices from RegioJet, regiojet.com.
//!
//! RegioJet has an open API that answers plain requests. A search takes RegioJet's station ids
//! and a date, and returns the connections of the day with the lowest and highest price of
//! each. The simple search does not name the trains, so a connection counts its trains from its
//! number of changes.

use reqwest::Client;
use serde_json::Value;

use crate::QuoteError;
use crate::site::{ConnectionSearch, ConnectionSource, SiteConnection, check_status, local_time};

/// The address of the connection search.
const SEARCH_URL: &str = "https://brn-ybus-pubapi.sa.cz/restapi/routes/search/simple";

/// RegioJet, read with plain requests.
#[derive(Debug, Default)]
pub struct Regiojet;

/// Reads the connections and their lowest price from an answer of the search. RegioJet's lowest
/// price is that of its cheapest class, which is the price for a second class ticket.
///
/// # Errors
///
/// Returns an error when the answer has no list of connections.
pub fn parse_connections(response: &Value) -> Result<Vec<SiteConnection>, QuoteError> {
    let routes = response
        .get("routes")
        .and_then(Value::as_array)
        .ok_or_else(|| QuoteError::Unexpected("no connections in the answer".to_owned()))?;
    Ok(routes
        .iter()
        .filter_map(|route| {
            let departure = route.get("departureTime")?.as_str().and_then(local_time)?;
            let changes = route
                .get("transfersCount")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let train_count = usize::try_from(changes).ok()? + 1;
            let bookable = route
                .get("bookable")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let seats = route
                .get("freeSeatsCount")
                .and_then(Value::as_u64)
                .unwrap_or(1);
            Some(SiteConnection {
                departure,
                trains: vec!["RegioJet".to_owned(); train_count],
                second_class_price: route.get("priceFrom").and_then(Value::as_f64),
                first_class_price: None,
                currency: "EUR".to_owned(),
                not_for_sale: !bookable || seats == 0,
            })
        })
        .collect())
}

impl ConnectionSource for Regiojet {
    const NAME: &'static str = "regiojet";

    async fn fetch(
        &self,
        client: &Client,
        search: &ConnectionSearch,
    ) -> Result<Vec<SiteConnection>, QuoteError> {
        let date = search.departure.format("%Y-%m-%d").to_string();
        let response = client
            .get(SEARCH_URL)
            .query(&[
                ("fromLocationId", search.origin.as_str()),
                ("fromLocationType", "STATION"),
                ("toLocationId", search.destination.as_str()),
                ("toLocationType", "STATION"),
                ("departureDate", date.as_str()),
                ("tariffs", "REGULAR"),
            ])
            .header("accept", "application/json")
            .header("x-currency", "EUR")
            .header("x-lang", "en")
            .send()
            .await?;
        parse_connections(&check_status(response, SEARCH_URL)?.json().await?)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn connections_count_their_trains_from_their_changes() {
        let response = json!({ "routes": [
            { "departureTime": "2026-11-20T06:05:00.000+01:00", "transfersCount": 0,
              "priceFrom": 8.4, "priceTo": 14.7, "bookable": true, "freeSeatsCount": 120 },
            { "departureTime": "2026-11-20T07:05:00.000+01:00", "transfersCount": 1,
              "priceFrom": 11.0, "bookable": true, "freeSeatsCount": 0 }
        ] });
        let connections = parse_connections(&response).unwrap();
        assert_eq!(connections[0].trains.len(), 1);
        assert_eq!(connections[0].second_class_price, Some(8.4));
        assert!(!connections[0].not_for_sale);
        assert_eq!(connections[1].trains.len(), 2);
        assert!(connections[1].not_for_sale);
    }
}
