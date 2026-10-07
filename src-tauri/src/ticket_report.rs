//! What the app keeps about ticket prices for problem reports: the last ticket plan of each open
//! trip with the journeys it was made from, and a log of the live price lookups with the raw
//! answers of the booking sites. Lookups go to the booking site that the address of the
//! booking link names.

use btt_model::{
    EuroCents, LiveQuoteRequest, Price, PriceKind, PriceSource, TicketLeg, TicketPlan,
    TicketPreferences, TravelClass, Traveller,
};
use btt_quotes::sbb::{Quote, Search, find_trip};
use serde::Serialize;

use crate::booking_links::{BookingSite, parameter};
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
    /// The site's price covers only part of the trip, so it is no price for the ticket.
    PartialPrice,
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
    match BookingSite::of(&request.booking_link) {
        None => (LiveQuoteOutcome::UnsupportedLink, None),
        Some(BookingSite::Sbb) => {
            let (outcome, quotes) = look_up_sbb(state, request).await;
            (
                outcome,
                quotes.and_then(|quotes| serde_json::to_value(quotes).ok()),
            )
        }
        Some(BookingSite::Trenitalia) => look_up_trenitalia(state, request).await,
        Some(BookingSite::Ns) => look_up_ns(state, request).await,
        Some(BookingSite::Db) => look_up_db(state, request).await,
        Some(BookingSite::Obb) => look_up_obb(state, request).await,
        Some(BookingSite::Pkp) => look_up_in_web_view(state, request, BookingSite::Pkp).await,
        Some(site) => look_up_on_site(state, request, site).await,
    }
}

/// Returns the name of a booking site for price sources.
fn site_name(site: BookingSite) -> &'static str {
    match site {
        BookingSite::Sbb => "SBB booking site",
        BookingSite::Db => "DB booking site",
        BookingSite::Obb => "ÖBB shop",
        BookingSite::Pkp => "PKP Intercity booking site",
        BookingSite::Trenitalia => "Trenitalia booking site",
        BookingSite::Ns => "NS fares",
        BookingSite::Dsb => "DSB booking site",
        BookingSite::Sj => "SJ booking site",
        BookingSite::Snalltaget => "Snälltåget booking site",
        BookingSite::Mav => "MÁV booking site",
        BookingSite::Italo => "Italo booking site",
        BookingSite::Renfe => "Renfe booking site",
        BookingSite::Eurostar => "Eurostar booking site",
    }
}

/// Returns a live price in euro from an amount in a currency, with a note on its source, or the
/// outcome for a currency the app does not convert.
fn live_price(
    state: &AppState,
    request: &LiveQuoteRequest,
    site: BookingSite,
    amount: f64,
    currency: &str,
    note: &str,
) -> LiveQuoteOutcome {
    let Some(rate) = state.fare_data.exchange_rates.euro_per(currency) else {
        return LiveQuoteOutcome::UnknownCurrency {
            currency: currency.to_owned(),
        };
    };
    let conversion = if currency == "EUR" {
        String::new()
    } else {
        format!(" {currency} {amount:.2}, converted at a fixed rough rate of {rate} euro.")
    };
    LiveQuoteOutcome::Quoted {
        price: Price {
            low: EuroCents::from_euro(amount * rate),
            high: EuroCents::from_euro(amount * rate),
            kind: PriceKind::Live,
            source: PriceSource {
                name: site_name(site).into(),
                note: format!("{note}{conversion} The booking site shows the final price.").into(),
                as_of: Some(crate::commands::today_utc()),
                page: Some(request.booking_link.clone()),
            },
        },
    }
}

/// Looks up the live price of a ticket by reading the site's page in the web view.
///
/// PKP Intercity shows second class prices on its list, so a first class ticket gets no live
/// price there.
async fn look_up_in_web_view(
    state: &AppState,
    request: &LiveQuoteRequest,
    site: BookingSite,
) -> (LiveQuoteOutcome, Option<serde_json::Value>) {
    let Some(service) = &state.webview_service else {
        return (LiveQuoteOutcome::NoService, None);
    };
    if request.travel_class == TravelClass::First && site == BookingSite::Pkp {
        return (LiveQuoteOutcome::NoPriceForClass, None);
    }
    let Some(departure) = crate::booking_links::search_time(&request.booking_link) else {
        return (LiveQuoteOutcome::UnsupportedLink, None);
    };
    let cards = match service.cards(&request.booking_link).await {
        Ok(cards) => cards,
        Err(error) => {
            tracing::info!(%error, "no live price from a booking page");
            return (LiveQuoteOutcome::Failed { error }, None);
        }
    };
    let answer = serde_json::to_value(&cards).ok();
    let Some(card) =
        crate::webview_quotes::planned_card(&cards, &departure, usize::from(request.ride_count))
    else {
        return (LiveQuoteOutcome::NoMatchingTrip, answer);
    };
    let Some(amount) = card.price else {
        return (LiveQuoteOutcome::NotSold, answer);
    };
    let trains = if card.trains.is_empty() {
        format!("the {departure} connection")
    } else {
        card.trains.join(" and ")
    };
    let note = format!(
        "The lowest price the site shows for {trains}, for one traveller with no discount card. \
         It can be a saver fare that ties the ticket to the train."
    );
    let currency = card.currency.as_deref().unwrap_or("EUR");
    (
        live_price(state, request, site, amount, currency, &note),
        answer,
    )
}

/// Looks up the live price of a ticket on a site that answers a search with its connections,
/// from the search in the request.
async fn look_up_on_site(
    state: &AppState,
    request: &LiveQuoteRequest,
    site: BookingSite,
) -> (LiveQuoteOutcome, Option<serde_json::Value>) {
    let Some(services) = &state.site_services else {
        return (LiveQuoteOutcome::NoService, None);
    };
    let Some(search) = crate::site_quotes::connection_search(site, request) else {
        return (LiveQuoteOutcome::UnsupportedLink, None);
    };
    let connections = match services.connections(site, &search).await {
        None => return (LiveQuoteOutcome::NoService, None),
        Some(Ok(connections)) => connections,
        Some(Err(error)) => {
            tracing::info!(%error, site = site_name(site), "no live price");
            return (
                LiveQuoteOutcome::Failed {
                    error: error.to_string(),
                },
                None,
            );
        }
    };
    let answer = serde_json::to_value(&connections).ok();
    let Some(connection) = btt_quotes::find_connection(&connections, &search) else {
        return (LiveQuoteOutcome::NoMatchingTrip, answer);
    };
    if connection.not_for_sale {
        return (LiveQuoteOutcome::NotSold, answer);
    }
    let Some(amount) = crate::site_quotes::class_price(connection, request.travel_class) else {
        return (LiveQuoteOutcome::NoPriceForClass, answer);
    };
    let note = format!(
        "The lowest price the site shows for {} in {} class, for one adult with no discount \
         card. It can be a saver fare that ties the ticket to the train.",
        connection.trains.join(" and "),
        match request.travel_class {
            TravelClass::First => "first",
            TravelClass::Second => "second",
        }
    );
    (
        live_price(state, request, site, amount, &connection.currency, &note),
        answer,
    )
}

/// Looks up the live price of a ticket on the service of the DB app, from the stations, time,
/// class and traveller of its bahn.de link.
async fn look_up_db(
    state: &AppState,
    request: &LiveQuoteRequest,
) -> (LiveQuoteOutcome, Option<serde_json::Value>) {
    let Some(service) = &state.db_service else {
        return (LiveQuoteOutcome::NoService, None);
    };
    let link = &request.booking_link;
    let traveller_type = parameter(link, "r")
        .and_then(|travellers| travellers.split(':').next().map(str::to_owned))
        .and_then(|id| btt_quotes::db::traveller_type(&id));
    let departure =
        chrono::DateTime::from_timestamp(i64::from(request.first_departure_time.0) * 60, 0)
            .map(|moment| moment.fixed_offset());
    let (Some(origin_eva), Some(destination_eva), Some(traveller_type), Some(departure)) = (
        parameter(link, "soei"),
        parameter(link, "zoei"),
        traveller_type,
        departure,
    ) else {
        return (LiveQuoteOutcome::UnsupportedLink, None);
    };
    let search = btt_quotes::db::Search {
        origin_eva,
        destination_eva,
        departure,
        class: match request.travel_class {
            TravelClass::First => "KLASSE_1",
            TravelClass::Second => "KLASSE_2",
        }
        .to_owned(),
        traveller_type: traveller_type.to_owned(),
    };
    let connections = match service.connections(&search).await {
        Ok(connections) => connections,
        Err(error) => {
            tracing::info!(%error, "no live price from the DB app service");
            return (
                LiveQuoteOutcome::Failed {
                    error: error.to_string(),
                },
                None,
            );
        }
    };
    let answer = serde_json::to_value(&connections).ok();
    let Some(connection) = btt_quotes::db::find_connection(
        &connections,
        departure.timestamp(),
        usize::from(request.ride_count),
    ) else {
        return (LiveQuoteOutcome::NoMatchingTrip, answer);
    };
    let Some(amount) = connection.price else {
        return (LiveQuoteOutcome::NotSold, answer);
    };
    if connection.partial_price {
        return (LiveQuoteOutcome::PartialPrice, answer);
    }
    let note = format!(
        "The lowest price DB shows for {}, for one traveller with no discount card. It can be a \
         saver fare that ties the ticket to the train.",
        connection.trains.join(" and ")
    );
    (
        live_price(state, request, BookingSite::Db, amount, "EUR", &note),
        answer,
    )
}

/// Looks up the live price of a ticket on the service of the ÖBB shop, from the stations and
/// time of its shop link. The shop lists second class prices, so a first class ticket gets none.
async fn look_up_obb(
    state: &AppState,
    request: &LiveQuoteRequest,
) -> (LiveQuoteOutcome, Option<serde_json::Value>) {
    let Some(service) = &state.obb_service else {
        return (LiveQuoteOutcome::NoService, None);
    };
    if request.travel_class == TravelClass::First {
        return (LiveQuoteOutcome::NoPriceForClass, None);
    }
    let link = &request.booking_link;
    let number = |name: &str| parameter(link, name).and_then(|value| value.parse().ok());
    let departure = parameter(link, "outwardDateTime")
        .and_then(|text| chrono::NaiveDateTime::parse_from_str(&text, "%Y-%m-%dT%H:%M").ok());
    let (Some(origin), Some(destination), Some(departure)) = (
        number("stationOrigEva"),
        number("stationDestEva"),
        departure,
    ) else {
        return (LiveQuoteOutcome::UnsupportedLink, None);
    };
    let search = btt_quotes::obb::Search {
        origin,
        destination,
        departure,
    };
    let connections = match service.connections(&search).await {
        Ok(connections) => connections,
        Err(error) => {
            tracing::info!(%error, "no live price from the ÖBB shop");
            return (
                LiveQuoteOutcome::Failed {
                    error: error.to_string(),
                },
                None,
            );
        }
    };
    let answer = serde_json::to_value(&connections).ok();
    let Some(connection) =
        btt_quotes::obb::find_connection(&connections, departure, usize::from(request.ride_count))
    else {
        return (LiveQuoteOutcome::NoMatchingTrip, answer);
    };
    let Some(amount) = connection.price else {
        return (LiveQuoteOutcome::NotSold, answer);
    };
    let kind = connection
        .note
        .as_deref()
        .map_or_else(String::new, |note| format!(" It is a {note}."));
    let note = format!(
        "The lowest price the ÖBB shop shows for {}, for one adult with no discount card.{kind}",
        connection.trains.join(" and ")
    );
    (
        live_price(state, request, BookingSite::Obb, amount, "EUR", &note),
        answer,
    )
}

/// Looks up the live price of a ticket on the Trenitalia site, from the stations, date, time and
/// travellers of its handoff link.
async fn look_up_trenitalia(
    state: &AppState,
    request: &LiveQuoteRequest,
) -> (LiveQuoteOutcome, Option<serde_json::Value>) {
    let Some(service) = &state.trenitalia_service else {
        return (LiveQuoteOutcome::NoService, None);
    };
    if request.travel_class == TravelClass::First {
        return (LiveQuoteOutcome::NoPriceForClass, None);
    }
    let link = &request.booking_link;
    let location = |name: Option<String>| {
        let name = name?;
        state
            .fare_data
            .trenitalia_stations
            .stations
            .iter()
            .find(|station| station.name == name)
            .and_then(|station| station.code.parse::<u64>().ok())
    };
    let count = |name: &str| parameter(link, name).and_then(|count| count.parse().ok());
    let departure = parameter(link, "departureDate")
        .zip(parameter(link, "departureTime"))
        .and_then(|(date, time)| {
            chrono::NaiveDateTime::parse_from_str(&format!("{date} {time}"), "%d-%m-%Y %H:%M").ok()
        });
    let (Some(origin_id), Some(destination_id), Some(departure)) = (
        location(parameter(link, "departureStation")),
        location(parameter(link, "arrivalStation")),
        departure,
    ) else {
        return (LiveQuoteOutcome::UnsupportedLink, None);
    };
    let search = btt_quotes::trenitalia::Search {
        origin_id,
        destination_id,
        departure,
        adults: count("noOfAdults").unwrap_or(1),
        children: count("noOfChildren").unwrap_or(0),
    };
    let solutions = match service.solutions(&search).await {
        Ok(solutions) => solutions,
        Err(error) => {
            tracing::info!(%error, "no live price from the Trenitalia site");
            return (
                LiveQuoteOutcome::Failed {
                    error: error.to_string(),
                },
                None,
            );
        }
    };
    let answer = serde_json::to_value(&solutions).ok();
    let departure_seconds = i64::from(request.first_departure_time.0) * 60;
    let Some(solution) = btt_quotes::trenitalia::find_solution(
        &solutions,
        departure_seconds,
        usize::from(request.ride_count),
    ) else {
        return (LiveQuoteOutcome::NoMatchingTrip, answer);
    };
    let Some(amount) = solution.price else {
        return (LiveQuoteOutcome::NotSold, answer);
    };
    let note = format!(
        "The lowest price the site shows for {}. It can be an Economy or Super Economy fare \
         that ties the ticket to the train.",
        solution.trains.join(" and ")
    );
    (
        live_price(
            state,
            request,
            BookingSite::Trenitalia,
            amount,
            "EUR",
            &note,
        ),
        answer,
    )
}

/// Looks up the fare of a single on ns.nl. NS fares are the same on every train of the day, so
/// no train is matched.
async fn look_up_ns(
    state: &AppState,
    request: &LiveQuoteRequest,
) -> (LiveQuoteOutcome, Option<serde_json::Value>) {
    let Some(service) = &state.ns_service else {
        return (LiveQuoteOutcome::NoService, None);
    };
    let link = &request.booking_link;
    let date = parameter(link, "travelDate").and_then(|date| date.parse().ok());
    let (Some(from), Some(to), Some(date)) = (
        parameter(link, "fromUicCode"),
        parameter(link, "toUicCode"),
        date,
    ) else {
        return (LiveQuoteOutcome::UnsupportedLink, None);
    };
    let fare = match service.single_fare(&from, &to, date).await {
        Ok(fare) => fare,
        Err(error) => {
            tracing::info!(%error, "no fare from NS");
            return (
                LiveQuoteOutcome::Failed {
                    error: error.to_string(),
                },
                None,
            );
        }
    };
    let answer = serde_json::to_value(fare).ok();
    let cents = match request.travel_class {
        TravelClass::First => fare.first_class_cents,
        TravelClass::Second => fare.second_class_cents,
    };
    let Some(cents) = cents else {
        return (LiveQuoteOutcome::NoPriceForClass, answer);
    };
    let price = Price {
        low: EuroCents(cents),
        high: EuroCents(cents),
        kind: PriceKind::Formula,
        source: PriceSource {
            name: site_name(BookingSite::Ns).into(),
            note: "The fixed NS fare of a single, valid on every train of the day. Supplements \
                   for the Intercity direct and the ICE are not included."
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
    let cents = EuroCents::from_euro(francs * state.fare_data.exchange_rates.chf_to_eur);
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
                state.fare_data.exchange_rates.chf_to_eur
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
            site_search: None,
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
