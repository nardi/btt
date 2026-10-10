//! Asks the ÖBB shop for the accommodation prices of a night train, with and without the
//! Interrail card, and prints the lowest price of each kind.
//!
//! Run `cargo run -p btt-quotes --example night_offers -- <origin> <destination> <departure> <train>`,
//! such as `1290401 8503000 2026-10-20T21:39 "NJ 466"`, where the stations are the numbers of the
//! shop.

use std::sync::Arc;

use btt_quotes::MemoryStore;
use btt_quotes::obb::{ObbService, Search};

#[tokio::main]
async fn main() {
    let mut arguments = std::env::args().skip(1);
    let origin = arguments
        .next()
        .expect("pass the origin")
        .parse()
        .expect("a number");
    let destination = arguments
        .next()
        .expect("pass the destination")
        .parse()
        .expect("a number");
    let departure = arguments.next().expect("pass the departure");
    let train = arguments.next().expect("pass the train");
    let search = Search {
        origin,
        destination,
        departure: chrono::NaiveDateTime::parse_from_str(&departure, "%Y-%m-%dT%H:%M")
            .expect("the departure as YYYY-MM-DDTHH:MM"),
    };
    let service = ObbService::new(Arc::new(MemoryStore::default())).expect("the client builds");
    for with_pass in [false, true] {
        match service.night_offers(&search, &train, with_pass).await {
            Ok(Some(offers)) => {
                println!(
                    "{train}, {}:",
                    if with_pass {
                        "with the Interrail card"
                    } else {
                        "without a card"
                    }
                );
                for kind in ["SEAT", "COUCHETTE", "BERTH"] {
                    match offers.lowest_price(kind, with_pass) {
                        Some(price) => println!("  {kind:<10} {price:.2}"),
                        None => println!("  {kind:<10} none"),
                    }
                }
            }
            Ok(None) => println!("the shop lists no {train} at {departure}"),
            Err(error) => println!("failed: {error}"),
        }
    }
}
