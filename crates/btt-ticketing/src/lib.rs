//! Finds the cheapest tickets for the trains of a trip.
//!
//! The optimizer takes the rides the planner chose, the traveller and the trip's ticket
//! preferences, and returns a `TicketPlan` with the ways to cover every ride. It does no
//! network work, so it runs on fixtures in tests.

mod links;
mod optimizer;
mod pass_plan;
mod rides;

use btt_fares::{FareData, PriceBook};
use btt_model::{
    AgeGroup, EuroCents, EuroRange, Journey, LegPlanStation, TicketPlan, TicketPreferences,
    TicketRide, TravelClass, Traveller,
};
use smol_str::SmolStr;

pub use links::{
    INTERRAIL_PASS_PAGE, Passengers, italo_link, italo_search_url, pass_link, shop_link,
};
pub use optimizer::{Problem, Settings, total_of};
pub use pass_plan::{LegOption, LegToPlan, TripScenario, TripToPlan, pass_scenarios, solve};
pub use rides::{RideInput, pricing_rides_of_journey, ride_input, rides_of_journey};

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

/// Returns the estimated price of the single tickets and the night accommodation of a
/// journey, which the planner shows next to the journey's time. The price is what the
/// single-ticket strategy of `plan_tickets` costs for the same rides: the cheapest tickets
/// that cover them and the extra for a berth. It assumes one adult in second class, whose
/// fare is the one the price model predicts. Returns `None` when a train of the journey has
/// no known operator, or when the station table does not hold every station of the journey.
pub fn journey_price(
    journey: &Journey,
    stations: &[LegPlanStation],
    data: &FareData,
    book: &PriceBook,
) -> Option<EuroRange> {
    let ride_count = journey.rides().count();
    let rides = pricing_rides_of_journey(journey, stations);
    if rides.len() != ride_count
        || rides
            .iter()
            .any(|ride| ride.train_label.operator.is_empty())
    {
        return None;
    }
    let preferences = TicketPreferences::default();
    let first_day = rides.first()?.departure_date;
    let settings = Settings {
        class: TravelClass::Second,
        age_group: AgeGroup::Adult,
        residence_country: None,
        preferences: &preferences,
        purchase_date: first_day,
        today: first_day,
        single_ticket_percent: 100,
    };
    Problem::new(&rides, data, book, &settings).rides_price()
}

/// Returns the estimated price of a journey on the travel days of a pass: the fares of the
/// rides the pass does not cover, the reservations of the ones it does, and the night
/// accommodation of a pass holder. It prices with the same rules as `plan_tickets`, so a
/// planned pass price and a ticket plan agree. It assumes one adult in second class and no
/// country of residence, whose rides at home the pass covers only at the ends of the trip.
/// Returns `None` when a train of the journey has no known operator, or when the station
/// table does not hold every station of the journey.
pub fn journey_price_on_pass(
    journey: &Journey,
    stations: &[LegPlanStation],
    data: &FareData,
    book: &PriceBook,
) -> Option<EuroRange> {
    let ride_count = journey.rides().count();
    let rides = pricing_rides_of_journey(journey, stations);
    if rides.len() != ride_count
        || rides
            .iter()
            .any(|ride| ride.train_label.operator.is_empty())
    {
        return None;
    }
    let preferences = TicketPreferences::default();
    let first_day = rides.first()?.departure_date;
    let settings = Settings {
        class: TravelClass::Second,
        age_group: AgeGroup::Adult,
        residence_country: None,
        preferences: &preferences,
        purchase_date: first_day,
        today: first_day,
        single_ticket_percent: 100,
    };
    Problem::new(&rides, data, book, &settings).rides_price_on_pass()
}

/// Returns what a ride of a class costs on the travel days of a pass, in whole euro cents:
/// the median of the fee the pass charges on top, or zero when it charges none. The class
/// knows the country of the boarding station and not the one of the whole ride, so a rule of
/// that country stands in. A class with berths costs its night accommodation fee, because
/// the pass charges that for a night ride. The exact price of a whole journey comes from
/// `journey_price_on_pass`.
pub fn pass_cents_for_class(
    category: &str,
    operator: &str,
    country: Option<&str>,
    offers_berths: bool,
    data: &FareData,
) -> u32 {
    if !data.reservations.pass_accepted(category, operator) {
        return 0;
    }
    let rule = if offers_berths {
        data.reservations.for_ride(country, country, category, true)
    } else {
        data.reservations
            .for_ride(country, country, category, false)
            .filter(|rule| {
                rule.mode
                    .is_charged(TicketPreferences::default().reserve_optional_seats)
            })
    };
    let Some(rule) = rule else {
        return 0;
    };
    let fee = rule.fee(TravelClass::Second);
    EuroCents::from_euro(f64::midpoint(fee.low, fee.high)).0
}

#[cfg(test)]
mod tests;
