//! Exchange rates from the euro foreign exchange reference rates of the European Central Bank.
//!
//! The ECB publishes the rates of about thirty currencies against the euro once each working
//! day, around 16:00 in Frankfurt, as a small XML file that anyone may fetch. The service
//! fetches it at most once a day and keeps the last rates it got.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::site::{USER_AGENT, check_status};
use crate::{QuoteError, QuoteStore, RequestLimiter};

/// The address of the daily reference rates.
const DAILY_RATES_URL: &str = "https://www.ecb.europa.eu/stats/eurofxref/eurofxref-daily.xml";

/// The key under which the rates are kept.
const STORE_KEY: &str = "ecb:daily";

/// How long rates are used before they are fetched again.
const RATES_LIFETIME: Duration = Duration::from_hours(24);

/// How long rates are still used when no newer ones can be fetched.
const STALE_RATES_LIFETIME: Duration = Duration::from_hours(24 * 30);

/// How long fetches stop after one fails.
const FAILURE_PAUSE: Duration = Duration::from_mins(15);

/// The reference rates of one day.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReferenceRates {
    /// The day of the rates, as `YYYY-MM-DD`.
    pub date: String,
    /// Units of each currency that one euro buys, by ISO code.
    pub per_euro: BTreeMap<String, f64>,
}

impl ReferenceRates {
    /// Returns euro for one unit of a currency, or `None` for a currency without a rate.
    pub fn euro_per(&self, currency: &str) -> Option<f64> {
        if currency == "EUR" {
            return Some(1.0);
        }
        self.per_euro
            .get(currency)
            .filter(|rate| **rate > 0.0)
            .map(|rate| 1.0 / rate)
    }
}

/// Reads the rates from the daily file, whose entries look like
/// `<Cube currency='CHF' rate='0.9309'/>` inside `<Cube time='2026-10-07'>`.
///
/// # Errors
///
/// Returns an error when the file has no date or no rates.
pub fn parse_rates(xml: &str) -> Result<ReferenceRates, QuoteError> {
    let attribute = |element: &str, name: &str| {
        let start = element.find(&format!("{name}='"))? + name.len() + 2;
        let length = element[start..].find('\'')?;
        Some(element[start..start + length].to_owned())
    };
    let mut date = None;
    let mut per_euro = BTreeMap::new();
    for element in xml.split("<Cube").skip(1) {
        if let Some(time) = attribute(element, "time") {
            date = Some(time);
        }
        if let (Some(currency), Some(rate)) = (
            attribute(element, "currency"),
            attribute(element, "rate").and_then(|rate| rate.parse::<f64>().ok()),
        ) {
            per_euro.insert(currency, rate);
        }
    }
    match date {
        Some(date) if !per_euro.is_empty() => Ok(ReferenceRates { date, per_euro }),
        _ => Err(QuoteError::Unexpected(
            "no rates in the ECB file".to_owned(),
        )),
    }
}

/// Fetches the reference rates at most once a day and keeps them in a store.
pub struct ExchangeRateService {
    /// The HTTP client.
    client: Client,
    /// Keeps the last rates.
    store: Arc<dyn QuoteStore>,
    /// Stops fetches for a while after one fails.
    limiter: RequestLimiter,
}

impl ExchangeRateService {
    /// Creates a service that keeps the rates in a store.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be built.
    pub fn new(store: Arc<dyn QuoteStore>) -> Result<Self, QuoteError> {
        Ok(Self {
            client: Client::builder()
                .user_agent(USER_AGENT)
                .timeout(Duration::from_secs(10))
                .build()?,
            store,
            limiter: RequestLimiter::new(Duration::ZERO, FAILURE_PAUSE),
        })
    }

    /// Returns the latest rates: the kept ones when they are less than a day old, else newly
    /// fetched ones, else kept ones of up to a month old. Returns `None` when there are none.
    pub async fn rates(&self) -> Option<ReferenceRates> {
        let kept = |max_age| {
            self.store
                .load(STORE_KEY, max_age)
                .and_then(|document| serde_json::from_str(&document).ok())
        };
        if let Some(rates) = kept(RATES_LIFETIME) {
            return Some(rates);
        }
        match self.fetch().await {
            Ok(rates) => {
                if let Ok(document) = serde_json::to_string(&rates) {
                    self.store.save(STORE_KEY, &document);
                }
                Some(rates)
            }
            Err(error) => {
                if !matches!(error, QuoteError::Paused) {
                    self.limiter.report_failure().await;
                }
                kept(STALE_RATES_LIFETIME)
            }
        }
    }

    /// Fetches the daily file.
    async fn fetch(&self) -> Result<ReferenceRates, QuoteError> {
        if !self.limiter.wait_for_turn().await {
            return Err(QuoteError::Paused);
        }
        let response = self.client.get(DAILY_RATES_URL).send().await?;
        parse_rates(&check_status(response, DAILY_RATES_URL)?.text().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_daily_file_gives_euro_per_unit() {
        let xml = "<gesmes:Envelope><Cube><Cube time='2026-10-07'>\
                   <Cube currency='CHF' rate='0.9309'/>\
                   <Cube currency='HUF' rate='366.75'/></Cube></Cube></gesmes:Envelope>";
        let rates = parse_rates(xml).unwrap();
        assert_eq!(rates.date, "2026-10-07");
        assert!((rates.euro_per("CHF").unwrap() - 1.0742).abs() < 0.0001);
        assert_eq!(rates.euro_per("EUR"), Some(1.0));
        assert_eq!(rates.euro_per("SEK"), None);
    }

    #[test]
    fn a_file_without_rates_is_rejected() {
        assert!(parse_rates("<Cube time='2026-10-07'></Cube>").is_err());
    }
}
