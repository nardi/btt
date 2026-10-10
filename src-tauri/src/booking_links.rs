//! Reading the parameters of booking links back, for the price lookups.

/// The booking sites the app reads live prices from, by the start of their booking links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookingSite {
    /// sbb.ch, read with plain requests.
    Sbb,
    /// int.bahn.de, read with plain requests to the service of the DB app.
    Db,
    /// The ÖBB shop, read with plain requests to the shop's service.
    Obb,
    /// PKP Intercity, read in the web view.
    Pkp,
    /// lefrecce.it of Trenitalia, read with plain requests.
    Trenitalia,
    /// ns.nl, whose fares are read with plain requests.
    Ns,
    /// dsb.dk, read with plain requests.
    Dsb,
    /// sj.se, read with plain requests.
    Sj,
    /// snalltaget.se, read with plain requests.
    Snalltaget,
    /// jegy.mav.hu, read with plain requests.
    Mav,
    /// The Italo site, read with plain requests.
    Italo,
    /// venta.renfe.com, read with plain requests.
    Renfe,
    /// eurostar.com, read with plain requests.
    Eurostar,
    /// regiojet.com, read with plain requests to its open API.
    Regiojet,
    /// koleo.pl, read with plain requests to the API of its web page.
    Koleo,
    /// booking.europeansleeper.eu, read with plain requests to its booking service.
    EuropeanSleeper,
}

impl BookingSite {
    /// Returns the site of a booking link, or `None` for a link the app reads no price from.
    pub fn of(link: &str) -> Option<Self> {
        let sites = [
            ("https://www.sbb.ch/", Self::Sbb),
            ("https://int.bahn.de/", Self::Db),
            ("https://shop.oebbtickets.at/", Self::Obb),
            ("https://ebilet.intercity.pl/", Self::Pkp),
            ("https://www.lefrecce.it/", Self::Trenitalia),
            ("https://www.ns.nl/", Self::Ns),
            ("https://www.dsb.dk/", Self::Dsb),
            ("https://www.sj.se/", Self::Sj),
            ("https://www.snalltaget.se/", Self::Snalltaget),
            ("https://jegy.mav.hu/", Self::Mav),
            ("https://biglietti.italotreno.com/", Self::Italo),
            ("https://venta.renfe.com/", Self::Renfe),
            ("https://www.eurostar.com/", Self::Eurostar),
            ("https://regiojet.com/", Self::Regiojet),
            ("https://koleo.pl/", Self::Koleo),
            ("https://booking.europeansleeper.eu/", Self::EuropeanSleeper),
        ];
        sites
            .into_iter()
            .find(|(prefix, _)| link.starts_with(prefix))
            .map(|(_, site)| site)
    }
}

/// Decodes a part of an address: `+` is a space, and `%XX` is a byte.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[index + 1..index + 3])
                    .ok()
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                {
                    Some(byte) => {
                        decoded.push(byte);
                        index += 2;
                    }
                    None => decoded.push(b'%'),
                }
            }
            byte => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Returns a parameter of a link, from its query or its fragment, decoded.
pub fn parameter(link: &str, name: &str) -> Option<String> {
    let after_path = link.split_once('?').map_or_else(
        || link.split_once('#').map_or("", |(_, rest)| rest),
        |(_, rest)| rest,
    );
    after_path
        .split(['&', '#'])
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
        .map(percent_decode)
}

/// Returns the local time of day of the search in a booking link, as `HH:MM`, which is the
/// planned departure of the first train. DB keeps it in `hd`, the ÖBB shop in
/// `outwardDateTime`, both after the date, and PKP Intercity in `time`.
pub fn search_time(link: &str) -> Option<String> {
    let after_date = |value: String| {
        value
            .split_once('T')
            .and_then(|(_, time)| time.get(..5).map(str::to_owned))
    };
    parameter(link, "hd")
        .and_then(after_date)
        .or_else(|| parameter(link, "outwardDateTime").and_then(after_date))
        .or_else(|| parameter(link, "time").and_then(|time| time.get(..5).map(str::to_owned)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_come_from_the_query_or_the_fragment() {
        let db = "https://int.bahn.de/en/buchung/fahrplan/suche#sts=true&so=Berlin%20Hbf&hd=2026-10-22T09:37:00";
        assert_eq!(parameter(db, "so").as_deref(), Some("Berlin Hbf"));
        assert_eq!(search_time(db).as_deref(), Some("09:37"));
        let obb = "https://shop.oebbtickets.at/en/ticket?stationOrigEva=8100002&outwardDateTime=2026-10-22T14:30&outwardArrival=false";
        assert_eq!(search_time(obb).as_deref(), Some("14:30"));
        let pkp = "https://ebilet.intercity.pl/wyszukiwanie?dwyj=2026-10-22&swyj=5100081&time=11:30&ticket100=1010";
        assert_eq!(search_time(pkp).as_deref(), Some("11:30"));
        let trenitalia = "https://www.lefrecce.it/x/handoff?departureStation=Genova+Piazza+Principe&departureTime=13%3A00";
        assert_eq!(
            parameter(trenitalia, "departureStation").as_deref(),
            Some("Genova Piazza Principe")
        );
        assert_eq!(
            parameter(trenitalia, "departureTime").as_deref(),
            Some("13:00")
        );
    }

    #[test]
    fn sites_are_told_apart_by_their_links() {
        assert_eq!(
            BookingSite::of("https://shop.oebbtickets.at/en/ticket?x"),
            Some(BookingSite::Obb)
        );
        assert_eq!(
            BookingSite::of("https://www.ns.nl/en/tickets"),
            Some(BookingSite::Ns)
        );
        assert_eq!(BookingSite::of("https://www.thetrainline.com/"), None);
    }
}
