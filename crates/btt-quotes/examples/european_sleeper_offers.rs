//! Asks European Sleeper for the offers of a train and prints the prices.
//!
//! Run `cargo run -p btt-quotes --example european_sleeper_offers -- <origin> <destination>
//! <departure>`, such as `8800104 8010100 2026-10-13T19:30`, where the stations are the codes of
//! the booking service.

use std::sync::Arc;

use btt_quotes::MemoryStore;
use btt_quotes::european_sleeper::{EuropeanSleeperService, PlaceKind, Search};

#[tokio::main]
async fn main() {
    let mut arguments = std::env::args().skip(1);
    let origin = arguments.next().expect("pass the origin");
    let destination = arguments.next().expect("pass the destination");
    let departure = arguments.next().expect("pass the departure");
    let search = Search {
        origin,
        destination,
        departure: chrono::NaiveDateTime::parse_from_str(&departure, "%Y-%m-%dT%H:%M")
            .expect("the departure as YYYY-MM-DDTHH:MM"),
    };
    let service =
        EuropeanSleeperService::new(Arc::new(MemoryStore::default())).expect("the client builds");
    let offers = match service.offers(&search).await {
        Ok(Some(offers)) => offers,
        Ok(None) => return println!("no train at that departure"),
        Err(error) => return println!("failed: {error}"),
    };
    println!(
        "{} leaves {}, bookable {}, last minute fare {:?}",
        offers.train, offers.departure, offers.bookable, offers.last_minute
    );
    println!("cheapest ticket {:?}", offers.cheapest_ticket());
    for kind in [PlaceKind::Seat, PlaceKind::Couchette, PlaceKind::Berth] {
        println!(
            "{kind:?}: adult {:?}, pass {:?}, extra over the ticket {:?}, price for a pass holder {:?}",
            offers.adult.of(kind),
            offers.pass.of(kind),
            offers.extra_over_ticket(kind),
            offers.pass_price(kind),
        );
    }
}
