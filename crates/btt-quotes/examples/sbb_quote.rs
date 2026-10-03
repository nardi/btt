//! Prints the trips and prices for an SBB booking link.
//!
//! Run `cargo run -p btt-quotes --example sbb_quote -- "<booking link>"`. The booking site can
//! refuse automated clients, and then the example says so.

use btt_quotes::sbb::SbbClient;

#[tokio::main]
async fn main() {
    let link = std::env::args().nth(1).expect("pass a booking link");
    let client = SbbClient::new().expect("the HTTP client builds");
    match client.quote(&link).await {
        Ok(quotes) => {
            for quote in quotes {
                let rides: Vec<String> = quote
                    .trip
                    .rides
                    .iter()
                    .map(|ride| {
                        format!(
                            "{} {} {} to {} {}",
                            ride.products.join("/"),
                            ride.departure.format("%H:%M"),
                            ride.from,
                            ride.arrival.format("%H:%M"),
                            ride.to
                        )
                    })
                    .collect();
                let prices = quote.prices.map_or_else(
                    || "no price".to_owned(),
                    |prices| {
                        prices
                            .prices
                            .iter()
                            .map(|class| {
                                format!(
                                    "{} {:.2} {} ({})",
                                    class.travel_class,
                                    f64::from(class.price.amount) / 100.0,
                                    class.price.currency,
                                    class.after_sale_flexibility
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    },
                );
                println!("{}  |  {prices}", rides.join(" ; "));
            }
        }
        Err(error) => println!("no quotes: {error}"),
    }
}
