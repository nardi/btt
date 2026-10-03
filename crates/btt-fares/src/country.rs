//! Countries of stations, from their time zones.

/// Returns the two-letter country code for an IANA time zone that belongs to one country, or
/// `None` for zones the table does not know.
///
/// The planner stores a time zone with each station and no country. Every country of the rail
/// network that the app covers has its own zone, so the zone names the country.
pub fn country_of_time_zone(time_zone: &str) -> Option<&'static str> {
    Some(match time_zone {
        "Europe/Vienna" => "AT",
        "Europe/Brussels" => "BE",
        "Europe/Sofia" => "BG",
        "Europe/Zagreb" => "HR",
        "Europe/Prague" => "CZ",
        "Europe/Copenhagen" => "DK",
        "Europe/Tallinn" => "EE",
        "Europe/Helsinki" => "FI",
        "Europe/Paris" => "FR",
        "Europe/Berlin" | "Europe/Busingen" => "DE",
        "Europe/Athens" => "GR",
        "Europe/Budapest" => "HU",
        "Europe/Dublin" => "IE",
        "Europe/Rome" => "IT",
        "Europe/Riga" => "LV",
        "Europe/Vilnius" => "LT",
        "Europe/Luxembourg" => "LU",
        "Europe/Amsterdam" => "NL",
        "Europe/Oslo" => "NO",
        "Europe/Warsaw" => "PL",
        "Europe/Lisbon" => "PT",
        "Europe/Bucharest" => "RO",
        "Europe/Belgrade" => "RS",
        "Europe/Bratislava" => "SK",
        "Europe/Ljubljana" => "SI",
        "Europe/Madrid" => "ES",
        "Europe/Stockholm" => "SE",
        "Europe/Zurich" => "CH",
        "Europe/London" | "Europe/Belfast" => "GB",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zones_name_their_countries() {
        assert_eq!(country_of_time_zone("Europe/Vienna"), Some("AT"));
        assert_eq!(country_of_time_zone("Europe/London"), Some("GB"));
        assert_eq!(country_of_time_zone("Asia/Tokyo"), None);
    }
}
