//! Prices from SJ, sj.se, for trips in Sweden.
//!
//! The booking service of the site answers plain requests with the subscription key of the
//! site's own pages. A search takes the station codes of Samtrafiken, such as 740000001 for
//! Stockholm Central, and a date. It returns the departures of the day, and the offers of one
//! departure take one more request, so only the planned departure is priced.

use reqwest::Client;
use serde_json::{Value, json};

use crate::QuoteError;
use crate::site::{
    ConnectionSearch, ConnectionSource, SiteConnection, check_status, local_time, lowest,
};

/// The address of the booking service.
const SERVICE_URL: &str = "https://prod-api.adp.sj.se/public/sales/booking/v3";

/// The subscription key of the site's own pages.
const SUBSCRIPTION_KEY: &str = "d6625619def348d38be070027fd24ff6";

/// The name the site's pages give themselves.
const CLIENT_NAME: &str = "sjse-booking-client";

/// The version the site's pages give themselves.
const CLIENT_VERSION: &str = "20261006.0081-prod";

/// SJ, read with plain requests.
#[derive(Debug, Default)]
pub struct Sj;

/// Adds the headers of the site's pages to a request.
fn with_site_headers(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request
        .header("accept", "application/json")
        .header("accept-language", "en-GB")
        .header("ocp-apim-subscription-key", SUBSCRIPTION_KEY)
        .header("x-client-name", CLIENT_NAME)
        .header("x-client-version", CLIENT_VERSION)
        .header("origin", "https://www.sj.se")
}

/// Reads the departures of a day from an answer, without prices.
///
/// # Errors
///
/// Returns an error when the answer has no list of departures.
pub fn parse_departures(response: &Value) -> Result<Vec<(String, SiteConnection)>, QuoteError> {
    let departures = response
        .pointer("/travels/0/departures")
        .and_then(Value::as_array)
        .ok_or_else(|| QuoteError::Unexpected("no departures in the answer".to_owned()))?;
    Ok(departures
        .iter()
        .filter_map(|departure| {
            let id = departure.get("departureId")?.as_str()?.to_owned();
            let trains = departure
                .get("legs")?
                .as_array()?
                .iter()
                .filter_map(|leg| {
                    let kind = leg.pointer("/serviceType/name")?.as_str()?;
                    let number = leg.get("publicServiceName")?.as_str()?;
                    Some(format!("{kind} {number}"))
                })
                .collect();
            Some((
                id,
                SiteConnection {
                    departure: local_time(departure.get("departureDateTime")?.as_str()?)?,
                    trains,
                    second_class_price: None,
                    first_class_price: None,
                    currency: "SEK".to_owned(),
                    not_for_sale: false,
                },
            ))
        })
        .collect())
}

/// Adds the lowest available price of each class from the offers of a departure.
pub fn add_offers(connection: &mut SiteConnection, offers: &Value) {
    let mut second = Vec::new();
    let mut first = Vec::new();
    for kind in ["seatOffers", "bedOffers"] {
        let Some(classes) = offers
            .pointer(&format!("/{kind}/offers"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (class, offer) in classes {
            let Some(flexibilities) = offer.get("flexibilities").and_then(Value::as_object) else {
                continue;
            };
            for flexibility in flexibilities.values() {
                if flexibility.get("available").and_then(Value::as_bool) != Some(true) {
                    continue;
                }
                let Some(amount) = flexibility
                    .pointer("/journeyPrices/price/amount")
                    .and_then(Value::as_str)
                    .and_then(|amount| amount.parse::<f64>().ok())
                else {
                    continue;
                };
                if class.starts_with("FIRST") {
                    first.push(amount);
                } else {
                    second.push(amount);
                }
            }
        }
    }
    connection.second_class_price = lowest(second);
    connection.first_class_price = lowest(first);
    connection.not_for_sale =
        connection.second_class_price.is_none() && connection.first_class_price.is_none();
}

impl ConnectionSource for Sj {
    const NAME: &'static str = "sj";

    async fn fetch(
        &self,
        client: &Client,
        search: &ConnectionSearch,
    ) -> Result<Vec<SiteConnection>, QuoteError> {
        let search_url = format!("{SERVICE_URL}/search");
        let body = json!({
            "origin": search.origin,
            "destination": search.destination,
            "departureDate": search.departure.format("%Y-%m-%d").to_string(),
            "passengers": [{ "passengerCategory": { "type": "ADULT" } }],
        });
        let response = with_site_headers(client.post(&search_url).json(&body))
            .send()
            .await?;
        let started: Value = check_status(response, &search_url)?.json().await?;
        let (Some(search_id), Some(passenger_list_id)) = (
            started.get("departureSearchId").and_then(Value::as_str),
            started.get("passengerListId").and_then(Value::as_str),
        ) else {
            return Err(QuoteError::Unexpected(
                "no search id in the answer".to_owned(),
            ));
        };
        let departures_url = format!("{SERVICE_URL}/departures/search/{search_id}");
        let response = with_site_headers(client.get(&departures_url))
            .send()
            .await?;
        let mut departures =
            parse_departures(&check_status(response, &departures_url)?.json().await?)?;
        let planned = departures
            .iter_mut()
            .find(|(_, connection)| search.is_planned(connection));
        if let Some((id, connection)) = planned {
            let offers_url = format!("{SERVICE_URL}/departures/{id}/offers");
            let response = with_site_headers(
                client
                    .get(&offers_url)
                    .query(&[("passengerListId", passenger_list_id)]),
            )
            .send()
            .await?;
            add_offers(
                connection,
                &check_status(response, &offers_url)?.json().await?,
            );
        }
        Ok(departures
            .into_iter()
            .map(|(_, connection)| connection)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_give_the_lowest_available_price_of_each_class() {
        let mut connection = SiteConnection {
            departure: local_time("2026-11-20T06:04:00").unwrap(),
            trains: vec!["SJ Snabbtåg 421".to_owned()],
            second_class_price: None,
            first_class_price: None,
            currency: "SEK".to_owned(),
            not_for_sale: false,
        };
        let price = |amount: &str, available: bool| json!({ "available": available, "journeyPrices": { "price": { "amount": amount } } });
        let offers = json!({ "seatOffers": { "offers": {
            "SECOND": { "flexibilities": {
                "NOFLEX": price("714", true), "FULLFLEX": price("994", true)
            } },
            "SECOND_CALM": { "flexibilities": { "NOFLEX": price("694", false) } },
            "FIRST": { "flexibilities": { "NOFLEX": price("884", true) } }
        } } });
        add_offers(&mut connection, &offers);
        assert_eq!(connection.second_class_price, Some(714.0));
        assert_eq!(connection.first_class_price, Some(884.0));
        assert!(!connection.not_for_sale);
    }
}
