//! Finds the cheapest tickets for the trains of a trip.
//!
//! The optimizer takes the rides the planner chose, the traveller and the trip's ticket
//! preferences, and returns a `TicketPlan` with the ways to cover every ride. It does no
//! network work, so it runs on fixtures in tests.

mod links;
mod optimizer;
mod rides;

use btt_fares::{FareData, PriceBook};
use btt_model::{AgeGroup, TicketPlan, TicketPreferences, TicketRide, TravelClass, Traveller};
use smol_str::SmolStr;

pub use links::{
    INTERRAIL_PASS_PAGE, Passengers, italo_link, italo_search_url, pass_link, shop_link,
};
pub use optimizer::{Problem, Settings, total_of};
pub use rides::{RideInput, rides_of_journey};

/// What the optimizer needs to know.
pub struct TicketRequest<'a> {
    /// The rides of the trip in travel order.
    pub rides: Vec<RideInput>,
    /// The traveller the tickets are for. `None` assumes an adult in second class.
    pub traveller: Option<&'a Traveller>,
    /// How tickets are chosen.
    pub preferences: &'a TicketPreferences,
    /// Today, which is the purchase date unless the preferences set another.
    pub today: chrono::NaiveDate,
}

/// The share of the adult single ticket price that a child pays. This is an assumption that
/// holds for many operators and not for all, and the plan says so.
const CHILD_PERCENT: u32 = 50;

/// Finds the tickets for a trip.
pub fn plan_tickets(request: &TicketRequest<'_>, data: &FareData, book: &PriceBook) -> TicketPlan {
    let rides: Vec<TicketRide> = request
        .rides
        .iter()
        .map(|ride| TicketRide {
            leg_arrival_place_id: ride.leg_arrival_place_id,
            train_label: ride.train_label.clone(),
            from_name: ride.from_name.clone(),
            to_name: ride.to_name.clone(),
            departure_time: ride.departure_time,
            arrival_time: ride.arrival_time,
            time_zone: ride.time_zone.clone(),
            offers_berths: ride.offers_berths,
        })
        .collect();
    let mut notes: Vec<SmolStr> = Vec::new();
    let Some(first_ride) = request.rides.first() else {
        notes.push("The trip has no trains yet.".into());
        return TicketPlan {
            rides,
            strategies: Vec::new(),
            traveller_name: request.traveller.map(|traveller| traveller.name.clone()),
            notes,
        };
    };
    let age_group = request.traveller.map_or(AgeGroup::Adult, |traveller| {
        traveller.age_group_on(first_ride.departure_date)
    });
    let single_ticket_percent = match age_group {
        AgeGroup::Infant => {
            notes.push("Children under 4 travel free.".into());
            0
        }
        AgeGroup::Child => {
            notes.push(
                "Single tickets are priced at half the adult price, which not every operator \
                 gives. A child travels free on a pass with a paying adult, and passes are not \
                 compared here."
                    .into(),
            );
            CHILD_PERCENT
        }
        _ => 100,
    };
    if request.preferences.live_prices && first_ride.departure_date < request.today {
        notes.push(
            "The trip starts before today, so booking sites give no live prices for the trains \
             that have left."
                .into(),
        );
    }
    if request.traveller.is_none() {
        notes.push("Add a traveller in Setup. The prices assume one adult in second class.".into());
    }
    let settings = Settings {
        class: request
            .traveller
            .map_or(TravelClass::Second, |traveller| traveller.travel_class),
        age_group,
        residence_country: request
            .traveller
            .map(|traveller| traveller.residence_country.as_str()),
        preferences: request.preferences,
        purchase_date: request
            .preferences
            .assumed_purchase_date
            .unwrap_or(request.today),
        today: request.today,
        single_ticket_percent,
    };
    let problem = Problem::new(&request.rides, data, book, &settings);
    TicketPlan {
        rides,
        strategies: problem.strategies(),
        traveller_name: request.traveller.map(|traveller| traveller.name.clone()),
        notes,
    }
}

#[cfg(test)]
mod tests;
