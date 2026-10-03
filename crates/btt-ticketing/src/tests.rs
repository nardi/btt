//! Tests of the optimizer: examples from the pass rules, and a comparison with an exhaustive
//! search on random small trips.

use std::num::NonZeroU32;

use btt_fares::{FareData, PassKind, PriceAgeGroup, PriceBook, RideFacts};
use btt_model::{
    EuroCents, StrategyKind, TicketPlan, TicketPreferences, TicketProduct, TicketStrategy,
    TrainLabel, TravelClass, Traveller, TripPlaceId, UnixTimeMinutes,
};
use chrono::NaiveDate;
use proptest::prelude::*;

use crate::{Problem, RideInput, Settings, TicketRequest, plan_tickets};

fn date(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 7, 1).unwrap() + chrono::Days::new(u64::from(day))
}

/// Builds a ride on a day of July 2026 that arrives on the same day, unless it has berths.
fn ride(
    day: u32,
    countries: (&str, &str),
    category: &str,
    kilometres: f64,
    offers_berths: bool,
) -> RideInput {
    RideInput {
        leg_arrival_place_id: TripPlaceId(NonZeroU32::MIN),
        train_label: TrainLabel {
            category: category.into(),
            number: "1".into(),
        },
        from_name: "From".into(),
        to_name: "To".into(),
        from_coordinates: btt_model::GeoCoordinates::from_degrees(0.0, 0.0).unwrap(),
        to_coordinates: btt_model::GeoCoordinates::from_degrees(0.0, 0.0).unwrap(),
        departure_time: UnixTimeMinutes(day * 1440),
        arrival_time: UnixTimeMinutes(day * 1440 + 300),
        time_zone: "Europe/Vienna".into(),
        departure_date: date(day),
        arrival_date: date(day + u32::from(offers_berths)),
        from_country: Some(countries.0.into()),
        to_country: Some(countries.1.into()),
        distance_kilometres: kilometres,
        offers_berths,
    }
}

fn plan(rides: Vec<RideInput>, traveller: Option<&Traveller>) -> TicketPlan {
    let data = FareData::bundled().unwrap();
    let book = PriceBook::from_data(&data);
    let preferences = TicketPreferences::default();
    plan_tickets(
        &TicketRequest {
            rides,
            traveller,
            preferences: &preferences,
            today: date(0),
        },
        &data,
        &book,
    )
}

fn middle_total(strategy: &TicketStrategy) -> u64 {
    strategy
        .items
        .iter()
        .map(|item| u64::from(item.price.middle().0))
        .sum()
}

fn adult(residence: &str) -> Traveller {
    Traveller::new(
        "Sam".into(),
        NaiveDate::from_ymd_opt(1990, 1, 1).unwrap(),
        residence.into(),
    )
}

#[test]
fn one_short_ride_costs_a_single_ticket() {
    let plan = plan(vec![ride(0, ("AT", "AT"), "RJX", 60.0, false)], None);
    let best = &plan.strategies[0];
    assert_eq!(best.kind, StrategyKind::SingleTickets);
    assert_eq!(best.items.len(), 1);
    assert_eq!(best.covers, vec![vec![0]]);
}

#[test]
fn a_week_of_long_rides_is_cheaper_on_a_pass() {
    let rides = (0..7)
        .map(|day| ride(day, ("DE", "DE"), "ICE", 600.0, false))
        .collect();
    let plan = plan(rides, Some(&adult("NL")));
    let best = &plan.strategies[0];
    assert_eq!(best.kind, StrategyKind::Pass);
    let pass = best
        .items
        .iter()
        .find(|item| matches!(item.product, TicketProduct::InterrailPass { .. }))
        .unwrap();
    let TicketProduct::InterrailPass { travel_days, .. } = &pass.product else {
        unreachable!()
    };
    assert_eq!(travel_days.len(), 7);
    // Every ride is covered by the pass.
    assert!(best.covers.iter().all(|cover| cover == &vec![pass.id]));
}

#[test]
fn a_mandatory_reservation_adds_to_the_pass_day() {
    let rides = (0..4)
        .map(|day| ride(day, ("FR", "FR"), "TGV INOUI", 700.0, false))
        .collect();
    let plan = plan(rides, Some(&adult("NL")));
    let pass_strategy = plan
        .strategies
        .iter()
        .find(|strategy| strategy.kind == StrategyKind::Pass)
        .unwrap();
    let reservations = pass_strategy
        .items
        .iter()
        .filter(|item| item.product == TicketProduct::Reservation)
        .count();
    assert_eq!(reservations, 4);
}

#[test]
fn a_night_train_adds_its_accommodation_to_every_strategy() {
    let plan = plan(vec![ride(0, ("AT", "DE"), "NJ", 800.0, true)], None);
    for strategy in &plan.strategies {
        assert!(
            strategy
                .items
                .iter()
                .any(|item| item.product == TicketProduct::NightAccommodation),
            "{}",
            strategy.title
        );
    }
}

#[test]
fn a_night_train_arrives_after_the_day_it_leaves() {
    let ride = ride(3, ("AT", "DE"), "NJ", 800.0, true);
    assert_eq!(ride.departure_date, date(3));
    assert_eq!(ride.arrival_date, date(4));
}

#[test]
fn the_country_of_residence_allows_one_journey_each_way() {
    let rides = vec![
        ride(0, ("NL", "NL"), "IC", 50.0, false),
        ride(0, ("NL", "DE"), "ICE", 400.0, false),
        ride(1, ("DE", "DE"), "ICE", 400.0, false),
        ride(2, ("NL", "NL"), "IC", 50.0, false),
        ride(3, ("DE", "NL"), "ICE", 400.0, false),
        ride(3, ("NL", "NL"), "IC", 50.0, false),
    ];
    let data = FareData::bundled().unwrap();
    let book = PriceBook::from_data(&data);
    let preferences = TicketPreferences::default();
    let settings = Settings {
        class: TravelClass::Second,
        age_group: btt_model::AgeGroup::Adult,
        residence_country: Some("NL"),
        preferences: &preferences,
        purchase_date: date(0),
        single_ticket_percent: 100,
    };
    let problem = Problem::new(&rides, &data, &book, &settings);
    assert_eq!(
        problem.pass_covers_ride(),
        vec![true, true, true, false, true, true]
    );
}

#[test]
fn a_child_pays_half_and_an_infant_nothing() {
    let rides = vec![ride(0, ("AT", "AT"), "RJX", 300.0, false)];
    let adult_plan = plan(rides.clone(), Some(&adult("AT")));
    let child = Traveller::new(
        "Kid".into(),
        NaiveDate::from_ymd_opt(2020, 1, 1).unwrap(),
        "AT".into(),
    );
    let child_plan = plan(rides.clone(), Some(&child));
    let infant = Traveller::new(
        "Baby".into(),
        NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
        "AT".into(),
    );
    let infant_plan = plan(rides, Some(&infant));
    let single = |plan: &TicketPlan| {
        plan.strategies
            .iter()
            .find(|strategy| strategy.kind == StrategyKind::SingleTickets)
            .unwrap()
            .total
            .low
            .0
    };
    assert_eq!(single(&child_plan), single(&adult_plan) / 2);
    assert_eq!(single(&infant_plan), 0);
    assert!(
        child_plan
            .strategies
            .iter()
            .all(|strategy| strategy.kind == StrategyKind::SingleTickets)
    );
}

#[test]
fn a_through_ticket_beats_two_tickets_by_the_minimum_fare() {
    // Each of two 20 km rides costs the minimum fare or more, so one ticket for 40 km is cheaper.
    let rides = vec![
        ride(0, ("AT", "AT"), "REX", 20.0, false),
        ride(0, ("AT", "AT"), "REX", 20.0, false),
    ];
    let plan = plan(rides, None);
    let single = &plan.strategies[0];
    assert_eq!(single.items.len(), 1);
    assert_eq!(single.items[0].ride_indices, vec![0, 1]);
}

/// Every price the exhaustive search needs, from the same book as the optimizer.
struct Brute<'a> {
    rides: &'a [RideInput],
    book: &'a PriceBook,
}

impl Brute<'_> {
    fn run_price(&self, first: usize, last: usize) -> u64 {
        let facts: Vec<RideFacts<'_>> = self.rides[first..=last]
            .iter()
            .map(|ride| RideFacts {
                from_country: ride.from_country.as_deref(),
                to_country: ride.to_country.as_deref(),
                category: &ride.train_label.category,
                distance_kilometres: ride.distance_kilometres,
            })
            .collect();
        u64::from(self.book.price_run(&facts).middle().0)
    }

    /// The cheapest cover of the rides of `range` that need a ticket, by plain recursion.
    fn partition(&self, range: &std::ops::Range<usize>, needs: &dyn Fn(usize) -> bool) -> u64 {
        self.partition_from(range.start, range.end, needs)
    }

    fn partition_from(&self, index: usize, end: usize, needs: &dyn Fn(usize) -> bool) -> u64 {
        if index >= end {
            return 0;
        }
        if !needs(index) {
            return self.partition_from(index + 1, end, needs);
        }
        let mut best = u64::MAX;
        for last in index..end {
            if !needs(last) {
                break;
            }
            best =
                best.min(self.run_price(index, last) + self.partition_from(last + 1, end, needs));
        }
        best
    }
}

/// The cheapest total over single tickets and every pass, start and set of travel days.
fn exhaustive_best(rides: &[RideInput], data: &FareData, book: &PriceBook) -> u64 {
    let brute = Brute { rides, book };
    let mut days: Vec<std::ops::Range<usize>> = Vec::new();
    for (index, ride) in rides.iter().enumerate() {
        match days.last_mut() {
            Some(day) if rides[day.start].departure_date == ride.departure_date => {
                day.end = index + 1;
            }
            _ => days.push(index..index + 1),
        }
    }
    let singles: Vec<u64> = days
        .iter()
        .map(|day| brute.partition(day, &|_| true))
        .collect();
    let mut best: u64 = singles.iter().sum();
    for pass in &data.passes.passes {
        let price = u64::from(
            pass.price(PriceAgeGroup::Adult, TravelClass::Second)
                .unwrap()
                .0,
        );
        for start_day in &days {
            let start = rides[start_day.start].departure_date;
            let end = pass.last_valid_date(start);
            let on_pass = |index: usize| {
                rides[index].departure_date >= start && rides[index].arrival_date <= end
            };
            // Cost of each day on the pass, or `None` when no ride of it can use the pass.
            let on_pass_costs: Vec<Option<u64>> = days
                .iter()
                .map(|day| {
                    if !day.clone().any(on_pass) {
                        return None;
                    }
                    let reservations: u64 = day
                        .clone()
                        .filter(|&index| on_pass(index))
                        .filter_map(|index| {
                            let ride = &rides[index];
                            data.reservations
                                .for_ride(
                                    ride.from_country.as_deref(),
                                    ride.to_country.as_deref(),
                                    &ride.train_label.category,
                                    false,
                                )
                                .filter(|rule| rule.mode.is_charged(false))
                        })
                        .map(|rule| {
                            let fee = rule.fee(TravelClass::Second);
                            let low = EuroCents::from_euro(fee.low);
                            let high = EuroCents::from_euro(fee.high);
                            u64::from(low.0.midpoint(high.0))
                        })
                        .sum();
                    Some(brute.partition(day, &|index| !on_pass(index)) + reservations)
                })
                .collect();
            let usable: Vec<usize> = (0..days.len())
                .filter(|&index| on_pass_costs[index].is_some())
                .collect();
            let limit = match pass.kind {
                PassKind::Flexi { travel_days } => usize::from(travel_days),
                PassKind::Continuous => usable.len(),
            };
            for mask in 0u32..(1 << usable.len()) {
                if mask.count_ones() as usize > limit {
                    continue;
                }
                let mut total = price;
                for (day_index, singles_cost) in singles.iter().enumerate() {
                    let chosen = usable
                        .iter()
                        .position(|&usable_day| usable_day == day_index)
                        .is_some_and(|position| mask & (1 << position) != 0);
                    total += if chosen {
                        on_pass_costs[day_index].unwrap()
                    } else {
                        *singles_cost
                    };
                }
                best = best.min(total);
            }
        }
    }
    best
}

fn arbitrary_ride() -> impl Strategy<Value = (u32, usize, usize, usize, f64)> {
    (0u32..6, 0usize..4, 0usize..4, 0usize..5, 20.0f64..1800.0)
}

proptest! {
    /// The optimizer finds the same cheapest total as a search over every pass, start, set of
    /// travel days and way to split the day into tickets.
    #[test]
    fn matches_the_exhaustive_search(mut raw in prop::collection::vec(arbitrary_ride(), 1..10)) {
        let countries = ["DE", "FR", "IT", "AT"];
        let categories = ["ICE", "TGV INOUI", "FR", "RJX", "REX"];
        raw.sort_by_key(|ride| ride.0);
        let rides: Vec<RideInput> = raw
            .iter()
            .map(|&(day, from, to, category, kilometres)| {
                ride(day, (countries[from], countries[to]), categories[category], kilometres, false)
            })
            .collect();
        let data = FareData::bundled().unwrap();
        let book = PriceBook::from_data(&data);
        let preferences = TicketPreferences::default();
        let settings = Settings {
            class: TravelClass::Second,
            age_group: btt_model::AgeGroup::Adult,
            residence_country: None,
            preferences: &preferences,
            purchase_date: date(0),
            single_ticket_percent: 100,
        };
        let problem = Problem::new(&rides, &data, &book, &settings);
        let strategies = problem.strategies();
        let found = strategies.iter().map(middle_total).min().unwrap();
        let expected = exhaustive_best(&rides, &data, &book);
        // The optimizer lists the best pass of each kind, and the cheapest of all is among them.
        prop_assert_eq!(found, expected);
    }
}

#[test]
fn an_italo_ticket_links_to_the_italo_site_with_station_codes() {
    let mut italo = ride(0, ("IT", "IT"), "Italo", 570.0, false);
    italo.from_coordinates = btt_model::GeoCoordinates::from_degrees(45.4339, 9.2391).unwrap();
    italo.to_coordinates = btt_model::GeoCoordinates::from_degrees(41.9006, 12.5020).unwrap();
    let plan = plan(vec![italo], None);
    let link = plan.strategies[0].items[0].booking_link.as_ref().unwrap();
    assert!(link.url.starts_with("https://biglietti.italotreno.com/"));
    assert!(
        link.url
            .contains("osc=RG_&dsc=RMT&jt=oneway&od=01%2F07%2F2026")
    );
}
