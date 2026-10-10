//! Experiment: opens DB booking links one after the other in a window of the app and writes what
//! each page shows, to check coverage and speed of the DB page reader.
//!
//! `BTT_PROBE_URLS` names a file with one booking link per line. The links are opened one after
//! the other in the same window, each result goes to the file `BTT_PROBE_OUT` names, and the app
//! exits when all are done. `BTT_PROBE_HIDDEN` hides the window.

use std::io::Write as _;
use std::time::{Duration, Instant};

use tauri::{AppHandle, WebviewWindow};

use crate::booking_page::{PageCard, PageResult, create_window, window_result};

/// Environment variable that names the file with the links to open at startup.
pub const PROBE_URLS_VARIABLE: &str = "BTT_PROBE_URLS";

/// Environment variable that names the file the results are appended to.
const PROBE_OUT_VARIABLE: &str = "BTT_PROBE_OUT";

/// Environment variable that sends the links through the live price lookup of the Tickets
/// screen instead, each as a ticket for one train at the time of its search.
const PROBE_SERVICE_VARIABLE: &str = "BTT_PROBE_SERVICE";

/// How long a lookup may take before it counts as failed.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(40);

/// How often the address of the window is read while waiting for a result.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Appends a line of JSON to the output file.
fn write_out(value: &serde_json::Value) {
    tracing::info!("probe: {value}");
    let Some(path) = std::env::var_os(PROBE_OUT_VARIABLE) else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{value}");
    }
}

/// Waits until the result of an earlier lookup is gone from the address of the window, so that
/// it is not read again as the result of the next one.
fn wait_until_cleared(window: &WebviewWindow) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        if window_result(window).is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Waits until the page in the window reports a result, or the timeout passes.
fn wait_for_result(window: &WebviewWindow) -> Option<PageResult> {
    let started = Instant::now();
    while started.elapsed() < LOOKUP_TIMEOUT {
        std::thread::sleep(POLL_INTERVAL);
        if let Some(result) = window_result(window) {
            return Some(result);
        }
    }
    None
}

/// Opens each link of the file `BTT_PROBE_URLS` names in one window, writes what each page
/// shows, and exits the app. Runs on a thread of its own, so the app starts as usual.
pub fn start(app: &AppHandle) {
    let Some(path) = std::env::var_os(PROBE_URLS_VARIABLE) else {
        return;
    };
    let app = app.clone();
    if std::env::var_os(PROBE_SERVICE_VARIABLE).is_some() {
        tauri::async_runtime::spawn(look_up_through_service(app, path));
        return;
    }
    std::thread::spawn(move || {
        let links: Vec<String> = std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        let Some(first) = links.first() else {
            app.exit(1);
            return;
        };
        let started = Instant::now();
        let visible = std::env::var_os("BTT_PROBE_HIDDEN").is_none();
        let window = match create_window(&app, "probe", first, visible) {
            Ok(window) => window,
            Err(error) => {
                write_out(&serde_json::json!({ "error": error }));
                app.exit(1);
                return;
            }
        };
        write_out(&serde_json::json!({ "window_ms": started.elapsed().as_millis() }));
        for (index, link) in links.iter().enumerate() {
            let lookup_started = Instant::now();
            if index > 0 {
                // A link that differs only in its fragment can leave the page showing the last
                // search, so the window goes through an empty page and loads the link anew. The
                // scripts of the page still come from the cache.
                let (Ok(blank), Ok(url)) = ("about:blank".parse(), link.parse()) else {
                    continue;
                };
                let _ = window.navigate(blank);
                wait_until_cleared(&window);
                let _ = window.navigate(url);
            }
            let result = wait_for_result(&window);
            write_out(&serde_json::json!({
                "link": index,
                "total_ms": lookup_started.elapsed().as_millis(),
                "page_ms": result.as_ref().map(|result| result.milliseconds),
                "first_cards_ms": result.as_ref().map(|result| result.first_cards_milliseconds),
                "loaded_ms": result.as_ref().map(|result| result.loaded_milliseconds),
                "page_text": result.as_ref().and_then(|result| result.page_text.clone()),
                "cards": result.map(|result| {
                    result
                        .cards
                        .into_iter()
                        .filter_map(|card| serde_json::from_value::<PageCard>(card).ok())
                        .collect::<Vec<_>>()
                }),
            }));
        }
        // A clean exit lets `WebView2` release its data folder, so the next run starts at once.
        let _ = window.destroy();
        app.exit(0);
    });
}

/// Sends each link through the live price lookup that the Tickets screen uses, twice, so the
/// second answer shows the cache, writes the outcomes and exits the app.
///
/// A line can add the departure of the first train in minutes since the Unix epoch and the
/// number of trains, as `link|minutes|trains`. Without them the request asks for one train.
async fn look_up_through_service(app: AppHandle, path: std::ffi::OsString) {
    use tauri::Manager as _;
    let links = std::fs::read_to_string(path).unwrap_or_default();
    let state = app.state::<crate::state::AppState>();
    for round in 0..2 {
        for (index, line) in links
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .enumerate()
        {
            let mut parts = line.split('|');
            let link = parts.next().unwrap_or_default();
            let minutes = parts.next().and_then(|minutes| minutes.parse().ok());
            let trains = parts.next().and_then(|trains| trains.parse().ok());
            let request = btt_model::LiveQuoteRequest {
                booking_link: link.into(),
                first_departure_time: btt_model::UnixTimeMinutes(minutes.unwrap_or(0)),
                ride_count: trains.unwrap_or(1),
                travel_class: btt_model::TravelClass::Second,
                site_search: None,
                run: None,
                refresh: false,
                accommodation: None,
            };
            let started = Instant::now();
            let (outcome, _) = crate::ticket_report::look_up(&state, &request).await;
            write_out(&serde_json::json!({
                "round": round,
                "link": index,
                "total_ms": started.elapsed().as_millis(),
                "outcome": outcome,
            }));
        }
    }
    app.exit(0);
}
