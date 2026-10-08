//! Prices from the Trenitalia booking site, lefrecce.it.
//!
//! The site's price service answers plain requests. A search takes the location ids of two
//! stations, a departure time and the number of travellers, and returns up to ten solutions with
//! their trains and the lowest price for the party.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, NaiveDateTime};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{QuoteError, QuoteStore, RequestLimiter};

/// What the client says about itself, in the generic form the booking sites accept.
const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (compatible; BigTrainTrip/",
    env!("CARGO_PKG_VERSION"),
    ")"
);

/// The address of the search for solutions.
const SOLUTIONS_URL: &str =
    "https://www.lefrecce.it/Channels.Website.BFF.WEB/website/ticket/solutions";

/// The least time between two requests to the site.
const MINIMUM_INTERVAL: Duration = Duration::from_secs(2);

/// How long requests stop after one fails.
const FAILURE_PAUSE: Duration = Duration::from_mins(15);

/// How long the solutions of a search are kept. Prices change with demand, so the time is short.
const QUOTE_LIFETIME: Duration = Duration::from_mins(30);

/// A search on the site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Search {
    /// Location id of the departure station, such as 830005043 for Bologna Centrale.
    pub origin_id: u64,
    /// Location id of the arrival station.
    pub destination_id: u64,
    /// The local time to leave at or after.
    pub departure: NaiveDateTime,
    /// Number of adults.
    pub adults: u8,
    /// Number of children.
    pub children: u8,
}

impl Search {
    /// Builds the request body the site's page sends.
    pub fn request(&self) -> Value {
        json!({
            "departureLocationId": self.origin_id,
            "arrivalLocationId": self.destination_id,
            "departureTime": self.departure.format("%Y-%m-%dT%H:%M:00.000").to_string(),
            "adults": self.adults,
            "children": self.children,
            "criteria": {
                "frecceOnly": false,
                "regionalOnly": false,
                "intercityOnly": false,
                "tourismOnly": false,
                "noChanges": false,
                "order": "DEPARTURE_DATE",
                "offset": 0,
                "limit": 10,
            },
            "advancedSearchRequest": {
                "bestFare": false,
                "bikeFilter": false,
                "forwardDiscountCodes": [],
            },
        })
    }

    /// Returns the key under which the solutions of the search are kept.
    fn cache_key(&self) -> String {
        format!(
            "trenitalia:{}:{}:{}:{}:{}",
            self.origin_id, self.destination_id, self.departure, self.adults, self.children
        )
    }
}

/// A solution the site found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Solution {
    /// Departure from the first station.
    pub departure: DateTime<FixedOffset>,
    /// Arrival at the last station.
    pub arrival: DateTime<FixedOffset>,
    /// The trains, such as `FR 9321`.
    pub trains: Vec<String>,
    /// The lowest price for the party in euro, or `None` when the site sells none.
    pub price: Option<f64>,
}

/// Reads the solutions from an answer of the site.
///
/// # Errors
///
/// Returns an error when the answer has no list of solutions.
pub fn parse_solutions(response: &Value) -> Result<Vec<Solution>, QuoteError> {
    let solutions = response
        .get("solutions")
        .and_then(Value::as_array)
        .ok_or_else(|| QuoteError::Unexpected("no solutions in the answer".to_owned()))?;
    Ok(solutions
        .iter()
        .filter_map(|entry| {
            let solution = entry.get("solution")?;
            let time = |key: &str| {
                solution
                    .get(key)
                    .and_then(Value::as_str)
                    .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            };
            let trains = solution
                .get("trains")
                .and_then(Value::as_array)
                .map(|trains| {
                    trains
                        .iter()
                        .filter_map(|train| {
                            let acronym = train.get("acronym")?.as_str()?;
                            let name = train.get("name")?.as_str()?;
                            Some(format!("{acronym} {name}"))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let price = solution
                .pointer("/price/amount")
                .and_then(Value::as_f64)
                .filter(|_| {
                    // `INHIBITED` solutions show a price but cannot be bought, such as trains
                    // whose sale has not opened.
                    !matches!(
                        solution.get("status").and_then(Value::as_str),
                        Some("NOT_SALEABLE" | "INHIBITED")
                    )
                });
            Some(Solution {
                departure: time("departureTime")?,
                arrival: time("arrivalTime")?,
                trains,
                price,
            })
        })
        .collect())
}

/// Returns the solution that leaves at the given moment, in seconds since the Unix epoch, with
/// the given number of trains.
pub fn find_solution(
    solutions: &[Solution],
    departure_unix_seconds: i64,
    train_count: usize,
) -> Option<&Solution> {
    solutions.iter().find(|solution| {
        solution.departure.timestamp() == departure_unix_seconds
            && solution.trains.len() == train_count
    })
}

/// Fetches solutions from the site, keeps them for a while and spaces its requests out.
pub struct TrenitaliaService {
    /// The HTTP client.
    client: Client,
    /// Keeps the solutions of searches.
    store: Arc<dyn QuoteStore>,
    /// Spaces requests out and stops them after a failure.
    limiter: RequestLimiter,
}

impl TrenitaliaService {
    /// Creates a service that keeps its answers in a store.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be built.
    pub fn new(store: Arc<dyn QuoteStore>) -> Result<Self, QuoteError> {
        Ok(Self {
            client: Client::builder()
                .user_agent(USER_AGENT)
                .timeout(Duration::from_secs(20))
                .build()?,
            store,
            limiter: RequestLimiter::new(MINIMUM_INTERVAL, FAILURE_PAUSE),
        })
    }

    /// Returns the solutions of a search. A search made in the last half hour is answered from
    /// the store.
    ///
    /// # Errors
    ///
    /// Returns an error when requests are paused after a failure, the request fails or is
    /// refused, or the answer has an unexpected shape. A failure pauses requests for 15 minutes.
    pub async fn solutions(&self, search: &Search) -> Result<Vec<Solution>, QuoteError> {
        let key = search.cache_key();
        if let Some(solutions) = self
            .store
            .load(&key, QUOTE_LIFETIME)
            .and_then(|document| serde_json::from_str(&document).ok())
        {
            return Ok(solutions);
        }
        let result = crate::with_one_retry(|| self.fetch(search)).await;
        match &result {
            Ok(solutions) => {
                if let Ok(document) = serde_json::to_string(solutions) {
                    self.store.save(&key, &document);
                }
            }
            Err(QuoteError::Paused) => {}
            Err(_) => self.limiter.report_failure().await,
        }
        result
    }

    /// Asks the site for the solutions of a search.
    async fn fetch(&self, search: &Search) -> Result<Vec<Solution>, QuoteError> {
        if !self.limiter.wait_for_turn().await {
            return Err(QuoteError::Paused);
        }
        let response = self
            .client
            .post(SOLUTIONS_URL)
            .header("accept", "application/json")
            .json(&search.request())
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(QuoteError::Refused {
                url: SOLUTIONS_URL.to_owned(),
                status: response.status().as_u16(),
            });
        }
        parse_solutions(&response.json().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solutions_are_read_with_their_trains_and_prices() {
        let response = json!({ "solutions": [
            { "solution": {
                "departureTime": "2026-10-14T13:02:00.000+02:00",
                "arrivalTime": "2026-10-14T16:09:00.000+02:00",
                "status": "SALEABLE",
                "trains": [
                    { "acronym": "RV", "name": "3030" },
                    { "acronym": "FR", "name": "9321" }
                ],
                "price": { "currency": "€", "amount": 46.2 }
            } },
            { "solution": {
                "departureTime": "2026-10-14T13:39:00.000+02:00",
                "arrivalTime": "2026-10-14T19:25:00.000+02:00",
                "status": "NOT_SALEABLE",
                "trains": [{ "acronym": "R", "name": "12381" }],
                "price": { "currency": "€", "amount": 26 }
            } }
        ] });
        let solutions = parse_solutions(&response).unwrap();
        assert_eq!(solutions[0].trains, ["RV 3030", "FR 9321"]);
        assert_eq!(solutions[0].price, Some(46.2));
        assert_eq!(solutions[1].price, None);
        let departure = DateTime::parse_from_rfc3339("2026-10-14T13:02:00+02:00")
            .unwrap()
            .timestamp();
        assert!(find_solution(&solutions, departure, 2).is_some());
        assert!(find_solution(&solutions, departure, 1).is_none());
    }

    #[test]
    fn the_request_has_the_shape_of_the_pages() {
        let search = Search {
            origin_id: 830_004_700,
            destination_id: 830_005_043,
            departure: chrono::NaiveDate::from_ymd_opt(2026, 10, 20)
                .unwrap()
                .and_hms_opt(13, 0, 0)
                .unwrap(),
            adults: 1,
            children: 0,
        };
        let request = search.request();
        assert_eq!(request["departureLocationId"], 830_004_700);
        assert_eq!(request["departureTime"], "2026-10-20T13:00:00.000");
        assert_eq!(request["criteria"]["limit"], 10);
    }
}
