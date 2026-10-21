//! What the app keeps about ticket prices for problem reports: the last ticket plan of each open
//! trip with the journeys it was made from, and a log of the live price lookups with the raw
//! answers of the booking sites. Lookups go to the booking site that the address of the
//! booking link names.

use btt_model::{
    AccommodationRequest, EuroCents, ForeignAmount, LiveQuoteRequest, Price, PriceKind,
    PriceSource, TicketLeg, TicketPlan, TicketPreferences, TravelClass, Traveller,
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
    /// What the price model estimated for the ticket before it learned from the live price,
    /// so a report shows how far off the model is.
    pub model_estimate: Option<Price>,
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
        Some(BookingSite::Obb) => match &request.accommodation {
            Some(accommodation) => look_up_obb_accommodation(state, request, accommodation).await,
            None => look_up_obb(state, request).await,
        },
        Some(BookingSite::EuropeanSleeper) => look_up_european_sleeper(state, request).await,
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
        BookingSite::Regiojet => "RegioJet booking site",
        BookingSite::Koleo => "Koleo booking site",
        BookingSite::EuropeanSleeper => "European Sleeper booking site",
    }
}

/// Returns a live price in euro from an amount in a currency, with a note on its source and the
/// amount as the site gave it, or the outcome for a currency the app does not convert.
async fn live_price(
    state: &AppState,
    request: &LiveQuoteRequest,
    site: BookingSite,
    amount: f64,
    currency: &str,
    note: &str,
) -> LiveQuoteOutcome {
    let Some(rate) = state.euro_per(currency).await else {
        return LiveQuoteOutcome::UnknownCurrency {
            currency: currency.to_owned(),
        };
    };
    let original = (currency != "EUR").then(|| ForeignAmount {
        // Prices on booking sites are far inside the range of `u32` hundredths.
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        hundredths: (amount * 100.0).round() as u32,
        currency: currency.into(),
    });
    LiveQuoteOutcome::Quoted {
        price: Price {
            low: EuroCents::from_euro(amount * rate),
            high: EuroCents::from_euro(amount * rate),
            kind: PriceKind::Live,
            source: PriceSource {
                name: site_name(site).into(),
                note: format!("{note} The booking site shows the final price.").into(),
                as_of: Some(crate::commands::today_utc()),
                page: Some(request.booking_link.clone()),
            },
            original,
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
        live_price(state, request, site, amount, currency, &note).await,
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
        live_price(state, request, site, amount, &connection.currency, &note).await,
        answer,
    )
}

/// Returns the departure with the UTC offset of the station.
///
/// The service reads the clock of `reiseDatum` as local time and ignores its offset, so a
/// departure in UTC would search two hours too early in summer. The local clock comes from the
/// `hd` parameter of the booking link, written in local time. A link without it gets UTC.
fn db_departure(
    unix_seconds: i64,
    link_time: Option<&str>,
) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    let moment = chrono::DateTime::from_timestamp(unix_seconds, 0)?;
    let local_clock = link_time
        .and_then(|text| chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S").ok());
    let offset = local_clock
        .and_then(|clock| {
            let seconds = clock
                .signed_duration_since(moment.naive_utc())
                .num_seconds();
            chrono::FixedOffset::east_opt(i32::try_from(seconds).ok()?)
        })
        .unwrap_or_else(|| chrono::FixedOffset::east_opt(0).expect("zero is a valid offset"));
    Some(moment.with_timezone(&offset))
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
    let departure = db_departure(
        i64::from(request.first_departure_time.0) * 60,
        parameter(link, "hd").as_deref(),
    );
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
        &request.train_labels,
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
        live_price(state, request, BookingSite::Db, amount, "EUR", &note).await,
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
        live_price(state, request, BookingSite::Obb, amount, "EUR", &note).await,
        answer,
    )
}

/// Looks up the live price of the accommodation of a night train in the ÖBB shop. The price is
/// the range from the lowest couchette to the lowest berth, and the lowest seat when the train
/// has no beds. A traveller with a pass is priced with the Interrail card, who pays the
/// accommodation with its reservation. Any other traveller pays the extra over a seat, because
/// the ticket holds a seat.
async fn look_up_obb_accommodation(
    state: &AppState,
    request: &LiveQuoteRequest,
    accommodation: &AccommodationRequest,
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
    let offers = match service
        .night_offers(&search, &accommodation.train, accommodation.with_pass)
        .await
    {
        Ok(Some(offers)) => offers,
        Ok(None) => return (LiveQuoteOutcome::NoMatchingTrip, None),
        Err(error) => {
            tracing::info!(%error, "no live accommodation price from the ÖBB shop");
            return (
                LiveQuoteOutcome::Failed {
                    error: error.to_string(),
                },
                None,
            );
        }
    };
    let answer = serde_json::to_value(&offers).ok();
    let seat_estimate = accommodation
        .seat_estimate
        .map(|cents| f64::from(cents.0) / 100.0);
    let seatless = !accommodation.with_pass && !offers.sells_seats();
    let lowest = |kind| accommodation_extra(&offers, kind, accommodation.with_pass, seat_estimate);
    let (couchette, berth, seat) = (lowest("COUCHETTE"), lowest("BERTH"), lowest("SEAT"));
    let Some((low, high)) = night_range(couchette, berth, seat, accommodation.with_pass) else {
        return (LiveQuoteOutcome::NotSold, answer);
    };
    let euro = |price: Option<f64>, name: &str| price.map(|price| format!(" {name} {price:.2}"));
    let kinds: String = [
        euro(seat, "seat"),
        euro(couchette, "couchette"),
        euro(berth, "berth"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(",");
    let holds = if accommodation.with_pass {
        "the accommodation with its reservation, for a holder of an Interrail Global Pass"
            .to_owned()
    } else if seatless {
        format!(
            "the accommodation on top of the ticket. This train sells no seats, so each price is \
             the lowest price of the bed less {:.2} euro, the usual price of a seat",
            seat_estimate.unwrap_or_default()
        )
    } else {
        "the accommodation on top of the ticket, which holds a seat".to_owned()
    };
    let note = format!(
        "The lowest price of each kind on {} in the ÖBB shop, in euro:{kinds}. This is {holds}. \n         The booking site shows the final price.",
        offers.train
    );
    let price = Price {
        low: EuroCents::from_euro(low),
        high: EuroCents::from_euro(high),
        kind: PriceKind::Live,
        source: PriceSource {
            name: site_name(BookingSite::Obb).into(),
            note: note.into(),
            as_of: Some(crate::commands::today_utc()),
            page: Some(request.booking_link.clone()),
        },
        original: None,
    };
    (LiveQuoteOutcome::Quoted { price }, answer)
}

/// Returns the lowest price in euro that a traveller pays for a kind of accommodation on a night
/// train.
///
/// A train without seats sells its beds at the price of the whole ticket, and its ticket item is
/// already priced as a seat. So for a traveller without a pass the accommodation is the bed price
/// less the estimated price of that seat, and it is unknown without an estimate.
fn accommodation_extra(
    offers: &btt_quotes::obb::NightOffers,
    kind: &str,
    with_pass: bool,
    seat_estimate: Option<f64>,
) -> Option<f64> {
    if with_pass || offers.sells_seats() {
        return offers.lowest_price(kind, with_pass);
    }
    let extra = offers.lowest_whole_price(kind)? - seat_estimate?;
    (extra > 0.0).then_some(extra)
}

/// Returns the range of the price of an accommodation, from its lowest to its highest kind, as
/// couchette to berth. Without beds, the range is a seat, and a train without a seat and a
/// traveller without a pass has no price, since the extra over a seat cannot be told.
fn night_range(
    couchette: Option<f64>,
    berth: Option<f64>,
    seat: Option<f64>,
    with_pass: bool,
) -> Option<(f64, f64)> {
    match (couchette, berth) {
        (Some(couchette), Some(berth)) => Some((couchette.min(berth), couchette.max(berth))),
        (Some(price), None) | (None, Some(price)) => Some((price, price)),
        (None, None) => seat.filter(|_| with_pass).map(|price| (price, price)),
    }
}

/// Returns the search on the booking service of European Sleeper that a request names in its
/// site search, or \`None\` when it names none or its departure cannot be read.
fn sleeper_search(request: &LiveQuoteRequest) -> Option<btt_quotes::european_sleeper::Search> {
    let search = request.site_search.as_ref()?;
    Some(btt_quotes::european_sleeper::Search {
        origin: search.origin_code.to_string(),
        destination: search.destination_code.to_string(),
        departure: chrono::NaiveDateTime::parse_from_str(&search.local_departure, "%Y-%m-%dT%H:%M")
            .ok()?,
    })
}

/// Looks up the live price of a ticket, or of the accommodation of a night ride when the request
/// names one, on the booking service of European Sleeper. The service lists second class only.
async fn look_up_european_sleeper(
    state: &AppState,
    request: &LiveQuoteRequest,
) -> (LiveQuoteOutcome, Option<serde_json::Value>) {
    let Some(service) = &state.european_sleeper_service else {
        return (LiveQuoteOutcome::NoService, None);
    };
    if request.travel_class == TravelClass::First {
        return (LiveQuoteOutcome::NoPriceForClass, None);
    }
    let Some(search) = sleeper_search(request) else {
        return (LiveQuoteOutcome::UnsupportedLink, None);
    };
    let offers = match service.offers(&search).await {
        Ok(Some(offers)) => offers,
        Ok(None) => return (LiveQuoteOutcome::NoMatchingTrip, None),
        Err(error) => {
            tracing::info!(%error, "no live price from European Sleeper");
            return (
                LiveQuoteOutcome::Failed {
                    error: error.to_string(),
                },
                None,
            );
        }
    };
    let answer = serde_json::to_value(&offers).ok();
    if !offers.bookable {
        return (LiveQuoteOutcome::NotSold, answer);
    }
    let outcome = match &request.accommodation {
        Some(accommodation) => {
            sleeper_accommodation_outcome(&offers, request, accommodation.with_pass)
        }
        None => sleeper_ticket_outcome(state, request, &offers).await,
    };
    (outcome, answer)
}

/// Returns the price of the cheapest ticket of a European Sleeper train for one adult, which is the
/// cheapest place the train sells, with a note on what it holds.
async fn sleeper_ticket_outcome(
    state: &AppState,
    request: &LiveQuoteRequest,
    offers: &btt_quotes::european_sleeper::SleeperOffers,
) -> LiveQuoteOutcome {
    use btt_quotes::european_sleeper::PlaceKind;

    // A night ride is a ticket for a bed, which the service sells with the ticket. A run that is
    // known to have no berths gets the cheapest place instead.
    let night = request
        .run
        .as_ref()
        .is_none_or(|run| run.rides.iter().any(|ride| ride.offers_berths));
    let beds = night_range(
        offers.whole_price(PlaceKind::Couchette),
        offers.whole_price(PlaceKind::Berth),
        None,
        false,
    );
    if let (true, Some((low, high))) = (night, beds) {
        let note = format!(
            "The lowest price of a ticket with a bed on {} on the booking site of European \
             Sleeper, from a bed in a shared compartment to a bed in a small one. The ticket \
             holds the bed, so the accommodation costs nothing on top. The booking site shows \
             the final price.",
            offers.train
        );
        return LiveQuoteOutcome::Quoted {
            price: Price {
                low: EuroCents::from_euro(low),
                high: EuroCents::from_euro(high),
                kind: PriceKind::Live,
                source: PriceSource {
                    name: site_name(BookingSite::EuropeanSleeper).into(),
                    note: note.into(),
                    as_of: Some(crate::commands::today_utc()),
                    page: Some(request.booking_link.clone()),
                },
                original: None,
            },
        };
    }
    let Some(amount) = offers.cheapest_ticket() else {
        return LiveQuoteOutcome::NotSold;
    };
    let holds = if offers.last_minute == Some(amount) {
        "It is the last minute fare, which holds a bed in a shared compartment of five."
    } else if offers.adult.seat == Some(amount) {
        "It is the price of a seat."
    } else if offers.whole_price(PlaceKind::Couchette) == Some(amount) {
        "The train sells no cheaper place than a bed in a shared compartment of five."
    } else {
        "The train sells no cheaper place than a bed in a small compartment."
    };
    let note = format!(
        "The lowest price the booking site shows for {}, for one adult. {holds}",
        offers.train
    );
    live_price(
        state,
        request,
        BookingSite::EuropeanSleeper,
        amount,
        "EUR",
        &note,
    )
    .await
}

/// Returns the price of the accommodation of a European Sleeper train, as a range from the lowest
/// bed in a shared compartment to the lowest bed in a small one.
///
/// A pass holder pays the reservation of the place, or the price of the place with the ticket when
/// that is lower, such as a last minute fare. Without a pass, the accommodation is what a place
/// costs on top of the cheapest ticket, since that ticket is a separate item of the plan.
fn sleeper_accommodation_outcome(
    offers: &btt_quotes::european_sleeper::SleeperOffers,
    request: &LiveQuoteRequest,
    with_pass: bool,
) -> LiveQuoteOutcome {
    use btt_quotes::european_sleeper::PlaceKind;

    let price = |kind| {
        if with_pass {
            offers.pass_price(kind)
        } else {
            offers.extra_over_ticket(kind)
        }
    };
    let (seat, couchette, berth) = (
        price(PlaceKind::Seat),
        price(PlaceKind::Couchette),
        price(PlaceKind::Berth),
    );
    let Some((low, high)) = night_range(couchette, berth, seat, with_pass) else {
        return LiveQuoteOutcome::NotSold;
    };
    let euro = |price: Option<f64>, name: &str| price.map(|price| format!(" {name} {price:.2}"));
    let kinds: String = [
        euro(seat.filter(|_| with_pass), "seat"),
        euro(couchette, "bed in a shared compartment"),
        euro(berth, "bed in a small compartment"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(",");
    let cheaper = [PlaceKind::Couchette, PlaceKind::Berth, PlaceKind::Seat]
        .into_iter()
        .any(|kind| with_pass && offers.is_cheaper_without_pass(kind));
    let holds = if with_pass {
        let mut text = "the reservation of the place, for a holder of an Interrail pass".to_owned();
        if cheaper {
            text.push_str(
                ". A place with a regular ticket costs less than its reservation on some kinds, \
                 such as a last minute fare, and those prices are that ticket",
            );
        }
        text
    } else {
        let ticket = offers.cheapest_ticket().unwrap_or_default();
        format!(
            "the place on top of the cheapest ticket of {ticket:.2} euro, which is a separate item"
        )
    };
    let note = format!(
        "The lowest price of each kind of place on {} on the booking site of European Sleeper, in \
         euro:{kinds}. This is {holds}. \n         The booking site shows the final price.",
        offers.train
    );
    LiveQuoteOutcome::Quoted {
        price: Price {
            low: EuroCents::from_euro(low),
            high: EuroCents::from_euro(high),
            kind: PriceKind::Live,
            source: PriceSource {
                name: site_name(BookingSite::EuropeanSleeper).into(),
                note: note.into(),
                as_of: Some(crate::commands::today_utc()),
                page: Some(request.booking_link.clone()),
            },
            original: None,
        },
    }
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
        )
        .await,
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
        original: None,
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
    let amount = f64::from(class_price.price.amount) / 100.0;
    let note = format!(
        "The price for {} class, {}.",
        class.to_lowercase(),
        class_price
            .after_sale_flexibility
            .to_lowercase()
            .replace('_', " "),
    );
    let currency = class_price.price.currency.clone();
    (
        live_price(state, request, BookingSite::Sbb, amount, &currency, &note).await,
        Some(quotes),
    )
}

/// Adds a lookup to the log, dropping the oldest when the log is full.
pub fn log_lookup(
    log: &mut Vec<LiveQuoteLogEntry>,
    request: &LiveQuoteRequest,
    outcome: LiveQuoteOutcome,
    site_answer: Option<serde_json::Value>,
    model_estimate: Option<Price>,
) {
    if log.len() >= MAXIMUM_LOGGED_LOOKUPS {
        log.remove(0);
    }
    log.push(LiveQuoteLogEntry {
        at: chrono::Local::now().to_rfc3339(),
        request: request.clone(),
        outcome,
        site_answer,
        model_estimate,
    });
}

#[cfg(test)]
mod tests {
    use btt_model::{TravelClass, UnixTimeMinutes};

    use super::*;

    #[test]
    fn the_db_departure_has_the_offset_of_the_local_clock_in_the_link() {
        // 10:18 UTC is 12:18 in Berlin on 2026-10-24.
        let unix_seconds = 29_880_618 * 60;
        let summer = db_departure(unix_seconds, Some("2026-10-24T12:18:00")).unwrap();
        assert_eq!(summer.to_rfc3339(), "2026-10-24T12:18:00+02:00");
        let without_link = db_departure(unix_seconds, None).unwrap();
        assert_eq!(without_link.to_rfc3339(), "2026-10-24T10:18:00+00:00");
        assert_eq!(summer.timestamp(), without_link.timestamp());
    }

    fn request(minute: u32) -> LiveQuoteRequest {
        LiveQuoteRequest {
            booking_link: "https://www.sbb.ch/en".into(),
            first_departure_time: UnixTimeMinutes(minute),
            ride_count: 1,
            train_labels: Vec::new(),
            travel_class: TravelClass::Second,
            site_search: None,
            run: None,
            refresh: false,
            accommodation: None,
        }
    }

    #[test]
    fn the_range_of_an_accommodation_runs_from_the_couchette_to_the_berth() {
        assert_eq!(
            night_range(Some(60.0), Some(140.0), Some(5.0), true),
            Some((60.0, 140.0))
        );
        // A train with one kind of bed has no range.
        assert_eq!(
            night_range(None, Some(140.0), None, false),
            Some((140.0, 140.0))
        );
        // A pass holder on a train with seats only pays the reservation of a seat.
        assert_eq!(night_range(None, None, Some(9.9), true), Some((9.9, 9.9)));
        // Without a pass, the extra over a seat of a train without beds is nothing to show.
        assert_eq!(night_range(None, None, Some(9.9), false), None);
        // A range is never upside down.
        assert_eq!(
            night_range(Some(150.0), Some(90.0), None, true),
            Some((90.0, 150.0))
        );
    }

    /// The offers of the Nightjet from Amsterdam to Basel, which sells beds and no seats.
    fn seatless_offers() -> btt_quotes::obb::NightOffers {
        let bed = |kind: &str, non_flex: f64, semi_flex: f64| btt_quotes::obb::Accommodation {
            kind: kind.to_owned(),
            name: "Compartment".to_owned(),
            prices: [
                ("NON-FLEX".to_owned(), non_flex),
                ("SEMI-FLEX".to_owned(), semi_flex),
            ]
            .into(),
        };
        btt_quotes::obb::NightOffers {
            train: "NJ 403".to_owned(),
            accommodations: vec![
                bed("COUCHETTE", 94.9, 114.9),
                bed("COUCHETTE", 114.9, 134.9),
                bed("BERTH", 134.9, 174.9),
            ],
        }
    }

    #[test]
    fn the_accommodation_of_a_train_without_seats_is_the_bed_less_a_seat() {
        let offers = seatless_offers();
        let couchette = accommodation_extra(&offers, "COUCHETTE", false, Some(35.0)).unwrap();
        assert!((couchette - 59.9).abs() < 1e-9, "{couchette}");
        let berth = accommodation_extra(&offers, "BERTH", false, Some(35.0)).unwrap();
        assert!((berth - 99.9).abs() < 1e-9, "{berth}");
        // Without an estimate of the seat the extra is unknown.
        assert_eq!(accommodation_extra(&offers, "BERTH", false, None), None);
        // A bed cheaper than the seat estimate is no extra.
        assert_eq!(
            accommodation_extra(&offers, "COUCHETTE", false, Some(120.0)),
            None
        );
        // A pass holder pays the listed reservation and needs no estimate.
        let pass = accommodation_extra(&offers, "COUCHETTE", true, None).unwrap();
        assert!((pass - 94.9).abs() < 1e-9, "{pass}");
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
            None,
        );
        let json = serde_json::to_string(&log).unwrap();
        assert!(json.contains(r#""Failed":{"error":"refused"}"#));
        assert!(json.contains(r#""booking_link":"https://www.sbb.ch/en""#));
    }
}
