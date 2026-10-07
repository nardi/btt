//! Prints the connections and prices of a booking site between two of its station codes.
//!
//! Run `cargo run -p btt-quotes --example site_quote -- <site> <origin> <destination>
//! <YYYY-MM-DDTHH:MM> <UTC offset in hours>`, where the site is one of `dsb`, `sj`,
//! `snalltaget`, `mav`, `italo`, `renfe`, `regiojet` and `eurostar`, and the time is the local
//! departure.

use std::sync::Arc;

use btt_quotes::{
    ConnectionSearch, ConnectionSource, MemoryStore, QuoteError, SiteConnection, SiteService,
};

/// Runs a search on a site and returns its connections.
async fn connections<S: ConnectionSource>(
    source: S,
    search: &ConnectionSearch,
) -> Result<Vec<SiteConnection>, QuoteError> {
    SiteService::new(source, Arc::new(MemoryStore::default()))?
        .connections(search)
        .await
}

#[tokio::main]
async fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [site, origin, destination, departure, offset] = arguments.as_slice() else {
        panic!("pass site, origin, destination, local departure and UTC offset");
    };
    let departure = chrono::NaiveDateTime::parse_from_str(departure, "%Y-%m-%dT%H:%M")
        .expect("a departure as YYYY-MM-DDTHH:MM");
    let offset: i64 = offset.parse().expect("an offset in hours");
    let search = ConnectionSearch {
        origin: origin.clone(),
        destination: destination.clone(),
        departure,
        departure_moment: (departure - chrono::Duration::hours(offset)).and_utc(),
        train_count: None,
    };
    let result = match site.as_str() {
        "dsb" => connections(btt_quotes::dsb::Dsb, &search).await,
        "sj" => connections(btt_quotes::sj::Sj, &search).await,
        "snalltaget" => connections(btt_quotes::snalltaget::Snalltaget::default(), &search).await,
        "mav" => connections(btt_quotes::mav::Mav, &search).await,
        "italo" => connections(btt_quotes::italo::Italo::default(), &search).await,
        "renfe" => connections(btt_quotes::renfe::Renfe, &search).await,
        "regiojet" => connections(btt_quotes::regiojet::Regiojet, &search).await,
        "eurostar" => connections(btt_quotes::eurostar::Eurostar, &search).await,
        other => panic!("unknown site {other}"),
    };
    match result {
        Ok(connections) => {
            for connection in connections {
                println!(
                    "{} {}  |  2nd {:?} 1st {:?} {}{}",
                    connection.departure.format("%Y-%m-%d %H:%M"),
                    connection.trains.join(", "),
                    connection.second_class_price,
                    connection.first_class_price,
                    connection.currency,
                    if connection.not_for_sale {
                        "  not for sale"
                    } else {
                        ""
                    }
                );
            }
        }
        Err(error) => println!("no connections: {error}"),
    }
}
