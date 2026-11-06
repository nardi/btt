//! Pricing the tickets of a trip for its first traveller.

use btt_fares::{FareData, PriceBook};
use btt_model::{PassScenario, PassScenarioLeg, TicketLeg, TicketPlan, Traveller, TripPreferences};
use chrono::NaiveDate;

/// Returns the first traveller of a trip that the profiles list. The tickets and passes of a trip
/// are priced for this traveller.
pub fn first_traveller<'a>(
    preferences: &TripPreferences,
    travellers: &'a [Traveller],
) -> Option<&'a Traveller> {
    preferences
        .traveller_ids
        .iter()
        .find_map(|id| travellers.iter().find(|traveller| traveller.id == *id))
}

/// Finds the cheapest tickets for the journeys of a trip's legs, for its first traveller.
pub fn plan_trip_tickets(
    legs: &[TicketLeg],
    preferences: &TripPreferences,
    traveller: Option<&Traveller>,
    today: NaiveDate,
    data: &FareData,
    book: &PriceBook,
) -> TicketPlan {
    let rides = legs
        .iter()
        .flat_map(|leg| {
            btt_ticketing::rides_of_journey(leg.arrival_place_id, &leg.journey, &leg.stations)
        })
        .collect();
    btt_ticketing::plan_tickets(
        &btt_ticketing::TicketRequest {
            rides,
            traveller,
            preferences: &preferences.ticket_preferences,
            today,
        },
        data,
        book,
    )
}

/// Chooses how to cover a trip's legs: single tickets everywhere, and the best one or two passes
/// with the legs that ride on their travel days. The scenarios come sorted, the lowest weighted
/// total first.
pub fn plan_trip_pass_scenarios(
    legs: &[PassScenarioLeg],
    preferences: &TripPreferences,
    traveller: Option<&Traveller>,
    data: &FareData,
) -> Vec<PassScenario> {
    btt_ticketing::pass_scenarios(legs, &data.passes, traveller, preferences.price_weight)
}
