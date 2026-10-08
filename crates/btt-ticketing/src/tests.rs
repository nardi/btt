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

/// The bundled fare data, read once for all tests. The station tables take a while to parse.
fn fare_data() -> &'static FareData {
    static DATA: std::sync::OnceLock<FareData> = std::sync::OnceLock::new();
    DATA.get_or_init(|| FareData::bundled().unwrap())
}

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
            operator: "".into(),
            agency: "".into(),
        },
        from_name: "From".into(),
        to_name: "To".into(),
        from_coordinates: btt_model::GeoCoordinates::from_degrees(0.0, 0.0).unwrap(),
        to_coordinates: btt_model::GeoCoordinates::from_degrees(0.0, 0.0).unwrap(),
        departure_time: UnixTimeMinutes(day * 1440),
        arrival_time: UnixTimeMinutes(day * 1440 + 300),
        time_zone: "Europe/Vienna".into(),
        departure_date: date(day),
        departure_clock: chrono::NaiveTime::from_hms_opt(8, 0, 0).unwrap(),
        arrival_date: date(day + u32::from(offers_berths)),
        from_country: Some(countries.0.into()),
        to_country: Some(countries.1.into()),
        distance_kilometres: kilometres,
        offers_berths,
    }
}

fn plan(rides: Vec<RideInput>, traveller: Option<&Traveller>) -> TicketPlan {
    let data = fare_data();
    let book = PriceBook::from_data(data);
    let preferences = TicketPreferences::default();
    plan_tickets(
        &TicketRequest {
            rides,
            traveller,
            preferences: &preferences,
            today: date(0),
        },
        data,
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
    let data = fare_data();
    let book = PriceBook::from_data(data);
    let preferences = TicketPreferences::default();
    let settings = Settings {
        class: TravelClass::Second,
        age_group: btt_model::AgeGroup::Adult,
        residence_country: Some("NL"),
        preferences: &preferences,
        purchase_date: date(0),
        today: date(0),
        single_ticket_percent: 100,
    };
    let problem = Problem::new(&rides, data, &book, &settings);
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
                operator: &ride.train_label.operator,
                category: &ride.train_label.category,
                distance_kilometres: ride.distance_kilometres,
                offers_berths: ride.offers_berths,
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
        let data = fare_data();
        let book = PriceBook::from_data(data);
        let preferences = TicketPreferences::default();
        let settings = Settings {
            class: TravelClass::Second,
            age_group: btt_model::AgeGroup::Adult,
            residence_country: None,
            preferences: &preferences,
            purchase_date: date(0),
            today: date(0),
            single_ticket_percent: 100,
        };
        let problem = Problem::new(&rides, data, &book, &settings);
        let strategies = problem.strategies();
        let found = strategies.iter().map(middle_total).min().unwrap();
        let expected = exhaustive_best(&rides, data, &book);
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
    assert!(link.url.contains("osc=RG_&dsc=RMT&jt=single&od=01/07/2026"));
}

#[test]
fn a_swiss_ticket_gets_an_sbb_search_and_a_live_price_request() {
    let mut swiss = ride(0, ("CH", "CH"), "IC", 100.0, false);
    swiss.from_coordinates = btt_model::GeoCoordinates::from_degrees(47.5474, 7.5896).unwrap();
    swiss.to_coordinates = btt_model::GeoCoordinates::from_degrees(46.9490, 7.4392).unwrap();
    let plan = plan(vec![swiss], None);
    let item = &plan.strategies[0].items[0];
    let link = item.booking_link.as_ref().unwrap();
    // The booking site's own search form reads the link back to the same stations.
    let search = btt_quotes::sbb::Search::from_link(&link.url).unwrap();
    assert_eq!(search.origin.id, "8500010");
    assert_eq!(search.destination.id, "8507000");
    assert_eq!(search.time.to_string(), "08:00:00");
    let request = item.live_quote_request.as_ref().unwrap();
    assert_eq!(request.booking_link, link.url);
    assert_eq!(request.ride_count, 1);
    // Without a traveller the plan assumes one adult in second class, whose fare the price
    // model learns from.
    let run = request.run.as_ref().unwrap();
    assert_eq!(run.rides.len(), 1);
    assert_eq!(run.rides[0].category, "IC");
    assert_eq!(run.rides[0].distance_metres, 100_000);
    assert_eq!(run.local_departure.to_string(), "2026-07-01 08:00:00");
}

#[test]
fn live_prices_off_asks_for_no_live_price() {
    let mut swiss = ride(0, ("CH", "CH"), "IC", 100.0, false);
    swiss.from_coordinates = btt_model::GeoCoordinates::from_degrees(47.5474, 7.5896).unwrap();
    swiss.to_coordinates = btt_model::GeoCoordinates::from_degrees(46.9490, 7.4392).unwrap();
    let data = fare_data();
    let book = PriceBook::from_data(data);
    let preferences = TicketPreferences {
        live_prices: false,
        ..TicketPreferences::default()
    };
    let plan = plan_tickets(
        &TicketRequest {
            rides: vec![swiss],
            traveller: None,
            preferences: &preferences,
            today: date(0),
        },
        data,
        &book,
    );
    assert!(plan.strategies[0].items[0].live_quote_request.is_none());
}

#[test]
fn a_german_ticket_gets_a_db_search_with_eva_numbers_and_a_live_price_request() {
    let mut german = ride(0, ("DE", "DE"), "ICE", 290.0, false);
    german.from_coordinates = btt_model::GeoCoordinates::from_degrees(52.5256, 13.3696).unwrap();
    german.to_coordinates = btt_model::GeoCoordinates::from_degrees(53.5527, 10.0069).unwrap();
    let plan = plan(vec![german], None);
    let item = &plan.strategies[0].items[0];
    let link = item.booking_link.as_ref().unwrap();
    assert!(
        link.url
            .starts_with("https://int.bahn.de/en/buchung/fahrplan/suche#")
    );
    assert!(!link.url.contains(char::is_whitespace), "{}", link.url);
    assert!(
        link.url
            .contains("&so=Berlin%20Hbf&zo=Hamburg%20Hbf&kl=2&r=13:16:KLASSENLOS:1")
    );
    assert!(
        link.url
            .contains("&soei=8011160&zoei=8002549&hd=2026-07-01T08:00:00&hza=D")
    );
    assert!(
        link.url
            .contains("&soid=A%3D1%40O%3DBerlin%20Hbf%40L%3D8011160%40")
    );
    assert_eq!(
        item.live_quote_request.as_ref().unwrap().booking_link,
        link.url
    );
}

/// Plans one ride between two positions in a country and returns its booking link and whether
/// it asks for a live price.
fn single_ride_link(
    country: &str,
    category: &str,
    from: (f64, f64),
    to: (f64, f64),
) -> (String, bool) {
    let mut ride = ride(0, (country, country), category, 200.0, false);
    ride.from_coordinates = btt_model::GeoCoordinates::from_degrees(from.0, from.1).unwrap();
    ride.to_coordinates = btt_model::GeoCoordinates::from_degrees(to.0, to.1).unwrap();
    let plan = plan(vec![ride], None);
    let item = &plan.strategies[0].items[0];
    let link = item.booking_link.as_ref().unwrap().url.to_string();
    assert!(!link.contains(char::is_whitespace), "{link}");
    (link, item.live_quote_request.is_some())
}

#[test]
fn each_country_goes_to_its_own_booking_site() {
    let (link, live) = single_ride_link("IT", "FR", (44.4171, 8.9208), (44.5061, 11.3434));
    assert!(link.contains("handoff?action=searchTickets"), "{link}");
    assert!(
        link.contains("departureStation=Genova+Piazza+Principe&arrivalStation=Bologna+Centrale")
    );
    assert!(link.contains("departureDate=01-07-2026&departureTime=08%3A00"));
    assert!(live);
    let (link, _) = single_ride_link("AT", "RJX", (47.8116, 16.2343), (46.6156, 14.3136));
    assert!(
        link.contains("stationOrigEva=8100516&stationDestEva=8100085"),
        "{link}"
    );
    assert!(link.contains("outwardDateTime=2026-07-01T08:00"));
    let (link, _) = single_ride_link("PL", "IC", (52.4020, 16.9109), (52.2289, 21.0032));
    assert!(
        link.contains("swyj=5100081&sprzy=5100065&time=08:00"),
        "{link}"
    );
    let (link, _) = single_ride_link("NL", "IC", (52.3791, 4.9003), (51.8418, 5.8517));
    assert!(
        link.contains("travelDate=2026-07-01&fromUicCode=8400058&toUicCode=8400470"),
        "{link}"
    );
}

#[test]
fn a_french_ticket_goes_to_sbb_and_a_luxembourg_ride_is_free() {
    let (link, live) = single_ride_link("FR", "TGV INOUI", (48.8443, 2.3744), (45.7606, 4.8594));
    assert!(
        link.contains("stops=Paris+Gare+de+Lyon_I8768600~Lyon+Part-Dieu_I8772319"),
        "{link}"
    );
    assert!(live);
    let mut ride = ride(0, ("LU", "LU"), "RE", 30.0, false);
    ride.from_coordinates = btt_model::GeoCoordinates::from_degrees(49.6000, 6.1342).unwrap();
    ride.to_coordinates = btt_model::GeoCoordinates::from_degrees(49.4958, 5.9806).unwrap();
    let plan = plan(vec![ride], None);
    assert_eq!(plan.strategies[0].total.high, EuroCents::ZERO);
}

#[test]
fn a_flixtrain_with_an_ic_category_is_not_covered_by_a_pass() {
    let mut flix = ride(0, ("DE", "DE"), "IC", 600.0, false);
    flix.train_label.operator = "flixtrain".into();
    let data = fare_data();
    let book = PriceBook::from_data(data);
    let preferences = TicketPreferences::default();
    let settings = Settings {
        class: TravelClass::Second,
        age_group: btt_model::AgeGroup::Adult,
        residence_country: None,
        preferences: &preferences,
        purchase_date: date(0),
        today: date(0),
        single_ticket_percent: 100,
    };
    let rides = [flix];
    let problem = Problem::new(&rides, data, &book, &settings);
    assert_eq!(problem.pass_covers_ride(), vec![false]);
}

#[test]
fn an_italo_ride_gets_a_ticket_of_its_own() {
    let mut italo = ride(0, ("IT", "IT"), "Italo", 570.0, false);
    italo.train_label.operator = "italo".into();
    let mut intercity = ride(0, ("IT", "IT"), "ICN", 900.0, false);
    intercity.train_label.operator = "trenitalia".into();
    let mut regional = ride(0, ("IT", "IT"), "R", 40.0, false);
    regional.train_label.operator = "trenitalia".into();
    let plan = plan(vec![italo, intercity, regional], None);
    let singles = plan
        .strategies
        .iter()
        .find(|strategy| strategy.kind == StrategyKind::SingleTickets)
        .unwrap();
    let runs: Vec<&[u16]> = singles
        .items
        .iter()
        .filter(|item| item.product == TicketProduct::SingleTicket)
        .map(|item| item.ride_indices.as_slice())
        .collect();
    assert_eq!(runs, [&[0][..], &[1, 2][..]]);
}

/// Plans one ride between two countries and returns its booking link.
fn border_ride_link(countries: (&str, &str), from: (f64, f64), to: (f64, f64)) -> String {
    let mut ride = ride(0, countries, "EC", 300.0, false);
    ride.from_coordinates = btt_model::GeoCoordinates::from_degrees(from.0, from.1).unwrap();
    ride.to_coordinates = btt_model::GeoCoordinates::from_degrees(to.0, to.1).unwrap();
    let plan = plan(vec![ride], None);
    plan.strategies[0].items[0]
        .booking_link
        .as_ref()
        .unwrap()
        .url
        .to_string()
}

#[test]
fn a_ticket_across_a_border_goes_to_the_site_of_its_departure_country() {
    let zurich = (47.3782, 8.5392);
    let rogoredo = (45.4337, 9.2390);
    let link = border_ride_link(("CH", "IT"), zurich, rogoredo);
    assert!(
        link.contains("stops=Z%C3%BCrich+HB_I8503000~Milano+Rogoredo_I8301820"),
        "{link}"
    );
    let link = border_ride_link(("IT", "CH"), rogoredo, zurich);
    assert!(link.contains("arrivalStation=Z%C3%BCrich+HB"), "{link}");
    let link = border_ride_link(("AT", "DE"), (48.1851, 16.3780), (48.1402, 11.5600));
    assert!(link.starts_with("https://shop.oebbtickets.at/"), "{link}");
    // No site of Sweden is known, so DB gets the ticket.
    let link = border_ride_link(("SE", "DE"), (59.3300, 18.0576), (53.5527, 10.0069));
    assert!(link.starts_with("https://int.bahn.de/"), "{link}");
}

/// Plans one ride of an operator and returns its booking link and the search for its live
/// price.
fn site_ride(
    countries: (&str, &str),
    operator: &str,
    from: (f64, f64),
    to: (f64, f64),
) -> (String, Option<btt_model::SiteSearch>) {
    let mut ride = ride(0, countries, "IC", 300.0, false);
    ride.train_label.operator = operator.into();
    ride.from_coordinates = btt_model::GeoCoordinates::from_degrees(from.0, from.1).unwrap();
    ride.to_coordinates = btt_model::GeoCoordinates::from_degrees(to.0, to.1).unwrap();
    let plan = plan(vec![ride], None);
    let item = &plan.strategies[0].items[0];
    (
        item.booking_link.as_ref().unwrap().url.to_string(),
        item.live_quote_request
            .as_ref()
            .and_then(|request| request.site_search.clone()),
    )
}

#[test]
fn sites_without_a_search_link_get_the_search_in_the_request() {
    let (link, search) = site_ride(("DK", "DK"), "dsb", (55.6727, 12.5646), (55.4018, 10.3860));
    assert_eq!(link, "https://www.dsb.dk/en/");
    let search = search.unwrap();
    assert_eq!(search.origin_code, "8600626");
    assert_eq!(search.destination_code, "8600512");
    assert_eq!(search.local_departure, "2026-07-01T08:00");
    let (link, search) = site_ride(("SE", "SE"), "sj", (59.3301, 18.0582), (57.7089, 11.9735));
    assert_eq!(link, "https://www.sj.se/en");
    assert_eq!(search.unwrap().destination_code, "740000002");
    let (link, search) = site_ride(("ES", "ES"), "renfe", (40.4060, -3.6898), (41.3789, 2.1404));
    assert!(link.contains("cdgoOrigen=0071%2C60000%2C60000"), "{link}");
    assert!(!link.contains(char::is_whitespace), "{link}");
    assert_eq!(search.unwrap().destination_code, "71801");
}

#[test]
fn operators_that_sell_their_own_trains_get_their_own_site() {
    let (link, search) = site_ride(
        ("SE", "DE"),
        "snalltaget",
        (59.3301, 18.0582),
        (52.5256, 13.3696),
    );
    assert_eq!(link, "https://www.snalltaget.se/en");
    assert_eq!(search.unwrap().destination_code, "Berlin");
    let (link, search) = site_ride(
        ("GB", "FR"),
        "eurostar",
        (51.5319, -0.1264),
        (48.8802, 2.3552),
    );
    assert!(
        link.contains("origin=7015400&destination=8727100"),
        "{link}"
    );
    assert!(search.is_some());
    let (link, search) = site_ride(("IT", "IT"), "italo", (45.4339, 9.2391), (41.9006, 12.5020));
    assert!(
        link.starts_with("https://biglietti.italotreno.com/"),
        "{link}"
    );
    assert_eq!(search.unwrap().origin_code, "RG_");
}

#[test]
fn trains_that_have_left_get_no_live_price_request() {
    let mut swiss = ride(0, ("CH", "CH"), "IC", 100.0, false);
    swiss.from_coordinates = btt_model::GeoCoordinates::from_degrees(47.5474, 7.5896).unwrap();
    swiss.to_coordinates = btt_model::GeoCoordinates::from_degrees(46.9490, 7.4392).unwrap();
    let data = fare_data();
    let book = PriceBook::from_data(data);
    let preferences = TicketPreferences::default();
    let plan = plan_tickets(
        &TicketRequest {
            rides: vec![swiss],
            traveller: None,
            preferences: &preferences,
            today: date(1),
        },
        data,
        &book,
    );
    assert!(plan.strategies[0].items[0].live_quote_request.is_none());
    assert!(plan.notes.iter().any(|note| note.contains("before today")));
}

#[test]
fn operators_without_a_readable_site_get_their_shop_and_no_live_price() {
    let mut westbahn = ride(0, ("AT", "AT"), "WB", 300.0, false);
    westbahn.train_label.operator = "westbahn".into();
    westbahn.from_coordinates = btt_model::GeoCoordinates::from_degrees(48.1851, 16.3780).unwrap();
    westbahn.to_coordinates = btt_model::GeoCoordinates::from_degrees(47.8131, 13.0458).unwrap();
    let plan = plan(vec![westbahn], None);
    let item = &plan.strategies[0].items[0];
    assert_eq!(
        item.booking_link.as_ref().unwrap().url,
        "https://westbahn.at/en/"
    );
    assert!(item.live_quote_request.is_none());
}

#[test]
fn an_iryo_ticket_goes_to_sbb() {
    let (link, _) = site_ride(("ES", "ES"), "iryo", (40.4060, -3.6898), (41.3789, 2.1404));
    assert!(link.starts_with("https://www.sbb.ch/en?stops="), "{link}");
    assert!(link.contains("_I7171801"), "{link}");
}

#[test]
fn a_regiojet_ticket_gets_a_search_on_regiojet() {
    let (link, search) = site_ride(
        ("CZ", "CZ"),
        "regiojet",
        (50.0829, 14.4356),
        (49.1909, 16.6125),
    );
    assert_eq!(link, "https://regiojet.com/");
    let search = search.unwrap();
    assert_eq!(search.origin_code, "372825000");
    assert_eq!(search.destination_code, "3088864001");
}
