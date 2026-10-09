//! The seed run of the price model: prices rides picked across the timetable bundle on the
//! booking sites, and writes each live price as a line of JSON for `data/fares/price_seed.jsonl`.
//!
//! `BTT_PRICE_SEED_OUT` names the output file and starts the run when the app starts. The app
//! exits when the run is done. `BTT_PRICE_SEED_RIDES` sets the rides per operator, 20 by
//! default, and `BTT_PRICE_SEED_OPERATORS` can name the operators as a comma-separated list.
//! `BTT_PRICE_SEED_DAYS` sets the days ahead of the departures and `BTT_PRICE_SEED_RANDOM` the
//! seed of the random choice of rides.
//! The run does not touch the app's database, so the seed and the traveller's own prices stay
//! apart.

use std::collections::VecDeque;
use std::io::Write as _;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use btt_model::{
    LiveQuoteRequest, PriceObservation, StrategyKind, TicketPreferences, TicketProduct, TripPlaceId,
};
use btt_planner::{FareSampleRide, LegPlanningBackend};
use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use tauri::{AppHandle, Manager as _};

use crate::state::AppState;
use crate::ticket_report::LiveQuoteOutcome;

/// Environment variable that names the output file and starts the run.
const OUTPUT_VARIABLE: &str = "BTT_PRICE_SEED_OUT";

/// Environment variable with the number of rides per operator.
const RIDES_VARIABLE: &str = "BTT_PRICE_SEED_RIDES";

/// Environment variable with the operators to sample, as a comma-separated list.
const OPERATORS_VARIABLE: &str = "BTT_PRICE_SEED_OPERATORS";

/// Environment variable with the days between today and the departures, as a comma-separated
/// list.
const DAYS_VARIABLE: &str = "BTT_PRICE_SEED_DAYS";

/// Environment variable with the seed of the random choice of rides, so a second run can pick
/// other rides than the first.
const RANDOM_SEED_VARIABLE: &str = "BTT_PRICE_SEED_RANDOM";

/// Operators whose trains a booking site with live prices sells, by the ids of
/// `data/operators.toml` or, for operators the file does not list, the name the app derives from
/// their agency.
const DEFAULT_OPERATORS: &[&str] = &[
    "db-fernverkehr",
    "db-regio",
    "oebb",
    "sbb",
    "bls",
    "sob",
    "sncf",
    "lyria",
    "trenitalia",
    "italo",
    "ns",
    "pkp-intercity",
    "cd",
    "regiojet",
    "dsb",
    "sj",
    "snalltaget",
    "mav",
    "renfe",
    "iryo",
    "eurostar",
    "polregio",
    "koleje-mazowieckie",
    "koleje-slaskie",
    "koleje-wielkopolskie",
    "koleje-dolnoslaskie",
];

/// Days between today and the departures the run picks, so the seed holds prices bought at
/// several distances from the departure.
const DAYS_AHEAD: &[u64] = &[3, 10, 21, 40];

/// How often a ride is tried while its site pauses requests after a failure.
const MAXIMUM_ATTEMPTS: u8 = 3;

/// How long the run leaves a site alone after it paused requests. The sites pause for fifteen
/// minutes after a failure.
const PAUSE_WAIT: Duration = Duration::from_mins(16);

/// Starts the seed run when `BTT_PRICE_SEED_OUT` is set.
pub fn start(app: &AppHandle) {
    let Some(path) = std::env::var_os(OUTPUT_VARIABLE) else {
        return;
    };
    tauri::async_runtime::spawn(run_seed(app.clone(), path));
}

/// Picks the rides, looks up their prices one after the other and writes the prices, then
/// exits the app.
async fn run_seed(app: AppHandle, path: std::ffi::OsString) {
    let state = app.state::<AppState>();
    let LegPlanningBackend::TimetableBundle(router) = state.planning_backend.as_ref() else {
        tracing::error!("the seed run needs a timetable bundle");
        app.exit(1);
        return;
    };
    let per_operator = std::env::var(RIDES_VARIABLE)
        .ok()
        .and_then(|rides| rides.parse().ok())
        .unwrap_or(20);
    let named = std::env::var(OPERATORS_VARIABLE).unwrap_or_default();
    let operators: Vec<&str> = if named.is_empty() {
        DEFAULT_OPERATORS.to_vec()
    } else {
        named.split(',').map(str::trim).collect()
    };
    let today = crate::commands::today_utc();
    let days_ahead: Vec<u64> = std::env::var(DAYS_VARIABLE).ok().map_or_else(
        || DAYS_AHEAD.to_vec(),
        |list| {
            list.split(',')
                .filter_map(|days| days.trim().parse().ok())
                .collect()
        },
    );
    let days: Vec<_> = days_ahead
        .iter()
        .filter_map(|&days| today.checked_add_days(chrono::Days::new(days)))
        .collect();
    let random_seed = std::env::var(RANDOM_SEED_VARIABLE)
        .ok()
        .and_then(|seed| seed.parse().ok())
        .unwrap_or(0x5EED);
    let samples = router.fare_sample_rides(&operators, per_operator, &days, random_seed);
    tracing::info!(rides = samples.len(), "seed run: picked rides");
    let Ok(mut output) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        tracing::error!("seed run: the output file does not open");
        app.exit(1);
        return;
    };
    let mut queue = lookup_queue(&state, &samples, today);
    // The moment until which the site of each operator pauses requests.
    let mut paused_until: FxHashMap<SmolStr, Instant> = FxHashMap::default();
    let (mut asked, mut priced) = (0, 0);
    while let Some((operator, request, attempts)) = queue.pop_front() {
        let now = Instant::now();
        let waiting =
            |operator: &SmolStr| paused_until.get(operator).is_some_and(|&until| until > now);
        if waiting(&operator) {
            // When every ride left waits for a paused site, the run waits for the first site to
            // open again.
            if queue.iter().all(|(other, ..)| waiting(other)) {
                let first_open = paused_until.values().filter(|&&until| until > now).min();
                if let Some(&until) = first_open {
                    tokio::time::sleep_until(until.into()).await;
                }
            }
            queue.push_back((operator, request, attempts));
            continue;
        }
        asked += 1;
        let (outcome, answer) = crate::ticket_report::look_up(&state, &request).await;
        tracing::info!(operator = %operator, ?outcome, "seed run: looked up");
        log_missing_price(&request, &outcome, answer);
        let paused =
            matches!(&outcome, LiveQuoteOutcome::Failed { error } if error.contains("paused"));
        if paused && attempts < MAXIMUM_ATTEMPTS {
            paused_until.insert(operator.clone(), Instant::now() + PAUSE_WAIT);
            queue.push_back((operator, request, attempts + 1));
            continue;
        }
        let (LiveQuoteOutcome::Quoted { price }, Some(run)) = (outcome, request.run) else {
            continue;
        };
        let observation = PriceObservation {
            run,
            price: price.low,
            original: price.original,
            looked_up_on: today,
            source: price.source.name,
        };
        if let Ok(line) = serde_json::to_string(&observation) {
            let _ = writeln!(output, "{line}");
            priced += 1;
        }
    }
    tracing::info!(asked, priced, "seed run: done");
    app.exit(0);
}

/// Logs the planned trip and what the site answered when a lookup ends without a price, to find
/// why it gave none.
fn log_missing_price(
    request: &LiveQuoteRequest,
    outcome: &LiveQuoteOutcome,
    answer: Option<serde_json::Value>,
) {
    if matches!(
        outcome,
        LiveQuoteOutcome::NoMatchingTrip | LiveQuoteOutcome::Failed { .. }
    ) {
        tracing::info!(
            search = ?request.site_search,
            rides = request.ride_count,
            answer = %answer.map(|answer| answer.to_string()).unwrap_or_default(),
            "seed run: no price"
        );
    }
}

/// Returns the live price request of every ride a booking site can price, with the operator of
/// the ride and the number of tries so far. The operators take turns, so a site that pauses after
/// a failure has time to recover while the others go on.
fn lookup_queue(
    state: &AppState,
    samples: &[FareSampleRide],
    today: chrono::NaiveDate,
) -> VecDeque<(SmolStr, LiveQuoteRequest, u8)> {
    let preferences = TicketPreferences::default();
    // The live price request of each ride, by operator.
    let mut requests_by_operator: Vec<(SmolStr, Vec<LiveQuoteRequest>)> = Vec::new();
    for sample in samples {
        let Some(ride) =
            btt_ticketing::ride_input(TripPlaceId(NonZeroU32::MIN), &sample.ride, &sample.stations)
        else {
            continue;
        };
        let operator = ride.train_label.operator.clone();
        let plan = btt_ticketing::plan_tickets(
            &btt_ticketing::TicketRequest {
                rides: vec![ride],
                traveller: None,
                preferences: &preferences,
                today,
            },
            &state.fare_data,
            &state.price_book,
        );
        let Some(request) = plan
            .strategies
            .iter()
            .filter(|strategy| strategy.kind == StrategyKind::SingleTickets)
            .flat_map(|strategy| &strategy.items)
            .find(|item| item.product == TicketProduct::SingleTicket)
            .and_then(|item| item.live_quote_request.clone())
        else {
            continue;
        };
        match requests_by_operator
            .iter_mut()
            .find(|(known, _)| *known == operator)
        {
            Some((_, requests)) => requests.push(request),
            None => requests_by_operator.push((operator, vec![request])),
        }
    }
    let mut queue: VecDeque<(SmolStr, LiveQuoteRequest, u8)> = VecDeque::new();
    let longest = requests_by_operator
        .iter()
        .map(|(_, requests)| requests.len())
        .max()
        .unwrap_or(0);
    for turn in 0..longest {
        for (operator, requests) in &requests_by_operator {
            if let Some(request) = requests.get(turn) {
                queue.push_back((operator.clone(), request.clone(), 0));
            }
        }
    }
    queue
}
