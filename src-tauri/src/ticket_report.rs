//! What the app keeps about ticket prices for problem reports: the last ticket plan of each open
//! trip with the journeys it was made from, and a log of the live price lookups with the raw
//! answers of the booking sites. Lookups go to the SBB site or the DB site, by the address of
//! the booking link.

use btt_model::{
    EuroCents, LiveQuoteRequest, Price, PriceKind, PriceSource, TicketLeg, TicketPlan,
    TicketPreferences, TravelClass, Traveller,
};
use btt_quotes::sbb::{Quote, Search, find_trip};
use serde::Serialize;

use crate::state::AppState;

/// The most live price lookups the log keeps. The oldest go first.
const MAXIMUM_LOGGED_LOOKUPS: usize = 100;

/// The last ticket plan of a trip with what it was made from, so the prices can be reproduced.
#[derive(Debug, Clone, Serialize)]
pub struct TicketPlanRecord {
    /// When the plan was made, in RFC 3339 format.
    pub created_at: String,
    /// The chosen journey of each leg that went into the plan.
    pub legs: Vec<TicketLeg>,
    /// The traveller the plan is for, or `None` when it assumed an adult.
    pub traveller: Option<Traveller>,
    /// The ticket preferences of the trip.
    pub preferences: TicketPreferences,
    /// The plan the app showed, with the estimates and the booking links.
    pub plan: TicketPlan,
}

/// How a live price lookup ended.
#[derive(Debug, Clone, Serialize)]
pub enum LiveQuoteOutcome {
    /// The app has no service for the booking site.
    NoService,
    /// The booking link is not one the service reads.
    UnsupportedLink,
    /// The request to the booking site failed or was refused, or requests were paused.
    Failed {
        /// What went wrong.
        error: String,
    },
    /// The site answered and has no trip at the planned departure with the planned rides.
    NoMatchingTrip,
    /// The site has the trip and no price for the class.
    NoPriceForClass,
    /// The site shows the trip and sells no ticket for it.
    NotSold,
    /// The site gave a price in a currency the app does not convert.
    UnknownCurrency {
        /// The currency code.
        currency: String,
    },
    /// The site gave a price.
    Quoted {
        /// The price as shown in the app, in euro.
        price: Price,
    },
}

/// One live price lookup.
#[derive(Debug, Clone, Serialize)]
pub struct LiveQuoteLogEntry {
    /// When the lookup was made, in RFC 3339 format.
    pub at: String,
    /// What was asked.
    pub request: LiveQuoteRequest,
    /// How it ended.
    pub outcome: LiveQuoteOutcome,
    /// Every trip with its prices that the site answered with, as it came back.
    pub site_answer: Option<serde_json::Value>,
}

/// Looks up the live price of a ticket, and returns how it ended with the answer of the site.
pub async fn look_up(
    state: &AppState,
    request: &LiveQuoteRequest,
) -> (LiveQuoteOutcome, Option<serde_json::Value>) {
    if request.booking_link.starts_with(DB_LINK_PREFIX) {
        return look_up_db(state, request).await;
    }
    let (outcome, quotes) = look_up_sbb(state, request).await;
    (
        outcome,
        quotes.and_then(|quotes| serde_json::to_value(quotes).ok()),
    )
}

/// The start of every booking link of the DB site.
const DB_LINK_PREFIX: &str = "https://int.bahn.de/";

/// Looks up the live price of a ticket on the DB site.
async fn look_up_db(
    state: &AppState,
    request: &LiveQuoteRequest,
) -> (LiveQuoteOutcome, Option<serde_json::Value>) {
    let Some(service) = &state.db_service else {
        return (LiveQuoteOutcome::NoService, None);
    };
    let Some(departure) = crate::db_quotes::search_time(&request.booking_link) else {
        return (LiveQuoteOutcome::UnsupportedLink, None);
    };
    let cards = match service.cards(&request.booking_link).await {
        Ok(cards) => cards,
        Err(error) => {
            tracing::info!(%error, "no live price from the DB site");
            return (LiveQuoteOutcome::Failed { error }, None);
        }
    };
    let answer = serde_json::to_value(&cards).ok();
    let Some(card) =
        crate::db_quotes::planned_card(&cards, departure, usize::from(request.ride_count))
    else {
        return (LiveQuoteOutcome::NoMatchingTrip, answer);
    };
    let Some(euro) = card.price else {
        return (LiveQuoteOutcome::NotSold, answer);
    };
    let cents = EuroCents::from_euro(euro);
    let class = match request.travel_class {
        TravelClass::First => "first",
        TravelClass::Second => "second",
    };
    let price = Price {
        low: cents,
        high: cents,
        kind: PriceKind::Live,
        source: PriceSource {
            name: "DB booking site".into(),
            note: format!(
                "The lowest price the DB site shows for {} in {class} class, for one traveller \
                 with no discount card. It can be a saver fare that ties the ticket to the train. \
                 The booking site shows the final price.",
                card.trains.join(" and ")
            )
            .into(),
            as_of: Some(crate::commands::today_utc()),
            page: Some(request.booking_link.clone()),
        },
    };
    (LiveQuoteOutcome::Quoted { price }, answer)
}

/// Looks up the live price of a ticket on the SBB site.
async fn look_up_sbb(
    state: &AppState,
    request: &LiveQuoteRequest,
) -> (LiveQuoteOutcome, Option<Vec<Quote>>) {
    let Some(service) = &state.sbb_service else {
        return (LiveQuoteOutcome::NoService, None);
    };
    let Ok(search) = Search::from_link(&request.booking_link) else {
        return (LiveQuoteOutcome::UnsupportedLink, None);
    };
    let quotes = match service.quotes(&search).await {
        Ok(quotes) => quotes,
        Err(error) => {
            tracing::info!(%error, "no live price from the SBB site");
            return (
                LiveQuoteOutcome::Failed {
                    error: error.to_string(),
                },
                None,
            );
        }
    };
    let departure_seconds = i64::from(request.first_departure_time.0) * 60;
    let Some(quote) = find_trip(&quotes, departure_seconds, usize::from(request.ride_count)) else {
        return (LiveQuoteOutcome::NoMatchingTrip, Some(quotes));
    };
    let class = match request.travel_class {
        TravelClass::First => "FIRST",
        TravelClass::Second => "SECOND",
    };
    let Some(class_price) = quote.prices.as_ref().and_then(|prices| {
        prices
            .prices
            .iter()
            .find(|price| price.travel_class == class)
    }) else {
        return (LiveQuoteOutcome::NoPriceForClass, Some(quotes));
    };
    if class_price.price.currency != "CHF" {
        let currency = class_price.price.currency.clone();
        return (LiveQuoteOutcome::UnknownCurrency { currency }, Some(quotes));
    }
    let francs = f64::from(class_price.price.amount) / 100.0;
    let cents = EuroCents::from_euro(francs * state.fare_data.chf_to_eur);
    let price = Price {
        low: cents,
        high: cents,
        kind: PriceKind::Live,
        source: PriceSource {
            name: "SBB booking site".into(),
            note: format!(
                "CHF {francs:.2} for {} class, {}, converted at 1 CHF = {} EUR, a fixed rough \
                 rate. The booking site shows the final price.",
                class.to_lowercase(),
                class_price
                    .after_sale_flexibility
                    .to_lowercase()
                    .replace('_', " "),
                state.fare_data.chf_to_eur
            )
            .into(),
            as_of: Some(crate::commands::today_utc()),
            page: Some(request.booking_link.clone()),
        },
    };
    (LiveQuoteOutcome::Quoted { price }, Some(quotes))
}

/// Adds a lookup to the log, dropping the oldest when the log is full.
pub fn log_lookup(
    log: &mut Vec<LiveQuoteLogEntry>,
    request: &LiveQuoteRequest,
    outcome: LiveQuoteOutcome,
    site_answer: Option<serde_json::Value>,
) {
    if log.len() >= MAXIMUM_LOGGED_LOOKUPS {
        log.remove(0);
    }
    log.push(LiveQuoteLogEntry {
        at: chrono::Local::now().to_rfc3339(),
        request: request.clone(),
        outcome,
        site_answer,
    });
}

#[cfg(test)]
mod tests {
    use btt_model::{TravelClass, UnixTimeMinutes};

    use super::*;

    fn request(minute: u32) -> LiveQuoteRequest {
        LiveQuoteRequest {
            booking_link: "https://www.sbb.ch/en".into(),
            first_departure_time: UnixTimeMinutes(minute),
            ride_count: 1,
            travel_class: TravelClass::Second,
        }
    }

    #[test]
    fn the_log_drops_the_oldest_lookup_when_full() {
        let mut log = Vec::new();
        for minute in 0..=u32::try_from(MAXIMUM_LOGGED_LOOKUPS).unwrap() {
            log_lookup(
                &mut log,
                &request(minute),
                LiveQuoteOutcome::NoMatchingTrip,
                None,
            );
        }
        assert_eq!(log.len(), MAXIMUM_LOGGED_LOOKUPS);
        assert_eq!(log[0].request.first_departure_time, UnixTimeMinutes(1));
    }

    #[test]
    fn a_lookup_serializes_with_its_outcome() {
        let mut log = Vec::new();
        log_lookup(
            &mut log,
            &request(5),
            LiveQuoteOutcome::Failed {
                error: "refused".to_owned(),
            },
            None,
        );
        let json = serde_json::to_string(&log).unwrap();
        assert!(json.contains(r#""Failed":{"error":"refused"}"#));
        assert!(json.contains(r#""booking_link":"https://www.sbb.ch/en""#));
    }
}
