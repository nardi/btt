//! Live prices from the booking sites that refuse scripted clients, read from a hidden window of
//! the app: DB, the ÖBB shop and PKP Intercity.

use std::sync::Arc;
use std::time::Duration;

use btt_quotes::{QuoteStore, RequestLimiter};
use tauri::{AppHandle, WebviewWindow};
use tokio::sync::Mutex;

use crate::booking_page::{PageCard, PageResult, create_window, window_result};

/// Label of the window that reads the booking sites.
const WINDOW_LABEL: &str = "booking-sites";

/// The least time between two lookups.
const MINIMUM_INTERVAL: Duration = Duration::from_secs(2);

/// How long lookups stop after one fails.
const FAILURE_PAUSE: Duration = Duration::from_mins(15);

/// How long the connections of a link are kept. Prices change with demand, so the time is short.
const QUOTE_LIFETIME: Duration = Duration::from_mins(30);

/// How long a lookup may take before it counts as failed.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(40);

/// How often the address of the window is read while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Reads connections and prices from booking links, one at a time in a hidden window, keeps
/// them for half an hour and spaces the lookups out. All sites share the window, so the app never
/// loads more than one booking page at a time.
pub struct WebviewQuoteService {
    /// The app, which owns the window.
    app: AppHandle,
    /// Keeps the connections of each link.
    store: Arc<dyn QuoteStore>,
    /// Spaces lookups out and stops them after a failure.
    limiter: RequestLimiter,
    /// The reading window once it exists. The lock also keeps lookups one at a time, because
    /// they share the window.
    window: Mutex<Option<WebviewWindow>>,
}

impl WebviewQuoteService {
    /// Creates the service. The window opens on the first lookup.
    pub fn new(app: AppHandle, store: Arc<dyn QuoteStore>) -> Self {
        Self {
            app,
            store,
            limiter: RequestLimiter::new(MINIMUM_INTERVAL, FAILURE_PAUSE),
            window: Mutex::new(None),
        }
    }

    /// Returns the connections that a booking link shows, with their prices. A link looked up
    /// in the last half hour is answered from the store.
    ///
    /// # Errors
    ///
    /// Returns a description when lookups are paused, the window cannot open, or the page shows
    /// nothing within 40 seconds. A failure pauses lookups for 15 minutes.
    pub async fn cards(&self, link: &str) -> Result<Vec<PageCard>, String> {
        let key = format!("page:{link}");
        if let Some(cards) = self
            .store
            .load(&key, QUOTE_LIFETIME)
            .and_then(|document| serde_json::from_str(&document).ok())
        {
            return Ok(cards);
        }
        let mut window = self.window.lock().await;
        if !self.limiter.wait_for_turn().await {
            return Err("lookups on the booking sites are paused after a failure".to_owned());
        }
        let result = self.read_page(&mut window, link).await;
        match &result {
            Ok(cards) => {
                if let Ok(document) = serde_json::to_string(cards) {
                    self.store.save(&key, &document);
                }
            }
            Err(_) => self.limiter.report_failure().await,
        }
        result
    }

    /// Opens the link in the window, creating the window when there is none, and waits for the
    /// page to report what it shows.
    async fn read_page(
        &self,
        window: &mut Option<WebviewWindow>,
        link: &str,
    ) -> Result<Vec<PageCard>, String> {
        let reading = if let Some(existing) = window.as_ref() {
            // A link that differs only in its fragment can leave the page showing the last
            // search, so the window goes through an empty page first.
            let blank = "about:blank".parse().map_err(|_| "bad address")?;
            let url = link.parse().map_err(|_| format!("bad link {link}"))?;
            existing
                .navigate(blank)
                .map_err(|error| error.to_string())?;
            wait_until_cleared(existing).await;
            existing.navigate(url).map_err(|error| error.to_string())?;
            existing.clone()
        } else {
            let app = self.app.clone();
            let link = link.to_owned();
            let created = tokio::task::spawn_blocking(move || {
                create_window(&app, WINDOW_LABEL, &link, false)
            })
            .await
            .map_err(|error| error.to_string())??;
            *window = Some(created.clone());
            created
        };
        let result = wait_for_result(&reading).await.ok_or_else(|| {
            // A window in a bad state is replaced on the next lookup.
            let _ = reading.destroy();
            *window = None;
            "the booking page showed no connections within 40 seconds".to_owned()
        })?;
        Ok(cards_of(result))
    }
}

/// Returns the connections of a page result that parse.
fn cards_of(result: PageResult) -> Vec<PageCard> {
    result
        .cards
        .into_iter()
        .filter_map(|card| serde_json::from_value(card).ok())
        .collect()
}

/// Waits until the result of an earlier lookup is gone from the address of the window.
async fn wait_until_cleared(window: &WebviewWindow) {
    for _ in 0..250 {
        if window_result(window).is_none() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Waits until the page in the window reports a result, or the timeout passes.
async fn wait_for_result(window: &WebviewWindow) -> Option<PageResult> {
    let started = tokio::time::Instant::now();
    while started.elapsed() < LOOKUP_TIMEOUT {
        tokio::time::sleep(POLL_INTERVAL).await;
        if let Some(result) = window_result(window) {
            return Some(result);
        }
    }
    None
}

/// Returns the connection that is the planned journey: the one that leaves at the time of the
/// search with the planned number of trains. A card that names no trains matches on its time
/// alone.
pub fn planned_card<'a>(
    cards: &'a [PageCard],
    departure: &str,
    ride_count: usize,
) -> Option<&'a PageCard> {
    let at_time = || cards.iter().filter(move |card| card.departure == departure);
    at_time()
        .find(|card| card.trains.len() == ride_count)
        .or_else(|| at_time().find(|card| card.trains.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(departure: &str, trains: &[&str], price: Option<f64>) -> PageCard {
        PageCard {
            departure: departure.to_owned(),
            arrival: None,
            trains: trains.iter().map(|&train| train.to_owned()).collect(),
            price,
            currency: Some("EUR".to_owned()),
        }
    }

    #[test]
    fn the_planned_card_leaves_at_the_time_with_the_planned_trains() {
        let cards = [
            card("11:09", &["ICE 144", "RE 4"], Some(59.99)),
            card("11:09", &["ICE 144"], Some(72.0)),
            card("11:37", &["ICE 602", "ICE 613"], Some(65.99)),
        ];
        assert_eq!(planned_card(&cards, "11:09", 1).unwrap().price, Some(72.0));
        assert_eq!(planned_card(&cards, "11:09", 2).unwrap().price, Some(59.99));
        assert!(planned_card(&cards, "11:09", 3).is_none());
        assert!(planned_card(&cards, "12:00", 1).is_none());
    }
}
