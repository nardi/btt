//! What the booking sites that answer one search with their connections have in common: the
//! search, the connections, and a service that keeps answers for a while and spaces requests
//! out.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::{QuoteError, QuoteStore, RequestLimiter};

/// What the client says about itself, in the generic form the booking sites accept.
pub(crate) const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (compatible; BigTrainTrip/",
    env!("CARGO_PKG_VERSION"),
    ")"
);

/// The least time between two searches on one site.
const MINIMUM_INTERVAL: Duration = Duration::from_secs(2);

/// How long searches on a site stop after one fails.
const FAILURE_PAUSE: Duration = Duration::from_mins(15);

/// How long a search waits before it is tried again after a transient error.
const RETRY_DELAY: Duration = Duration::from_secs(2);

/// How long the connections of a search are kept. Prices change with demand, so the time is
/// short.
const QUOTE_LIFETIME: Duration = Duration::from_mins(30);

/// A search for the connections between two stations of a booking site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionSearch {
    /// Code of the departure station on the site.
    pub origin: String,
    /// Code of the arrival station on the site.
    pub destination: String,
    /// The planned departure of the first train on the local clock of its station. Sites
    /// search from it, and a site that prices one connection at a time prices this one.
    pub departure: NaiveDateTime,
    /// The same departure as a moment, for sites that search from one.
    pub departure_moment: DateTime<Utc>,
    /// The number of trains of the planned connection, or `None` when the site splits or joins
    /// trains differently from the timetable, so that any number matches.
    pub train_count: Option<usize>,
}

impl ConnectionSearch {
    /// Returns whether a connection leaves at the planned moment with the planned number of
    /// trains.
    pub fn is_planned(&self, connection: &SiteConnection) -> bool {
        connection.departure == self.departure
            && self
                .train_count
                .is_none_or(|count| connection.trains.len() == count)
    }
}

impl ConnectionSearch {
    /// Returns the key under which the connections of the search are kept on a site.
    fn cache_key(&self, site: &str) -> String {
        format!(
            "{site}:{}:{}:{}:{:?}",
            self.origin, self.destination, self.departure, self.train_count
        )
    }
}

/// A connection a booking site offers, with its lowest prices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiteConnection {
    /// Departure of the first train on the local clock of its station.
    pub departure: NaiveDateTime,
    /// The trains, such as `IC 835`.
    pub trains: Vec<String>,
    /// The lowest second class price for one adult, or `None` when the site sells none.
    pub second_class_price: Option<f64>,
    /// The lowest first class price for one adult, or `None` when the site sells none.
    pub first_class_price: Option<f64>,
    /// ISO code of the currency of the prices, such as `SEK`.
    pub currency: String,
    /// Whether the site lists the connection as not for sale, such as when it is sold out.
    pub not_for_sale: bool,
}

/// Returns the connection that leaves at the planned moment with the planned number of trains.
pub fn find_connection<'a>(
    connections: &'a [SiteConnection],
    search: &ConnectionSearch,
) -> Option<&'a SiteConnection> {
    connections
        .iter()
        .find(|connection| search.is_planned(connection))
}

/// A booking site that answers a search with its connections.
pub trait ConnectionSource: Send + Sync {
    /// Name of the site in cache keys, such as `dsb`.
    const NAME: &'static str;

    /// Asks the site for the connections of a search.
    fn fetch(
        &self,
        client: &Client,
        search: &ConnectionSearch,
    ) -> impl Future<Output = Result<Vec<SiteConnection>, QuoteError>> + Send;
}

/// Fetches connections from a booking site, keeps them for a while and spaces its requests out.
pub struct SiteService<S> {
    /// The site.
    source: S,
    /// The HTTP client, which keeps the cookies of the site.
    client: Client,
    /// Keeps the connections of searches.
    store: Arc<dyn QuoteStore>,
    /// Spaces searches out and stops them after a failure.
    limiter: RequestLimiter,
}

impl<S: ConnectionSource> SiteService<S> {
    /// Creates a service for a site that keeps its answers in a store.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be built.
    pub fn new(source: S, store: Arc<dyn QuoteStore>) -> Result<Self, QuoteError> {
        Ok(Self {
            source,
            client: Client::builder()
                .user_agent(USER_AGENT)
                .timeout(Duration::from_secs(30))
                .cookie_store(true)
                .build()?,
            store,
            limiter: RequestLimiter::new(MINIMUM_INTERVAL, FAILURE_PAUSE),
        })
    }

    /// Returns the connections of a search. A search made in the last half hour is answered
    /// from the store.
    ///
    /// # Errors
    ///
    /// Returns an error when searches are paused after a failure, a request fails or is
    /// refused, or an answer has an unexpected shape. A failure pauses searches for 15 minutes.
    pub async fn connections(
        &self,
        search: &ConnectionSearch,
    ) -> Result<Vec<SiteConnection>, QuoteError> {
        let key = search.cache_key(S::NAME);
        if let Some(connections) = self
            .store
            .load(&key, QUOTE_LIFETIME)
            .and_then(|document| serde_json::from_str(&document).ok())
        {
            return Ok(connections);
        }
        if !self.limiter.wait_for_turn().await {
            return Err(QuoteError::Paused);
        }
        let mut result = self.source.fetch(&self.client, search).await;
        // A server error or a dropped connection is often gone a moment later, as with the
        // offer service of MÁV, so the search is tried once more before the site is paused.
        if result.as_ref().is_err_and(QuoteError::is_transient) {
            tokio::time::sleep(RETRY_DELAY).await;
            result = self.source.fetch(&self.client, search).await;
        }
        match &result {
            Ok(connections) => {
                if let Ok(document) = serde_json::to_string(connections) {
                    self.store.save(&key, &document);
                }
            }
            Err(_) => self.limiter.report_failure().await,
        }
        result
    }
}

/// Returns an error for an answer with a status other than success.
pub(crate) fn check_status(
    response: reqwest::Response,
    url: &str,
) -> Result<reqwest::Response, QuoteError> {
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(QuoteError::Refused {
            url: url.to_owned(),
            status: response.status().as_u16(),
        })
    }
}

/// Returns the smallest of some prices, or `None` when there are none.
pub(crate) fn lowest(prices: impl IntoIterator<Item = f64>) -> Option<f64> {
    prices.into_iter().reduce(f64::min)
}

/// Reads a local date and time such as `2026-11-20T09:32:00+01:00` or `2026-11-20T09:32:00`,
/// keeping the clock time and dropping any offset.
pub(crate) fn local_time(text: &str) -> Option<NaiveDateTime> {
    let clock = text.get(..19)?;
    NaiveDateTime::parse_from_str(clock, "%Y-%m-%dT%H:%M:%S").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_times_keep_the_clock_of_the_station() {
        let expected = chrono::NaiveDate::from_ymd_opt(2026, 11, 20)
            .unwrap()
            .and_hms_opt(9, 32, 0)
            .unwrap();
        assert_eq!(local_time("2026-11-20T09:32:00+01:00"), Some(expected));
        assert_eq!(local_time("2026-11-20T09:32:00.000"), Some(expected));
        assert_eq!(local_time("09:32"), None);
    }

    #[test]
    fn server_errors_are_tried_again_and_refusals_are_not() {
        let refused = |status| QuoteError::Refused {
            url: String::new(),
            status,
        };
        assert!(refused(500).is_transient());
        assert!(refused(503).is_transient());
        assert!(!refused(400).is_transient());
        assert!(!refused(403).is_transient());
        assert!(!QuoteError::Paused.is_transient());
        assert!(!QuoteError::Unexpected(String::new()).is_transient());
    }

    #[test]
    fn the_planned_connection_is_found_by_departure_and_train_count() {
        let departure = local_time("2026-11-20T09:32:00").unwrap();
        let connection = |trains: &[&str]| SiteConnection {
            departure,
            trains: trains.iter().map(|&train| train.to_owned()).collect(),
            second_class_price: Some(88.0),
            first_class_price: None,
            currency: "DKK".to_owned(),
            not_for_sale: false,
        };
        let connections = [connection(&["IC 835", "RE 1"]), connection(&["IC 835"])];
        let search = ConnectionSearch {
            origin: "8600626".to_owned(),
            destination: "8600512".to_owned(),
            departure,
            departure_moment: departure.and_utc(),
            train_count: Some(1),
        };
        assert_eq!(
            find_connection(&connections, &search).map(|found| found.trains.len()),
            Some(1)
        );
    }
}
