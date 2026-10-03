//! Live price quotes from the booking sites of operators.
//!
//! These sources are unofficial. They read what a booking site's own web page reads, so they
//! can break without notice and can be blocked. Each lives in its own module and is meant to be
//! switched off by default.

pub mod sbb;

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
    /// The network request failed.
    #[error("request failed: {0}")]
    Network(#[from] reqwest::Error),
    /// The service answered with something other than the expected data.
    #[error("unexpected response: {0}")]
    Unexpected(String),
}
