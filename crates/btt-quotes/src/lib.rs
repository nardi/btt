//! Live price quotes from the booking sites of operators.
//!
//! These sources are unofficial. They read what a booking site's own web page reads, so they
//! can break without notice and can be blocked. Each lives in its own module and is meant to be
//! switched off by default.

pub mod db;
pub mod dsb;
pub mod ecb;
pub mod european_sleeper;
pub mod eurostar;
pub mod italo;
mod js_literal;
pub mod koleo;
pub mod limiter;
pub mod mav;
pub mod ns;
pub mod obb;
pub mod regiojet;
pub mod renfe;
pub mod sbb;
pub mod site;
pub mod sj;
pub mod snalltaget;
pub mod store;
pub mod trenitalia;

pub use limiter::RequestLimiter;
pub use site::{ConnectionSearch, ConnectionSource, SiteConnection, SiteService, find_connection};
pub use store::{MemoryStore, QuoteStore};

use thiserror::Error;

/// Why a quote could not be fetched.
#[derive(Debug, Error)]
pub enum QuoteError {
    /// The booking link is not one this source understands.
    #[error("unsupported booking link: {0}")]
    BadLink(String),
    /// The page did not hold the address of the price service.
    #[error("the booking page does not name its price service")]
    NoServiceAddress,
    /// The server refused the request, for example because it blocks automated clients.
    #[error("{url} refused the request with status {status}")]
    Refused {
        /// The address that refused.
        url: String,
        /// The HTTP status.
        status: u16,
    },
    /// Requests are stopped for a while after an earlier failure, so the site is left alone.
    #[error("requests to the site are paused after a failure")]
    Paused,
    /// The network request failed.
    #[error("request failed: {0}")]
    Network(#[from] reqwest::Error),
    /// The service answered with something other than the expected data.
    #[error("unexpected response: {0}")]
    Unexpected(String),
    /// The site could not answer this one search, while it answers others. The site is not
    /// paused for it.
    #[error("the site could not answer the search: {0}")]
    SearchFailed(String),
}

impl QuoteError {
    /// Returns whether the error is likely gone a moment later: a server error, a request
    /// that failed on the way, or a refusal for too many requests. A refusal for the request
    /// itself, such as a 400 or a 403, is not.
    pub fn is_transient(&self) -> bool {
        self.retry_delay().is_some()
    }

    /// Returns how long to wait before trying again after the error, or `None` when trying
    /// again would not help. A site that refuses for too many requests (status 429) needs a
    /// longer rest than one that failed by chance.
    pub fn retry_delay(&self) -> Option<std::time::Duration> {
        match self {
            Self::Refused { status: 429, .. } => Some(std::time::Duration::from_secs(10)),
            Self::Refused { status, .. } if *status >= 500 => {
                Some(std::time::Duration::from_secs(2))
            }
            Self::Network(_) => Some(std::time::Duration::from_secs(2)),
            _ => None,
        }
    }
}

/// Runs a lookup, and runs it once more after a wait when it fails with a transient error. A
/// server error, a dropped connection or a refusal for too many requests is often gone a moment
/// later, and a failure pauses the site for a quarter of an hour.
pub(crate) async fn with_one_retry<T, Lookup>(
    mut lookup: impl FnMut() -> Lookup,
) -> Result<T, QuoteError>
where
    Lookup: std::future::Future<Output = Result<T, QuoteError>>,
{
    let result = lookup().await;
    if let Some(delay) = result.as_ref().err().and_then(QuoteError::retry_delay) {
        tokio::time::sleep(delay).await;
        return lookup().await;
    }
    result
}
