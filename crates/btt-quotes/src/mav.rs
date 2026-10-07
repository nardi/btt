//! Prices from MÁV, jegy.mav.hu, for trips in Hungary.
//!
//! The site's offer service answers plain requests. A search takes the site's station codes,
//! which are the UIC numbers with two leading zeros such as 005510017 for Budapest-Keleti, and a
//! departure time, and returns the routes of the next hours with the price of each class.

use reqwest::Client;
use serde_json::{Value, json};

use crate::QuoteError;
use crate::site::{
    ConnectionSearch, ConnectionSource, SiteConnection, check_status, local_time, lowest,
};

/// The address of the offer search.
const SEARCH_URL: &str = "https://jegy-a.mav.hu/IK_API_PROD/api/OfferRequestApi/GetOfferRequest";

/// The customer type of a full fare adult.
const ADULT: &str = "HU_44_025-065";

/// MÁV, read with plain requests.
#[derive(Debug, Default)]
pub struct Mav;

/// Builds the search that the site's page sends, for routes from the planned departure on.
pub fn request(search: &ConnectionSearch) -> Value {
    // The site takes the local time with its offset from UTC.
    let offset_minutes = (search.departure - search.departure_moment.naive_utc()).num_minutes();
    let offset = format!(
        "{}{:02}:{:02}",
        if offset_minutes < 0 { "-" } else { "+" },
        offset_minutes.abs() / 60,
        offset_minutes.abs() % 60
    );
    let moment = |minutes: i64| {
        let local = search.departure + chrono::Duration::minutes(minutes);
        format!("{}{offset}", local.format("%Y-%m-%dT%H:%M:00"))
    };
    json!({
        "offerkind": "1",
        "startStationCode": search.origin,
        "innerStationsCodes": [],
        "endStationCode": search.destination,
        "modalities": [100, 109],
        "passangers": [{
            "passengerCount": 1,
            "passengerId": 0,
            "customerTypeKey": ADULT,
            "customerDiscountsKeys": [],
        }],
        "isOneWayTicket": true,
        "isTravelEndTime": false,
        "isSupplementaryTicketsOnly": false,
        "hasHungaryPass": false,
        "travelStartDate": moment(0),
        "travelReturnDate": moment(30),
        "selectedServices": [52],
        "selectedSearchServices": [],
        "eszkozSzamok": [],
        "isOfDetailedSearch": false,
        "isFromTimeTable": false,
    })
}

/// Reads the routes and the price of each class from an answer of the search. Parts of a route
/// without a train number, such as local transport, are left out of its trains.
///
/// # Errors
///
/// Returns an error when the answer has no list of routes.
pub fn parse_connections(response: &Value) -> Result<Vec<SiteConnection>, QuoteError> {
    let routes = response
        .get("route")
        .and_then(Value::as_array)
        .ok_or_else(|| QuoteError::Unexpected("no routes in the answer".to_owned()))?;
    Ok(routes
        .iter()
        .filter_map(|route| {
            let trains: Vec<&Value> = route
                .pointer("/details/routes")?
                .as_array()?
                .iter()
                .filter(|part| {
                    part.pointer("/trainDetails/trainNumber")
                        .is_some_and(|number| !number.is_null())
                })
                .collect();
            let departure = trains
                .first()?
                .pointer("/departure/time")?
                .as_str()
                .and_then(local_time)?;
            let names = trains
                .iter()
                .filter_map(|part| {
                    let details = part.get("trainDetails")?;
                    let number = details.get("trainNumber")?.as_str()?;
                    let kind = details
                        .pointer("/trainKind/sortName")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    Some(format!("{kind} {number}").trim().to_owned())
                })
                .collect();
            let classes = route.get("travelClasses").and_then(Value::as_array);
            let class_price = |class: &str| {
                lowest(classes.into_iter().flatten().filter_map(|entry| {
                    (entry.get("name").and_then(Value::as_str) == Some(class))
                        .then(|| entry.pointer("/price/amount")?.as_f64())
                        .flatten()
                }))
            };
            let order_disabled = route
                .get("orderDisabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Some(SiteConnection {
                departure,
                trains: names,
                second_class_price: class_price("2"),
                first_class_price: class_price("1"),
                currency: "HUF".to_owned(),
                not_for_sale: order_disabled,
            })
        })
        .collect())
}

impl ConnectionSource for Mav {
    const NAME: &'static str = "mav";

    async fn fetch(
        &self,
        client: &Client,
        search: &ConnectionSearch,
    ) -> Result<Vec<SiteConnection>, QuoteError> {
        let response = client
            .post(SEARCH_URL)
            .header("accept", "application/json")
            .header("language", "en")
            .header("usersessionid", format!("\"{}\"", uuid::Uuid::now_v7()))
            .header("origin", "https://jegy.mav.hu")
            .json(&request(search))
            .send()
            .await?;
        parse_connections(&check_status(response, SEARCH_URL)?.json().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_leave_out_parts_without_a_train() {
        let response = json!({ "route": [{
            "orderDisabled": false,
            "details": { "routes": [
                { "trainDetails": { "trainNumber": null },
                  "departure": { "time": "2026-11-20T13:20:00+01:00" } },
                { "trainDetails": { "trainNumber": "3174", "trainKind": { "sortName": "BESZ" } },
                  "departure": { "time": "2026-11-20T13:35:00+01:00" } },
                { "trainDetails": { "trainNumber": "143", "trainKind": { "sortName": "EC" } },
                  "departure": { "time": "2026-11-20T14:40:00+01:00" } }
            ] },
            "travelClasses": [{ "name": "2", "price": { "amount": 4600 } }]
        }] });
        let connections = parse_connections(&response).unwrap();
        assert_eq!(connections[0].trains, ["BESZ 3174", "EC 143"]);
        assert_eq!(
            connections[0].departure,
            local_time("2026-11-20T13:35:00").unwrap()
        );
        assert_eq!(connections[0].second_class_price, Some(4600.0));
        assert_eq!(connections[0].first_class_price, None);
    }
}
