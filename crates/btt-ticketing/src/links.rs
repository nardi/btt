//! Links to the places where tickets are sold.

use std::fmt::Write as _;

use btt_fares::{ItaloStations, StationTable};
use btt_model::{AgeGroup, BookingLink, BookingLinkKind, TravelClass};
use chrono::NaiveDate;

use crate::rides::RideInput;

/// The page that sells the Interrail Global Pass.
pub const INTERRAIL_PASS_PAGE: &str = "https://www.interrail.eu/en/interrail-passes/global-pass";

/// Returns the shop where a single ticket for a ride that starts in a country is bought. These
/// are the start pages of the shops, where the traveller enters the search.
pub fn shop_link(country: Option<&str>) -> BookingLink {
    let (label, url) = match country {
        Some("DE") => ("Open the Deutsche Bahn shop", "https://int.bahn.de/en"),
        Some("AT") => ("Open the ÖBB shop", "https://shop.oebb.at/en/"),
        Some("CH") => ("Open the SBB shop", "https://www.sbb.ch/en"),
        Some("FR") => ("Open SNCF Connect", "https://www.sncf-connect.com/en-en/"),
        Some("IT") => (
            "Open the Trenitalia shop",
            "https://www.trenitalia.com/en.html",
        ),
        Some("ES") => ("Open the Renfe shop", "https://www.renfe.com/es/en"),
        Some("NL") => ("Open the NS shop", "https://www.ns.nl/en"),
        Some("BE") => ("Open the SNCB shop", "https://www.belgiantrain.be/en"),
        Some("CZ") => ("Open the České dráhy shop", "https://www.cd.cz/en/"),
        Some("PL") => (
            "Open the PKP Intercity shop",
            "https://www.intercity.pl/en/",
        ),
        Some("HU") => ("Open the MÁV shop", "https://jegy.mav.hu/"),
        Some("SK") => ("Open the ZSSK shop", "https://predaj.zssk.sk/"),
        Some("DK") => ("Open the DSB shop", "https://www.dsb.dk/en/"),
        Some("SE") => ("Open the SJ shop", "https://www.sj.se/en"),
        Some("NO") => ("Open the Vy shop", "https://www.vy.no/en"),
        Some("GB") => ("Open National Rail", "https://www.nationalrail.co.uk/"),
        _ => ("Open Trainline", "https://www.thetrainline.com/"),
    };
    BookingLink {
        label: label.into(),
        url: url.into(),
        kind: BookingLinkKind::Search,
    }
}

/// How many travellers of each fare category a booking link asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Passengers {
    /// Adults.
    pub adults: u8,
    /// Young travellers.
    pub youth: u8,
    /// Children.
    pub children: u8,
    /// Seniors.
    pub seniors: u8,
    /// Infants.
    pub infants: u8,
}

impl Passengers {
    /// One traveller of an age group.
    pub fn one(age_group: AgeGroup) -> Self {
        let mut passengers = Self::default();
        match age_group {
            AgeGroup::Infant => passengers.infants = 1,
            AgeGroup::Child => passengers.children = 1,
            AgeGroup::Youth => passengers.youth = 1,
            AgeGroup::Adult => passengers.adults = 1,
            AgeGroup::Senior => passengers.seniors = 1,
        }
        passengers
    }
}

/// Builds the address of a search on the Italo booking site.
///
/// The parameters are `osc` and `dsc` for the station codes, `jt` for `return` or `single`, `od`
/// and `id` for the outbound and the return date as `dd/mm/yyyy`, and the number of travellers in
/// `adt`, `yng`, `chd`, `snr` and `inf`. A search with `startSearch=true` runs when the page opens.
/// A one-way search has `jt=single` and an empty `id`. Both forms come from addresses of the
/// site.
pub fn italo_search_url(
    origin_code: &str,
    destination_code: &str,
    outbound: NaiveDate,
    return_date: Option<NaiveDate>,
    passengers: Passengers,
) -> String {
    let date = |date: NaiveDate| date.format("%d/%m/%Y").to_string();
    let journey_type = if return_date.is_some() {
        "return"
    } else {
        "single"
    };
    let return_date = return_date.map_or_else(String::new, date);
    format!(
        "https://biglietti.italotreno.com/en/booking/ricerca-treni?osc={origin_code}&dsc={destination_code}&jt={journey_type}&od={}&id={return_date}&adt={}&yng={}&chd={}&snr={}&inf={}&pet=0&promo=&lang=en&startSearch=true",
        date(outbound),
        passengers.adults,
        passengers.youth,
        passengers.children,
        passengers.seniors,
        passengers.infants,
    )
}

/// Returns a link to the Italo search for the stations of two rides, or `None` when a station
/// is not one that Italo serves.
pub fn italo_link(
    stations: &ItaloStations,
    first: &RideInput,
    last: &RideInput,
    age_group: AgeGroup,
) -> Option<BookingLink> {
    let origin = stations.station_near(first.from_coordinates)?;
    let destination = stations.station_near(last.to_coordinates)?;
    Some(BookingLink {
        label: "Search on Italo".into(),
        url: italo_search_url(
            &origin.code,
            &destination.code,
            first.departure_date,
            None,
            Passengers::one(age_group),
        )
        .into(),
        kind: BookingLinkKind::Search,
    })
}

/// Encodes a station name for a booking link: a space is `+`, and anything that is not an ASCII
/// letter or digit or one of `-._~` is `%XX`.
fn encode_name(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        match byte {
            b' ' => encoded.push('+'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            // Writing to a string never fails.
            other => {
                let _ = write!(encoded, "%{other:02X}");
            }
        }
    }
    encoded
}

/// Returns a link to the SBB search for the stations of two rides in Switzerland, or `None`
/// when a station is not a Swiss one the table knows. The link asks for the departure of the
/// first ride, so the first trip of the answer is the planned one.
pub fn sbb_link(
    stations: &StationTable,
    first: &RideInput,
    last: &RideInput,
) -> Option<BookingLink> {
    let origin = stations.station_near(first.from_coordinates)?;
    let destination = stations.station_near(last.to_coordinates)?;
    let url = format!(
        "https://www.sbb.ch/en?stops={}_I{}~{}_I{}&day={}&time={}&moment=dep&reduction=none",
        encode_name(&origin.name),
        origin.code,
        encode_name(&destination.name),
        destination.code,
        first.departure_date,
        first.departure_clock.format("%H_%M"),
    );
    Some(BookingLink {
        label: "Search on sbb.ch".into(),
        url: url.into(),
        kind: BookingLinkKind::Search,
    })
}

/// Encodes text for the fragment of a DB link: anything that is not an ASCII letter or digit or
/// one of `-._~` is `%XX`, including spaces.
fn encode_component(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            // Writing to a string never fails.
            other => {
                let _ = write!(encoded, "%{other:02X}");
            }
        }
    }
    encoded
}

/// Returns the traveller type of the DB site for an age group, as its numeric id.
fn db_traveller_type(age_group: AgeGroup) -> u8 {
    match age_group {
        AgeGroup::Infant => 8,
        AgeGroup::Child => 11,
        AgeGroup::Youth => 9,
        AgeGroup::Adult => 13,
        AgeGroup::Senior => 12,
    }
}

/// Returns a link to the DB search for the stations of two rides, or `None` when a station is
/// not one the table knows.
///
/// The link holds one traveller of the age group with no discount card, the class, and the
/// departure of the first ride as the search time, so the first connection the site shows is
/// the planned one. The site needs the names and the EVA numbers of the stations and nothing
/// else of their full location ids.
pub fn db_link(
    stations: &StationTable,
    first: &RideInput,
    last: &RideInput,
    class: TravelClass,
    age_group: AgeGroup,
) -> Option<BookingLink> {
    let origin = stations.station_near(first.from_coordinates)?;
    let destination = stations.station_near(last.to_coordinates)?;
    let location = |name: &str, code: &str| encode_component(&format!("A=1@O={name}@L={code}@"));
    let url = format!(
        "https://int.bahn.de/en/buchung/fahrplan/suche#sts=true&so={}&zo={}&kl={}&r={}:16:KLASSENLOS:1         &soid={}&zoid={}&sot=ST&zot=ST&soei={}&zoei={}&hd={}T{}:00&hza=D&hz=%5B%5D&ar=false         &s=true&d=false&mud=10&vm=00,01,02,03,04,05,06,07,08,09&fm=false&bp=false&dlt=false         &nfv=false&dltv=false",
        encode_component(&origin.name),
        encode_component(&destination.name),
        match class {
            TravelClass::First => 1,
            TravelClass::Second => 2,
        },
        db_traveller_type(age_group),
        location(&origin.name, &origin.code),
        location(&destination.name, &destination.code),
        origin.code,
        destination.code,
        first.departure_date,
        first.departure_clock.format("%H:%M"),
    );
    Some(BookingLink {
        label: "Search on bahn.de".into(),
        url: url.into(),
        kind: BookingLinkKind::Search,
    })
}

/// Returns the link to buy the Interrail Global Pass.
pub fn pass_link() -> BookingLink {
    BookingLink {
        label: "Buy on interrail.eu".into(),
        url: INTERRAIL_PASS_PAGE.into(),
        kind: BookingLinkKind::Product,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The app may only open the addresses its capability file lists, so every link has to be
    /// in it.
    #[test]
    fn every_link_is_allowed_by_the_app_capability() {
        let capability = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../src-tauri/capabilities/default.json"
        ))
        .unwrap();
        let countries = [
            "DE", "AT", "CH", "FR", "IT", "ES", "NL", "BE", "CZ", "PL", "HU", "SK", "DK", "SE",
            "NO", "GB",
        ];
        let mut urls: Vec<String> = countries
            .iter()
            .map(|country| shop_link(Some(country)).url.to_string())
            .collect();
        urls.push(shop_link(None).url.to_string());
        urls.push(pass_link().url.to_string());
        urls.push("https://www.sbb.ch/en".to_owned());
        urls.push("https://int.bahn.de/en/buchung/fahrplan/suche".to_owned());
        urls.push(italo_search_url(
            "RMT",
            "MC_",
            NaiveDate::MIN,
            None,
            Passengers::default(),
        ));
        for url in urls {
            // The capability lists patterns such as `https://shop.oebb.at/*`, which cover a host.
            let host_pattern =
                format!("{}/*", url.split('/').take(3).collect::<Vec<_>>().join("/"));
            assert!(
                capability.contains(&host_pattern),
                "{url} needs {host_pattern}"
            );
        }
    }

    #[test]
    fn the_italo_url_matches_a_return_search_of_the_site() {
        let date = |day| NaiveDate::from_ymd_opt(2026, 10, day).unwrap();
        let passengers = Passengers {
            adults: 1,
            youth: 1,
            children: 1,
            seniors: 1,
            infants: 0,
        };
        assert_eq!(
            italo_search_url("RG_", "RMT", date(16), Some(date(17)), passengers),
            "https://biglietti.italotreno.com/en/booking/ricerca-treni?osc=RG_&dsc=RMT&jt=return&od=16/10/2026&id=17/10/2026&adt=1&yng=1&chd=1&snr=1&inf=0&pet=0&promo=&lang=en&startSearch=true"
        );
    }

    #[test]
    fn a_one_way_search_has_an_empty_return_date() {
        let url = italo_search_url(
            "MC_",
            "NAC",
            NaiveDate::from_ymd_opt(2026, 3, 5).unwrap(),
            None,
            Passengers::one(AgeGroup::Senior),
        );
        assert!(url.contains("jt=single&od=05/03/2026&id=&adt=0&yng=0&chd=0&snr=1&inf=0"));
    }

    #[test]
    fn a_known_country_has_its_own_shop() {
        assert_eq!(shop_link(Some("AT")).url, "https://shop.oebb.at/en/");
    }

    #[test]
    fn an_unknown_country_falls_back_to_one_shop_for_all() {
        assert_eq!(shop_link(None).url, "https://www.thetrainline.com/");
        assert_eq!(shop_link(Some("LV")).kind, BookingLinkKind::Search);
    }
}
