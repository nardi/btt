//! Prices from Italo, biglietti.italotreno.com.
//!
//! The site's booking service answers plain requests from an anonymous session. Logging in
//! anonymously with a new working session id returns a token in a cookie. A search takes the
//! site's station codes, such as `RG_` for Milano Rogoredo, and a date. It starts an operation
//! whose status returns the solutions of the day with the fares of each train once it is done.

use std::time::{Duration, Instant};

use reqwest::Client;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::QuoteError;
use crate::site::{
    ConnectionSearch, ConnectionSource, SiteConnection, check_status, local_time, lowest,
};

/// The address of the anonymous login.
const LOGIN_URL: &str = "https://biglietti.italotreno.com/api/login";

/// The address of the booking service.
const BOOKING_URL: &str = "https://api-biglietti.italotreno.com/api/v1/booking";

/// The origin of the site's own pages.
const ORIGIN: &str = "https://biglietti.italotreno.com";

/// How long a session is used. Its token lasts an hour.
const SESSION_LIFETIME: Duration = Duration::from_mins(50);

/// How often the status of a search is asked before giving up.
const MAXIMUM_POLLS: u32 = 10;

/// An anonymous session of the booking service.
#[derive(Debug, Clone)]
struct Session {
    /// The working session id that the login was made with.
    working_id: String,
    /// The token of the session.
    token: String,
    /// When the session was opened.
    opened: Instant,
}

/// Italo, read with plain requests.
#[derive(Debug, Default)]
pub struct Italo {
    /// The current session, if one is open.
    session: Mutex<Option<Session>>,
}

impl Italo {
    /// Returns an open session, logging in anew when there is none or it is old.
    async fn session(&self, client: &Client) -> Result<Session, QuoteError> {
        let mut session = self.session.lock().await;
        if let Some(open) = session.as_ref()
            && open.opened.elapsed() < SESSION_LIFETIME
        {
            return Ok(open.clone());
        }
        let working_id = uuid::Uuid::now_v7().to_string();
        let response = client
            .post(LOGIN_URL)
            .header("origin", ORIGIN)
            .json(&json!({ "isAnonymous": true, "workingId": working_id }))
            .send()
            .await?;
        let response = check_status(response, LOGIN_URL)?;
        let token = response
            .cookies()
            .find(|cookie| cookie.name() == "BIGSessionToken")
            .map(|cookie| cookie.value().to_owned())
            .ok_or_else(|| QuoteError::Unexpected("no session token after login".to_owned()))?;
        let open = Session {
            working_id,
            token,
            opened: Instant::now(),
        };
        *session = Some(open.clone());
        Ok(open)
    }
}

/// Builds the search that the site's page sends.
pub fn request(search: &ConnectionSearch) -> Value {
    json!({
        "isRoundTrip": false,
        "departureStation": search.origin,
        "arrivalStation": search.destination,
        "departureDate": search.departure.format("%Y-%m-%d").to_string(),
        "promoCode": "",
        "passengersAges": null,
        "seniorPassengers": 0,
        "adultPassengers": 1,
        "youngPassengers": 0,
        "childPassengers": 0,
        "culture": "en-US",
        "referrerURL": "https://www.italotreno.com/",
        "promocodeAlias": "",
        "hasPet": false,
        "showBestPrices": true,
        "showPrivateOffers": false,
        "employeeOffer": null,
        "portalType": "B2C",
        "isExclusive": false,
        "flowType": "Booking",
    })
}

/// Returns the lowest fare of a train in a class of the site, `S` for Smart or `P` for Prima,
/// among the fares that still have seats.
fn segment_price(segment: &Value, product_class: &str) -> Option<f64> {
    lowest(
        segment
            .get("fares")?
            .as_array()?
            .iter()
            .filter(|fare| {
                fare.get("productClass").and_then(Value::as_str) == Some(product_class)
                    && fare.get("availableCount").and_then(Value::as_u64) != Some(0)
            })
            .filter_map(|fare| fare.pointer("/paxFares/0/fullPaxDiscountPrice")?.as_f64()),
    )
}

/// Reads the solutions and the lowest price of each class from a finished search. A solution
/// with several trains costs the sum of the lowest fare of each.
///
/// # Errors
///
/// Returns an error when the answer has no list of solutions.
pub fn parse_connections(response: &Value) -> Result<Vec<SiteConnection>, QuoteError> {
    let solutions = response
        .pointer("/trips/0/travelSolutions")
        .and_then(Value::as_array)
        .ok_or_else(|| QuoteError::Unexpected("no solutions in the answer".to_owned()))?;
    Ok(solutions
        .iter()
        .filter_map(|solution| {
            let segments: Vec<&Value> = solution
                .get("journeys")?
                .as_array()?
                .iter()
                .filter_map(|journey| journey.get("segments")?.as_array())
                .flatten()
                .collect();
            let departure = segments
                .first()?
                .get("std")?
                .as_str()
                .and_then(local_time)?;
            let trains = segments
                .iter()
                .filter_map(|segment| {
                    segment
                        .get("trainNumber")?
                        .as_str()
                        .map(|number| format!("Italo {number}"))
                })
                .collect();
            let total = |product_class: &str| {
                segments
                    .iter()
                    .map(|segment| segment_price(segment, product_class))
                    .sum::<Option<f64>>()
            };
            let second_class_price = total("S");
            let first_class_price = total("P");
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

impl ConnectionSource for Italo {
    const NAME: &'static str = "italo";

    async fn fetch(
        &self,
        client: &Client,
        search: &ConnectionSearch,
    ) -> Result<Vec<SiteConnection>, QuoteError> {
        // A search whose planning failed often works when it is started anew in a new session.
        match self.search_once(client, search).await {
            Err(QuoteError::SearchFailed(_)) => self.search_once(client, search).await,
            result => result,
        }
    }
}

/// Adds the session and the headers of the site's pages to a request of the booking service.
fn authorized(request: reqwest::RequestBuilder, session: &Session) -> reqwest::RequestBuilder {
    request
        .bearer_auth(&session.token)
        .header("accept", "application/json")
        .header("x-big-working-session-id", &session.working_id)
        .header("origin", ORIGIN)
}

impl Italo {
    /// Starts a search and waits for its trips.
    async fn search_once(
        &self,
        client: &Client,
        search: &ConnectionSearch,
    ) -> Result<Vec<SiteConnection>, QuoteError> {
        let session = self.session(client).await?;
        let response = authorized(client.post(BOOKING_URL).json(&request(search)), &session)
            .send()
            .await;
        let started: Value = match response {
            Ok(response) if response.status().is_success() => response.json().await?,
            Ok(response) => {
                // A refused search ends the session, so the next search logs in anew.
                *self.session.lock().await = None;
                return Err(QuoteError::Refused {
                    url: BOOKING_URL.to_owned(),
                    status: response.status().as_u16(),
                });
            }
            Err(error) => return Err(error.into()),
        };
        let operation = started
            .get("operationId")
            .and_then(Value::as_str)
            .ok_or_else(|| QuoteError::Unexpected("no operation id in the answer".to_owned()))?;
        let wait = started
            .get("pollAfter")
            .and_then(Value::as_u64)
            .unwrap_or(1500);
        self.poll_status(client, &session, operation, wait).await
    }

    /// Asks for the status of a started search until it holds the trips, and reads them.
    async fn poll_status(
        &self,
        client: &Client,
        session: &Session,
        operation: &str,
        wait: u64,
    ) -> Result<Vec<SiteConnection>, QuoteError> {
        let status_url = format!("{BOOKING_URL}/status/{operation}");
        for _ in 0..MAXIMUM_POLLS {
            tokio::time::sleep(Duration::from_millis(wait)).await;
            let response = authorized(client.get(&status_url), session).send().await?;
            // Italo's planner fails with 500 for some searches, such as Treviso Centrale to
            // Pordenone on one day, while it plans the same pair on other days and answers
            // other searches. The status of some searches answers 404 instead, as if the
            // operation was lost. Both are failures of the search and not of the site. They end
            // the session, so the next search logs in anew.
            if matches!(
                response.status(),
                reqwest::StatusCode::INTERNAL_SERVER_ERROR | reqwest::StatusCode::NOT_FOUND
            ) {
                *self.session.lock().await = None;
                return Err(QuoteError::SearchFailed(
                    "Italo's planner failed for the search".to_owned(),
                ));
            }
            let status: Value = check_status(response, &status_url)?.json().await?;
            if status.get("trips").is_some() {
                return parse_connections(&status);
            }
        }
        Err(QuoteError::Unexpected(
            "the search did not finish in time".to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solutions_take_the_lowest_fare_with_seats_of_each_class() {
        let fare = |class: &str, price: f64, seats: u64| {
            json!({ "productClass": class, "availableCount": seats,
                    "paxFares": [{ "fullPaxDiscountPrice": price }] })
        };
        let response = json!({ "trips": [{ "travelSolutions": [{ "journeys": [{ "segments": [{
            "std": "2026-11-20T05:50:00",
            "trainNumber": "9907",
            "fares": [
                fare("S", 89.9, 247), fare("S", 53.9, 67), fare("S", 29.9, 0),
                fare("P", 57.9, 40), fare("C", 109.9, 8)
            ]
        }] }] }] }] });
        let connections = parse_connections(&response).unwrap();
        assert_eq!(connections[0].trains, ["Italo 9907"]);
        assert_eq!(connections[0].second_class_price, Some(53.9));
        assert_eq!(connections[0].first_class_price, Some(57.9));
    }
}
