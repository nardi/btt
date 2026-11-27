//! Names the countries of time zones.

/// Returns the English name of the country that a time zone belongs to, or `None` for a time
/// zone outside the table.
pub fn country_of_time_zone(time_zone: chrono_tz::Tz) -> Option<&'static str> {
    // Each arm holds the IANA names and aliases of one country.
    let country = match time_zone.name() {
        "Europe/Tirane" => "Albania",
        "Europe/Andorra" => "Andorra",
        "Europe/Vienna" => "Austria",
        "Europe/Minsk" => "Belarus",
        "Europe/Brussels" => "Belgium",
        "Europe/Sarajevo" => "Bosnia and Herzegovina",
        "Europe/Sofia" => "Bulgaria",
        "Europe/Zagreb" => "Croatia",
        "Asia/Famagusta" | "Asia/Nicosia" | "Europe/Nicosia" => "Cyprus",
        "Europe/Prague" => "Czechia",
        "Europe/Copenhagen" => "Denmark",
        "Europe/Tallinn" => "Estonia",
        "Atlantic/Faeroe" | "Atlantic/Faroe" => "Faroe Islands",
        "Europe/Helsinki" | "Europe/Mariehamn" => "Finland",
        "Europe/Paris" => "France",
        "Europe/Berlin" | "Europe/Busingen" => "Germany",
        "Europe/Gibraltar" => "Gibraltar",
        "Europe/Athens" => "Greece",
        "Europe/Guernsey" => "Guernsey",
        "Europe/Budapest" => "Hungary",
        "Atlantic/Reykjavik" => "Iceland",
        "Europe/Dublin" => "Ireland",
        "Europe/Isle_of_Man" => "Isle of Man",
        "Europe/Rome" => "Italy",
        "Europe/Jersey" => "Jersey",
        "Europe/Riga" => "Latvia",
        "Europe/Vaduz" => "Liechtenstein",
        "Europe/Vilnius" => "Lithuania",
        "Europe/Luxembourg" => "Luxembourg",
        "Europe/Malta" => "Malta",
        "Europe/Chisinau" | "Europe/Tiraspol" => "Moldova",
        "Europe/Monaco" => "Monaco",
        "Europe/Podgorica" => "Montenegro",
        "Europe/Amsterdam" => "Netherlands",
        "Europe/Skopje" => "North Macedonia",
        "Arctic/Longyearbyen" | "Atlantic/Jan_Mayen" | "Europe/Oslo" => "Norway",
        "Europe/Warsaw" => "Poland",
        "Atlantic/Azores" | "Atlantic/Madeira" | "Europe/Lisbon" => "Portugal",
        "Europe/Bucharest" => "Romania",
        "Europe/Astrakhan" | "Europe/Kaliningrad" | "Europe/Kirov" | "Europe/Moscow"
        | "Europe/Samara" | "Europe/Saratov" | "Europe/Ulyanovsk" | "Europe/Volgograd" => "Russia",
        "Europe/San_Marino" => "San Marino",
        "Europe/Belgrade" => "Serbia",
        "Europe/Bratislava" => "Slovakia",
        "Europe/Ljubljana" => "Slovenia",
        "Africa/Ceuta" | "Atlantic/Canary" | "Europe/Madrid" => "Spain",
        "Europe/Stockholm" => "Sweden",
        "Europe/Zurich" => "Switzerland",
        "Asia/Istanbul" | "Europe/Istanbul" => "Turkey",
        "Europe/Kiev" | "Europe/Kyiv" | "Europe/Uzhgorod" | "Europe/Zaporozhye" => "Ukraine",
        "Europe/Belfast" | "Europe/London" => "United Kingdom",
        "Europe/Vatican" => "Vatican City",
        _ => return None,
    };
    Some(country)
}

#[cfg(test)]
mod tests {
    use super::country_of_time_zone;

    #[test]
    fn vienna_is_in_austria() {
        assert_eq!(
            country_of_time_zone(chrono_tz::Europe::Vienna),
            Some("Austria")
        );
    }

    #[test]
    fn busingen_is_in_germany() {
        assert_eq!(
            country_of_time_zone(chrono_tz::Europe::Busingen),
            Some("Germany")
        );
    }

    #[test]
    fn utc_is_in_no_country() {
        assert_eq!(country_of_time_zone(chrono_tz::Tz::UTC), None);
    }
}
