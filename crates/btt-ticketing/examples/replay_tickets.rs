//! Plans the tickets of a problem report again and prints the single tickets, to show how the
//! optimizer splits the trains of a trip into tickets.
//!
//! Run `cargo run -p btt-ticketing --example replay_tickets -- <tickets.json>` with the
//! `tickets.json` of an unpacked problem report.

use btt_fares::{FareData, PriceBook};
use btt_model::{TicketLeg, TicketPreferences, TicketProduct, Traveller};
use btt_ticketing::{TicketRequest, plan_tickets, rides_of_journey};
use serde::Deserialize;

/// The part of `tickets.json` that the replay reads.
#[derive(Deserialize)]
struct TicketsReport {
    /// The last ticket plan of the trip with what it was made from.
    plan: PlanRecord,
}

/// What a ticket plan was made from.
#[derive(Deserialize)]
struct PlanRecord {
    /// The chosen journey of each leg.
    legs: Vec<TicketLeg>,
    /// The traveller, or `None` for an adult.
    traveller: Option<Traveller>,
    /// The ticket preferences of the trip.
    preferences: TicketPreferences,
    /// When the plan was made, in RFC 3339 format.
    created_at: String,
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("pass the tickets.json of a report");
    let text = std::fs::read_to_string(path).expect("the file reads");
    let report: TicketsReport = serde_json::from_str(&text).expect("the file parses");
    let record = report.plan;
    let rides = record
        .legs
        .iter()
        .flat_map(|leg| rides_of_journey(leg.arrival_place_id, &leg.journey, &leg.stations))
        .collect();
    let today = chrono::DateTime::parse_from_rfc3339(&record.created_at)
        .expect("the date of the plan parses")
        .date_naive();
    let data = FareData::bundled().expect("the fare data loads");
    let book = PriceBook::from_data(&data);
    let plan = plan_tickets(
        &TicketRequest {
            rides,
            traveller: record.traveller.as_ref(),
            preferences: &record.preferences,
            today,
        },
        &data,
        &book,
    );
    for strategy in &plan.strategies {
        println!(
            "
{}  {:.2}-{:.2} EUR",
            strategy.title,
            strategy.total.low.euro(),
            strategy.total.high.euro()
        );
        print_items(&plan, strategy);
    }
}

/// Prints the single tickets and the night accommodation of a strategy.
fn print_items(plan: &btt_model::TicketPlan, strategy: &btt_model::TicketStrategy) {
    for item in &strategy.items {
        if item.product == TicketProduct::NightAccommodation {
            println!(
                "  night {:?}  {:.2}-{:.2} EUR  {}",
                item.ride_indices,
                item.price.low.euro(),
                item.price.high.euro(),
                item.price.source.name,
            );
            continue;
        }
        if item.product != TicketProduct::SingleTicket {
            continue;
        }
        let first = &plan.rides[usize::from(item.ride_indices[0])];
        let last = &plan.rides[usize::from(item.ride_indices[item.ride_indices.len() - 1])];
        println!(
            "{:?}  {} -> {}  {:.2}-{:.2} EUR  {}  {}",
            item.ride_indices,
            first.from_name,
            last.to_name,
            item.price.low.euro(),
            item.price.high.euro(),
            if item.live_quote_request.is_some() {
                "live price"
            } else {
                "no live price"
            },
            item.booking_link
                .as_ref()
                .map_or("", |link| link.url.as_str())
                .chars()
                .take(60)
                .collect::<String>(),
        );
    }
}
