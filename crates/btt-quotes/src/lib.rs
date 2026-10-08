//! Live price quotes from the booking sites of operators.
//!
//! These sources are unofficial. They read what a booking site's own web page reads, so they
//! can break without notice and can be blocked. Each lives in its own module and is meant to be
//! switched off by default.

pub mod db;
pub mod dsb;
pub mod ecb;
pub mod eurostar;
pub mod italo;
mod js_literal;
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
}

impl QuoteError {
    /// Returns whether the error is likely gone a moment later: a server error or a request
    /// that failed on the way. A refusal for the request itself, such as a 400 or a 403, is not.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Refused { status, .. } => *status >= 500,
            Self::Network(_) => true,
            _ => false,
        }
    }
}
