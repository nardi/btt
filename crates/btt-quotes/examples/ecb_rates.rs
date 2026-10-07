//! Prints the latest ECB reference rates as euro per unit.
//!
//! Run `cargo run -p btt-quotes --example ecb_rates`.

use std::sync::Arc;

use btt_quotes::MemoryStore;
use btt_quotes::ecb::ExchangeRateService;

#[tokio::main]
async fn main() {
    let service =
        ExchangeRateService::new(Arc::new(MemoryStore::default())).expect("the HTTP client builds");
    let Some(rates) = service.rates().await else {
        println!("no rates");
        return;
    };
    println!("rates of {}", rates.date);
    for currency in ["CHF", "PLN", "SEK", "DKK", "HUF", "CZK", "GBP", "NOK"] {
        println!("{currency}: {:?}", rates.euro_per(currency));
    }
}
