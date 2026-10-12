//! Plans the benchmark queries of MOTIS as legs, a daytime and a night leg for each, and records
//! or checks the chosen journeys and the planning times.
//!
//! Run `cargo run --release -p btt-planner --example benchmark_legs -- record <bundle>
//! <queries.txt> <responses.txt> <results.tsv>` to write the chosen journey, its cost and the
//! planning time of each leg. Run it again with `check` in place of `record` after a change to
//! the planner. The check lists every leg whose chosen journey or cost differs from the file,
//! and compares the planning times. Run it with `count` to plan every leg at the balanced and
//! the frugal price weight, which reports how many legs get a different chosen journey when
//! prices count. Run it with `times` and a limit, then the weights in hundredths of a minute
//! per euro, such as `times <bundle> <queries.txt> <responses.txt> 500 0 100 300 800`, to time
//! every leg at each weight.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Instant;

use btt_geo::TimeZoneFinder;
use btt_model::{
    DurationMinutes, EuroRange, GeoCoordinates, Journey, LegPlanStation, LegPlanningResult,
    LegTravelMode, OvernightLegOptions, PriceWeight, StationSearchRadius, TripPreferences,
};
use btt_planner::{JourneyPricer, LegEndpoint, LegPlanningInput, TimetableBundleRouter};
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// The parts of a MOTIS plan response the benchmark reads.
#[derive(Deserialize)]
struct MotisResponse {
    from: Option<MotisPlace>,
    to: Option<MotisPlace>,
}

#[derive(Deserialize)]
struct MotisPlace {
    name: String,
    lat: f64,
    lon: f64,
}

/// Prices journeys with the ticket optimizer, so the price weights of the `count` mode see the
/// prices the app plans with.
struct TicketingPricer {
    /// The tariffs, reservation rules and price model.
    data: btt_fares::FareData,
    /// The sources that price single tickets.
    book: btt_fares::PriceBook,
}

impl JourneyPricer for TicketingPricer {
    fn journey_price(&self, journey: &Journey, stations: &[LegPlanStation]) -> Option<EuroRange> {
        btt_ticketing::journey_price(journey, stations, &self.data, &self.book)
    }

    fn journey_price_with_pass(
        &self,
        journey: &Journey,
        stations: &[LegPlanStation],
    ) -> Option<EuroRange> {
        btt_ticketing::journey_price_on_pass(journey, stations, &self.data, &self.book)
    }

    fn pass_price_table(
        &self,
        classes: &[btt_planner::RidePriceClass],
    ) -> Option<btt_planner::RidePassTable> {
        if self.data.passes.passes.is_empty() {
            return None;
        }
        let rows = classes
            .iter()
            .map(|class| {
                let country = btt_fares::country_of_time_zone(&class.time_zone);
                btt_planner::RidePassRow {
                    takes: self
                        .data
                        .reservations
                        .pass_accepted(&class.category, &class.operator),
                    cents: btt_ticketing::pass_cents_for_class(
                        &class.category,
                        &class.operator,
                        country,
                        class.offers_berths,
                        &self.data,
                    ),
                }
            })
            .collect();
        Some(btt_planner::RidePassTable { rows })
    }

    fn price_table(
        &self,
        classes: &[btt_planner::RidePriceClass],
    ) -> Option<btt_planner::RidePriceTable> {
        let model = self.data.price_model.read();
        let classes: Vec<btt_fares::RouteClass> = classes
            .iter()
            .map(|class| btt_fares::RouteClass {
                operator: class.operator.clone(),
                category: class.category.clone(),
                offers_berths: class.offers_berths,
                country: btt_fares::country_of_time_zone(&class.time_zone).map(Into::into),
            })
            .collect();
        let table = btt_fares::PriceTable::from_model(&model, &classes);
        Some(btt_planner::RidePriceTable {
            rows: table
                .rows()
                .iter()
                .map(|row| btt_planner::RidePriceRow {
                    constant: row.constant,
                    distance: row.distance,
                    curve: row.curve,
                    base: row.base,
                    shift: row.shift,
                })
                .collect(),
            reference_kilometres: table.reference_kilometres(),
        })
    }
}

/// What a leg planned to.
struct LegResult {
    /// The leg's description, with its travel mode.
    description: String,
    /// The chosen journey as the trains and times of its rides, and its cost.
    journey: String,
    /// Milliseconds spent planning.
    milliseconds: f64,
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let mode = arguments.next().expect("pass record, check or count");
    let bundle_path = arguments.next().expect("pass the bundle");
    let queries_path = arguments.next().expect("pass queries.txt");
    let responses_path = arguments.next().expect("pass responses.txt");
    let limit: usize = arguments
        .next()
        .and_then(|limit| limit.parse().ok())
        .unwrap_or(usize::MAX);
    // The data files are compiled in and a test in `btt-fares` loads them, so this cannot fail.
    let data = btt_fares::FareData::bundled().expect("the bundled fare data is valid");
    let book = btt_fares::PriceBook::from_data(&data);
    let pricer = TicketingPricer { data, book };
    let router = TimetableBundleRouter::open(
        bundle_path.as_ref(),
        Arc::new(TimeZoneFinder::new()),
        pricer,
    )
    .expect("bundle loads");
    let legs = legs(&queries_path, &responses_path, limit);
    match mode.as_str() {
        "record" => {
            let results_path = arguments.next().expect("pass the results file");
            let results = plan_queries(&router, &legs);
            let mut text = String::new();
            for result in &results {
                // Writing to a string never fails.
                let _ = writeln!(
                    text,
                    "{}\t{}\t{:.1}",
                    result.description, result.journey, result.milliseconds
                );
            }
            std::fs::write(&results_path, text).expect("results write");
            print_times("recorded", &results);
        }
        "check" => {
            let results_path = arguments.next().expect("pass the results file");
            check(&plan_queries(&router, &legs), &results_path);
        }
        "count" => count_price_weights(&router, &legs),
        "times" => {
            let weights: Vec<u16> = arguments.filter_map(|weight| weight.parse().ok()).collect();
            time_price_weights(&router, &legs, &weights);
        }
        other => panic!("unknown mode {other}"),
    }
}

/// Builds the daytime and the night leg of every benchmark query, at the weight of no price.
fn legs(queries_path: &str, responses_path: &str, limit: usize) -> Vec<(String, LegPlanningInput)> {
    let preferences = TripPreferences::default();
    let queries = std::fs::read_to_string(queries_path).expect("queries read");
    let responses = std::fs::read_to_string(responses_path).expect("responses read");
    let mut legs = Vec::new();
    for (query, response) in queries.lines().zip(responses.lines()).take(limit) {
        let Some(time) = query
            .split(['?', '&'])
            .find_map(|part| part.strip_prefix("time="))
            .and_then(|time| time.parse::<DateTime<Utc>>().ok())
        else {
            continue;
        };
        let Ok(MotisResponse {
            from: Some(from),
            to: Some(to),
        }) = serde_json::from_str::<MotisResponse>(response)
        else {
            continue;
        };
        let (Ok(from_coordinates), Ok(to_coordinates)) = (
            GeoCoordinates::from_degrees(from.lat, from.lon),
            GeoCoordinates::from_degrees(to.lat, to.lon),
        ) else {
            continue;
        };
        let endpoint = |coordinates, time_window| LegEndpoint {
            coordinates,
            station_search_radius: StationSearchRadius::DEFAULT,
            time_window,
        };
        for (name, travel_mode) in [
            ("day", LegTravelMode::Daytime),
            (
                "night",
                LegTravelMode::Overnight(OvernightLegOptions::DEFAULT),
            ),
        ] {
            legs.push((
                format!(
                    "{} to {} on {}, {name}",
                    from.name,
                    to.name,
                    time.date_naive()
                ),
                LegPlanningInput {
                    departure: endpoint(from_coordinates, preferences.default_windows.departure),
                    arrival: endpoint(to_coordinates, preferences.default_windows.arrival),
                    departure_date: time.date_naive(),
                    travel_mode,
                    minimum_transfer_time: DurationMinutes(10),
                    pass_policy: preferences.pass_policy,
                    transfer_aversion: preferences.transfer_aversion,
                    price_weight: PriceWeight::TIME_FIRST,
                    chosen_journey: None,
                },
            ));
        }
    }
    legs
}

/// Plans every leg of the benchmark and times each plan.
fn plan_queries(
    router: &TimetableBundleRouter<TicketingPricer>,
    legs: &[(String, LegPlanningInput)],
) -> Vec<LegResult> {
    legs.iter()
        .map(|(description, input)| {
            let started = Instant::now();
            let result = router.plan_leg(input, || false);
            let milliseconds = started.elapsed().as_secs_f64() * 1000.0;
            LegResult {
                description: description.clone(),
                journey: describe(&result),
                milliseconds,
            }
        })
        .collect()
}

/// Plans every leg at the balanced and the frugal price weight, and prints how many legs get a
/// journey the planner would not pick by time alone.
fn count_price_weights(
    router: &TimetableBundleRouter<TicketingPricer>,
    legs: &[(String, LegPlanningInput)],
) {
    for weight in [PriceWeight::BALANCED, PriceWeight::FRUGAL] {
        let mut different = 0;
        for (_, input) in legs {
            let mut weighted = input.clone();
            weighted.price_weight = weight;
            let by_time = describe(&router.plan_leg(input, || false));
            let by_price = describe(&router.plan_leg(&weighted, || false));
            if by_time != by_price {
                different += 1;
            }
        }
        let minutes_per_euro = weight.hundredths_of_minute_per_euro() / 100;
        println!(
            "{different} of {} legs pick a different journey at {minutes_per_euro} minutes per euro",
            legs.len()
        );
    }
}

/// Plans every leg at each of the given weights, after one untimed pass that warms the caches
/// of the router, and prints the planning times and how many legs hold pass options.
fn time_price_weights(
    router: &TimetableBundleRouter<TicketingPricer>,
    legs: &[(String, LegPlanningInput)],
    weights: &[u16],
) {
    for (_, input) in legs.iter().take(20) {
        router.plan_leg(input, || false);
    }
    for &weight in weights {
        let mut results = Vec::new();
        let mut with_pass_options = 0;
        let mut with_cheapest = 0;
        for (description, input) in legs {
            let mut weighted = input.clone();
            weighted.price_weight = PriceWeight::new(weight).expect("every u16 is a weight");
            let started = Instant::now();
            let result = router.plan_leg(&weighted, || false);
            let milliseconds = started.elapsed().as_secs_f64() * 1000.0;
            if let LegPlanningResult::Found(plan) = &result {
                with_pass_options += usize::from(!plan.pass_options.is_empty());
                with_cheapest += usize::from(plan.cheapest.is_some());
            }
            results.push(LegResult {
                description: description.clone(),
                journey: describe(&result),
                milliseconds,
            });
        }
        println!(
            "weight {weight}: {with_pass_options} legs with pass options, {with_cheapest} with a cheaper journey"
        );
        print_times("now", &results);
    }
}

/// Describes the chosen journey of a result by the number, the departure and the arrival of
/// each ride, with the journey's cost.
fn describe(result: &LegPlanningResult) -> String {
    match result {
        LegPlanningResult::NoJourneyFound(reason) => format!("none {reason:?}"),
        LegPlanningResult::Found(plan) => {
            let rides: Vec<String> = plan
                .chosen_journey
                .rides()
                .map(|ride| {
                    format!(
                        "{} {}-{}",
                        ride.train_label.number, ride.departure_time.0, ride.arrival_time.0
                    )
                })
                .collect();
            format!("{} | cost {}", rides.join(", "), plan.best_journey_cost)
        }
    }
}

/// Compares the results with the recorded file.
fn check(results: &[LegResult], results_path: &str) {
    let recorded = std::fs::read_to_string(results_path).expect("results read");
    let recorded: Vec<Vec<&str>> = recorded
        .lines()
        .map(|line| line.split('\t').collect())
        .collect();
    let mut differences = 0;
    let mut recorded_results = Vec::new();
    for (result, line) in results.iter().zip(&recorded) {
        if line[0] != result.description || line[1] != result.journey {
            differences += 1;
            if differences <= 200 {
                println!(
                    "{}\n  recorded: {}\n  now:      {}",
                    result.description, line[1], result.journey
                );
            }
        }
        recorded_results.push(LegResult {
            description: line[0].to_owned(),
            journey: line[1].to_owned(),
            milliseconds: line[2].parse().unwrap_or(0.0),
        });
    }
    println!("{differences} of {} legs differ", results.len());
    print_times("recorded", &recorded_results);
    print_times("now", results);
}

/// Prints the median, the 90th percentile and the largest planning time, and the total, for
/// the daytime and the night legs.
fn print_times(name: &str, results: &[LegResult]) {
    for mode in ["day", "night"] {
        let mut times: Vec<f64> = results
            .iter()
            .filter(|result| result.description.ends_with(mode))
            .map(|result| result.milliseconds)
            .collect();
        times.sort_by(f64::total_cmp);
        let at = |fraction: f64| {
            // Rounding to an index of a short list stays well within `usize`.
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let position = (times.len().saturating_sub(1) as f64 * fraction).round() as usize;
            times.get(position).copied().unwrap_or_default()
        };
        println!(
            "  {name} {mode}: median {:.1} ms, 90th percentile {:.1} ms, largest {:.1} ms, total {:.1} s",
            at(0.5),
            at(0.9),
            at(1.0),
            times.iter().sum::<f64>() / 1000.0
        );
    }
}
