//! Texts that repeat across trips, which a link writes as an index instead of letters.
//!
//! The web viewer ships the same lists, generated from these by a test. Entries are only ever
//! appended, because a link that was sent once has to keep decoding. Adding an entry does not
//! change the schema version, since an older link never uses it, but removing or reordering
//! one would.

/// IANA time zones of the stations a viewer is likely to meet.
pub const TIME_ZONES: &[&str] = &[
    "Europe/Amsterdam",
    "Europe/Berlin",
    "Europe/Vienna",
    "Europe/Zurich",
    "Europe/Paris",
    "Europe/Brussels",
    "Europe/Rome",
    "Europe/Madrid",
    "Europe/Prague",
    "Europe/Warsaw",
    "Europe/Budapest",
    "Europe/Copenhagen",
    "Europe/Stockholm",
    "Europe/Oslo",
    "Europe/Helsinki",
    "Europe/London",
    "Europe/Lisbon",
    "Europe/Dublin",
    "Europe/Luxembourg",
    "Europe/Bratislava",
    "Europe/Ljubljana",
    "Europe/Zagreb",
    "Europe/Belgrade",
    "Europe/Sofia",
    "Europe/Bucharest",
    "Europe/Athens",
    "Europe/Vilnius",
    "Europe/Riga",
    "Europe/Tallinn",
    "Europe/Sarajevo",
    "Europe/Skopje",
    "Europe/Podgorica",
    "Europe/Tirane",
    "Europe/Chisinau",
    "Europe/Kyiv",
    "Europe/Istanbul",
    "Europe/Monaco",
    "Europe/Vaduz",
    "Europe/Andorra",
    "Europe/Malta",
];

/// Train categories as the timetable feeds print them.
pub const CATEGORIES: &[&str] = &[
    "ICE",
    "IC",
    "EC",
    "EN",
    "NJ",
    "RJ",
    "RJX",
    "TGV",
    "THA",
    "ICN",
    "RE",
    "RB",
    "IR",
    "S",
    "R",
    "REX",
    "D",
    "ES",
    "FR",
    "FA",
    "AVE",
    "ALVIA",
    "AVLO",
    "OUIGO",
    "Lyria",
    "EST",
    "EIP",
    "ICD",
    "IRE",
    "RegioExpress",
    "Regionalzug",
    "Intercity",
    "Eurocity",
    "Railjet",
    "Nightjet",
    "InterRegio",
    "InterCity",
    "EuroCity",
    "IC Bus",
    "FLX",
    "ECE",
    "IN",
    "RS",
    "TER",
    "TE",
    "SJ",
    "SJ Snabbtåg",
    "X",
    "ICE Sprinter",
    "EXT",
    "VR",
    "Regionaltog",
    "LYN",
    "Tog",
    "L",
    "U",
    "Os",
    "Sp",
    "Ex",
    "R 5",
    "EuroNight",
    "Zug",
    "Bus",
    "Train",
    "STUB",
];

/// Operators as the app names them, which are the ids of `data/operators.toml`.
pub const OPERATORS: &[&str] = &[
    "bls",
    "cd",
    "cp",
    "db-fernverkehr",
    "db-regio",
    "dsb",
    "european-sleeper",
    "eurostar",
    "flixtrain",
    "gysev",
    "hellenic-train",
    "hz",
    "irish-rail",
    "iryo",
    "italo",
    "laenderbahn",
    "leo-express",
    "lyria",
    "mav",
    "ns",
    "oebb",
    "ouigo",
    "ouigo-espana",
    "pkp-intercity",
    "polregio",
    "regiojet",
    "renfe",
    "rhb",
    "sbb",
    "sj",
    "snalltaget",
    "sncb",
    "sncf",
    "sob",
    "sz",
    "trenitalia",
    "vr",
    "vy",
    "westbahn",
    "zssk",
    "stub",
];

/// Returns the index of a text in a vocabulary.
pub(super) fn index_of(vocabulary: &[&str], text: &str) -> Option<usize> {
    vocabulary.iter().position(|entry| *entry == text)
}

/// Renders the vocabularies as the TypeScript module the viewer imports.
pub fn typescript_module() -> String {
    use std::fmt::Write as _;
    let mut module = String::from(
        "// Generated from `crates/btt-model/src/link/vocabulary.rs` by the `viewer_fixtures` test.\n\
         // Do not edit by hand. Entries are only ever appended.\n",
    );
    for (name, entries) in [
        ("timeZones", TIME_ZONES),
        ("categories", CATEGORIES),
        ("operators", OPERATORS),
    ] {
        // Writing to a string never fails.
        let _ = write!(module, "\nexport const {name}: readonly string[] = [\n");
        for entry in entries {
            let _ = writeln!(module, "  {},", quoted_literal(entry));
        }
        module.push_str("];\n");
    }
    module
}

/// Quotes a text as a TypeScript string literal.
fn quoted_literal(text: &str) -> String {
    let mut quoted = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_vocabulary_repeats_an_entry() {
        for vocabulary in [TIME_ZONES, CATEGORIES, OPERATORS] {
            let mut sorted = vocabulary.to_vec();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), vocabulary.len());
        }
    }
}
