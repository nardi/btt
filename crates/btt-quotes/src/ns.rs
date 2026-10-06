//! Fares from the NS booking site.
//!
//! NS fares are fixed. A single costs the same on every train of a day between two stations,
//! so a fare is kept for a week and needs no train to match.

use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{QuoteError, QuoteStore, RequestLimiter};

/// What the client says about itself, in the generic form the booking sites accept.
const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (compatible; BigTrainTrip/",
    env!("CARGO_PKG_VERSION"),
    ")"
);

/// The address of the fare service of ns.nl.
const FARES_URL: &str = "https://gateway.apiportal.ns.nl/ecom/v1/fares";

/// The subscription key that the pages of ns.nl send with their requests. It belongs to ns.nl
/// and stands in until the app has a key of its own from the NS API portal.
const SUBSCRIPTION_KEY: &str = "dfbee210f00a4454b14254fa0842ab10";

/// The least time between two requests to the site.
const MINIMUM_INTERVAL: Duration = Duration::from_secs(2);

/// How long requests stop after one fails.
const FAILURE_PAUSE: Duration = Duration::from_mins(15);

/// How long a fare is kept. Fares change with the yearly tariff, so a week is safe.
const FARE_LIFETIME: Duration = Duration::from_hours(7 * 24);

/// The prices of a single between two stations on a day, in euro cents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SingleFare {
    /// Second class.
    pub second_class_cents: Option<u32>,
    /// First class.
    pub first_class_cents: Option<u32>,
}

/// Reads the fare of a single from an answer of the fare service.
///
/// # Errors
///
/// Returns an error when the answer has no list of fares.
pub fn parse_single_fare(response: &Value) -> Result<SingleFare, QuoteError> {
    let fares = response
        .get("fares")
        .and_then(Value::as_array)
        .ok_or_else(|| QuoteError::Unexpected("no fares in the answer".to_owned()))?;
    let cents = |class: &str| {
        fares
            .iter()
            .filter(|fare| fare.get("propositionId").and_then(Value::as_str) == Some("enkele-reis"))
            .filter(|fare| fare.get("travelClass").and_then(Value::as_str) == Some(class))
            .find_map(|fare| fare.pointer("/pricePerUnit/amountInCents")?.as_u64())
            .and_then(|cents| u32::try_from(cents).ok())
    };
    Ok(SingleFare {
        second_class_cents: cents("SECOND_CLASS"),
        first_class_cents: cents("FIRST_CLASS"),
    })
}

/// Fetches fares from ns.nl, keeps them for a week and spaces its requests out.
pub struct NsService {
    /// The HTTP client.
    client: Client,
    /// Keeps the fares.
    store: Arc<dyn QuoteStore>,
    /// Spaces requests out and stops them after a failure.
    limiter: RequestLimiter,
}

impl NsService {
    /// Creates a service that keeps its fares in a store.
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

    /// Returns the fare of a single between two stations, given by their UIC numbers, on a day.
    ///
    /// # Errors
    ///
    /// Returns an error when requests are paused after a failure, the request fails or is
    /// refused, or the answer has an unexpected shape. A failure pauses requests for 15 minutes.
    pub async fn single_fare(
        &self,
        from_uic: &str,
        to_uic: &str,
        date: NaiveDate,
    ) -> Result<SingleFare, QuoteError> {
        let key = format!("ns:{from_uic}:{to_uic}:{date}");
        if let Some(fare) = self
            .store
            .load(&key, FARE_LIFETIME)
            .and_then(|document| serde_json::from_str(&document).ok())
        {
            return Ok(fare);
        }
        let result = self.fetch(from_uic, to_uic, date).await;
        match &result {
            Ok(fare) => {
                if let Ok(document) = serde_json::to_string(fare) {
                    self.store.save(&key, &document);
                }
            }
            Err(QuoteError::Paused) => {}
            Err(_) => self.limiter.report_failure().await,
        }
        result
    }

    /// Asks the fare service for the fare of a single.
    async fn fetch(
        &self,
        from_uic: &str,
        to_uic: &str,
        date: NaiveDate,
    ) -> Result<SingleFare, QuoteError> {
        if !self.limiter.wait_for_turn().await {
            return Err(QuoteError::Paused);
        }
        // The service writes UIC numbers with the prefix 11 for the Netherlands.
        let response = self
            .client
            .get(FARES_URL)
            .query(&[
                ("propositionId", "enkele-reis"),
                ("travelDate", &date.to_string()),
                ("fromUicCode", &format!("11{from_uic}")),
                ("toUicCode", &format!("11{to_uic}")),
                ("adults", "1"),
                ("children", "0"),
                ("youngPeople", "0"),
                ("salesChannel", "ns.nl"),
            ])
            .header("accept", "application/json")
            .header("Ocp-Apim-Subscription-Key", SUBSCRIPTION_KEY)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(QuoteError::Refused {
                url: FARES_URL.to_owned(),
                status: response.status().as_u16(),
            });
        }
        parse_single_fare(&response.json().await?)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_single_fare_is_read_by_class() {
        let response = json!({ "fares": [
            { "propositionId": "enkele-reis", "travelClass": "FIRST_CLASS",
              "pricePerUnit": { "amountInCents": 4536 } },
            { "propositionId": "enkele-reis", "travelClass": "SECOND_CLASS",
              "pricePerUnit": { "amountInCents": 2520 } },
            { "propositionId": "dagretour", "travelClass": "SECOND_CLASS",
              "pricePerUnit": { "amountInCents": 5040 } }
        ] });
        let fare = parse_single_fare(&response).unwrap();
        assert_eq!(fare.second_class_cents, Some(2520));
        assert_eq!(fare.first_class_cents, Some(4536));
    }
}
