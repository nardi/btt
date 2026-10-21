//! Feeds the live prices of tickets to the price model and keeps them in the database, so the
//! model starts from them again on the next start.

use btt_fares::RideFacts;
use btt_model::{LiveQuoteRequest, Price, PriceObservation};

use crate::state::AppState;
use crate::ticket_report::LiveQuoteOutcome;

/// Days that a live price stays in the database. Forgetting has made older prices count for
/// almost nothing.
pub const OBSERVATION_LIFETIME_DAYS: u64 = 2 * 365;

/// Returns what the price model estimates for the ticket of a request, and teaches the model
/// the live price when the lookup found one. The estimate comes first, so a problem report
/// shows how far off the model was.
///
/// A request without a run is for a traveller whose fare the model does not predict, and then
/// this does nothing. A price for a departure that the database already has from the same day
/// is not learned twice.
pub fn learn_from_lookup(
    state: &AppState,
    request: &LiveQuoteRequest,
    outcome: &LiveQuoteOutcome,
) -> Option<Price> {
    let run = request.run.as_ref()?;
    let rides = RideFacts::of_run(run);
    let estimate = state.fare_data.price_model.read().price(&rides);
    if let LiveQuoteOutcome::Quoted { price } = outcome {
        let observation = PriceObservation {
            run: run.clone(),
            price: price.low,
            original: price.original.clone(),
            looked_up_on: crate::commands::today_utc(),
            source: price.source.name.clone(),
        };
        match state.storage.store_price_observation(&observation) {
            Ok(true) => {
                state.fare_data.price_model.write().observe(&observation);
            }
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "could not save a live price for the price model"),
        }
    }
    estimate
}

/// Teaches the price model the live prices in the database, after deleting those that are too
/// old to matter, and moves the model to today.
pub fn replay_observations(state: &AppState) {
    let today = crate::commands::today_utc();
    let oldest = today
        .checked_sub_days(chrono::Days::new(OBSERVATION_LIFETIME_DAYS))
        .unwrap_or(today);
    if let Err(error) = state.storage.delete_price_observations_before(oldest) {
        tracing::warn!(%error, "could not delete old live prices");
    }
    let observations = state
        .storage
        .list_price_observations()
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "could not read the live prices for the price model");
            Vec::new()
        });
    let mut model = state.fare_data.price_model.write();
    let started = std::time::Instant::now();
    for observation in &observations {
        model.observe(observation);
    }
    model.advance_to(today);
    tracing::info!(
        replayed = observations.len(),
        learned = model.observation_count(),
        milliseconds = started.elapsed().as_millis(),
        "the price model learned from the saved live prices"
    );
}
