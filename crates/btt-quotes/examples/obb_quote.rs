//! Prints the connections and prices of the ÖBB shop between two of its station numbers.
//!
//! Run `cargo run -p btt-quotes --example obb_quote -- <origin> <destination> <YYYY-MM-DDTHH:MM>`.

use std::sync::Arc;

use btt_quotes::MemoryStore;
use btt_quotes::obb::{ObbService, Search};

#[tokio::main]
async fn main() {
    let mut arguments = std::env::args().skip(1);
    let mut next = || {
        arguments
            .next()
            .expect("pass origin, destination and departure")
    };
    let search = Search {
        origin: next().parse().expect("a station number"),
        destination: next().parse().expect("a station number"),
        departure: chrono::NaiveDateTime::parse_from_str(&next(), "%Y-%m-%dT%H:%M")
            .expect("a departure as YYYY-MM-DDTHH:MM"),
    };
    let service =
        ObbService::new(Arc::new(MemoryStore::default())).expect("the HTTP client builds");
    match service.connections(&search).await {
        Ok(connections) => {
            for connection in connections {
                let price = connection
                    .price
                    .map_or_else(|| "no price".to_owned(), |price| format!("{price:.2} EUR"));
                println!(
                    "{} {}  |  {price}",
                    connection.departure.format("%H:%M"),
                    connection.trains.join(", ")
                );
            }
        }
        Err(error) => println!("no connections: {error}"),
    }
}
