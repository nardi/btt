//! Links to the places where tickets are sold.

use btt_model::{BookingLink, BookingLinkKind};

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
    fn a_known_country_has_its_own_shop() {
        assert_eq!(shop_link(Some("AT")).url, "https://shop.oebb.at/en/");
    }

    #[test]
    fn an_unknown_country_falls_back_to_one_shop_for_all() {
        assert_eq!(shop_link(None).url, "https://www.thetrainline.com/");
        assert_eq!(shop_link(Some("LV")).kind, BookingLinkKind::Search);
    }
}
