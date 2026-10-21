//! The booking sites that answer a search with their connections, and the live price lookup
//! that they share.

use std::sync::Arc;

use btt_model::{LiveQuoteRequest, TravelClass};
use btt_quotes::dsb::Dsb;
use btt_quotes::eurostar::Eurostar;
use btt_quotes::italo::Italo;
use btt_quotes::koleo::Koleo;
use btt_quotes::mav::Mav;
use btt_quotes::regiojet::Regiojet;
use btt_quotes::renfe::Renfe;
use btt_quotes::sj::Sj;
use btt_quotes::snalltaget::Snalltaget;
use btt_quotes::{ConnectionSearch, QuoteError, QuoteStore, SiteConnection, SiteService};

use crate::booking_links::BookingSite;

/// The services of the booking sites that answer a search with their connections.
pub struct SiteServices {
    /// DSB.
    dsb: SiteService<Dsb>,
    /// SJ.
    sj: SiteService<Sj>,
    /// Snälltåget.
    snalltaget: SiteService<Snalltaget>,
    /// MÁV.
    mav: SiteService<Mav>,
    /// Italo.
    italo: SiteService<Italo>,
    /// Renfe.
    renfe: SiteService<Renfe>,
    /// RegioJet.
    regiojet: SiteService<Regiojet>,
    /// Eurostar.
    eurostar: SiteService<Eurostar>,
    /// Koleo.
    koleo: SiteService<Koleo>,
}

impl SiteServices {
    /// Creates the services, which keep their answers in a store.
    ///
    /// # Errors
    ///
    /// Returns an error when an HTTP client cannot be built.
    pub fn new(store: &Arc<dyn QuoteStore>) -> Result<Self, QuoteError> {
        Ok(Self {
            dsb: SiteService::new(Dsb, Arc::clone(store))?,
            sj: SiteService::new(Sj, Arc::clone(store))?,
            snalltaget: SiteService::new(Snalltaget::default(), Arc::clone(store))?,
            mav: SiteService::new(Mav, Arc::clone(store))?,
            italo: SiteService::new(Italo::default(), Arc::clone(store))?,
            renfe: SiteService::new(Renfe, Arc::clone(store))?,
            regiojet: SiteService::new(Regiojet, Arc::clone(store))?,
            eurostar: SiteService::new(Eurostar, Arc::clone(store))?,
            koleo: SiteService::new(Koleo::default(), Arc::clone(store))?,
        })
    }

    /// Returns the connections of a search on a site, or `None` for a site these services do
    /// not read.
    pub async fn connections(
        &self,
        site: BookingSite,
        search: &ConnectionSearch,
    ) -> Option<Result<Vec<SiteConnection>, QuoteError>> {
        Some(match site {
            BookingSite::Dsb => self.dsb.connections(search).await,
            BookingSite::Sj => self.sj.connections(search).await,
            BookingSite::Snalltaget => self.snalltaget.connections(search).await,
            BookingSite::Mav => self.mav.connections(search).await,
            BookingSite::Italo => self.italo.connections(search).await,
            BookingSite::Renfe => self.renfe.connections(search).await,
            BookingSite::Regiojet => self.regiojet.connections(search).await,
            BookingSite::Eurostar => self.eurostar.connections(search).await,
            BookingSite::Koleo => self.koleo.connections(search).await,
            _ => return None,
        })
    }
}

/// Returns the search for a live price request that names one, or `None` when it names none
/// or its departure cannot be read.
///
/// Snälltåget splits and joins its trains differently from the timetable, so any number of
/// trains matches there.
pub fn connection_search(
    site: BookingSite,
    request: &LiveQuoteRequest,
) -> Option<ConnectionSearch> {
    let search = request.site_search.as_ref()?;
    let departure =
        chrono::NaiveDateTime::parse_from_str(&search.local_departure, "%Y-%m-%dT%H:%M").ok()?;
    let departure_moment =
        chrono::DateTime::from_timestamp(i64::from(request.first_departure_time.0) * 60, 0)?;
    Some(ConnectionSearch {
        origin: search.origin_code.to_string(),
        destination: search.destination_code.to_string(),
        departure,
        departure_moment,
        train_count: (site != BookingSite::Snalltaget).then_some(usize::from(request.ride_count)),
    })
}

/// Returns the lowest price of a connection in the class of a request.
pub fn class_price(connection: &SiteConnection, class: TravelClass) -> Option<f64> {
    match class {
        TravelClass::First => connection.first_class_price,
        TravelClass::Second => connection.second_class_price,
    }
}

#[cfg(test)]
mod tests {
    use btt_model::{SiteSearch, UnixTimeMinutes};

    use super::*;

    #[test]
    fn a_request_turns_into_the_search_of_its_site() {
        let request = LiveQuoteRequest {
            booking_link: "https://www.dsb.dk/en/".into(),
            first_departure_time: UnixTimeMinutes(29_920_052),
            ride_count: 2,
            train_labels: Vec::new(),
            travel_class: TravelClass::Second,
            site_search: Some(SiteSearch {
                origin_code: "8600626".into(),
                destination_code: "8600512".into(),
                local_departure: "2026-11-20T09:32".into(),
            }),
            run: None,
            refresh: false,
            accommodation: None,
        };
        let search = connection_search(BookingSite::Dsb, &request).unwrap();
        assert_eq!(search.origin, "8600626");
        assert_eq!(search.train_count, Some(2));
        assert_eq!(search.departure.format("%H:%M").to_string(), "09:32");
        assert_eq!(search.departure_moment.timestamp(), 29_920_052 * 60);
        let snalltaget = connection_search(BookingSite::Snalltaget, &request).unwrap();
        assert_eq!(snalltaget.train_count, None);
        let without_search = LiveQuoteRequest {
            site_search: None,
            ..request
        };
        assert!(connection_search(BookingSite::Dsb, &without_search).is_none());
    }
}
