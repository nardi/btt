//! Experiment: opens DB booking links in a window of the app and reads the connections and
//! their prices from the rendered page, to see whether the site serves prices to the app's web
//! view and how fast a lookup can be.
//!
//! `BTT_PROBE_URLS` names a file with one booking link per line. The links are opened one after
//! the other in the same window, each result goes to the file `BTT_PROBE_OUT` names, and the app
//! exits when all are done. `BTT_PROBE_HIDDEN` hides the window.

use std::io::Write as _;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// Environment variable that names the file with the links to open at startup.
pub const PROBE_URLS_VARIABLE: &str = "BTT_PROBE_URLS";

/// Environment variable that names the file the results are appended to.
const PROBE_OUT_VARIABLE: &str = "BTT_PROBE_OUT";

/// Marks the fragment of the address that carries a result back from the page.
const RESULT_MARKER: &str = "BTTPROBE=";

/// How long a lookup may take before it counts as failed.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(40);

/// How often the address of the window is read while waiting for a result.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How often the window is tried again when `WebView2` refuses to create it.
const CREATE_ATTEMPTS: u32 = 20;

/// Script that runs in every page of the window. On the DB site it waits until the connection
/// list has rendered and stopped changing, reads it, and puts the result as JSON into the
/// fragment of the address, which the app can read without any channel into the page.
///
/// The reader works from the content and the structure of the page, not from its layout or
/// wording. The head of a card is the smallest element that holds a price and at least two times
/// of day, and the card is the largest element around it that holds no other head.
/// Within a card the first two times are the departure and the arrival, the lowest amount is the
/// price, and texts that look like a train category followed by a number are the trains.
const INITIALIZATION_SCRIPT: &str = r"(function () {
  if (window.top !== window || !/(^|\.)bahn\.de$/.test(location.hostname)) { return; }
  const TIME = /^([01]\d|2[0-3]):[0-5]\d$/;
  const MONEY = /(?:€|EUR)\s*(\d{1,4}(?:[.,  ]\d{3})*[.,]\d{2})|(\d{1,4}(?:[.,  ]\d{3})*[.,]\d{2})\s*(?:€|EUR)/;
  const TRAIN = /^([A-ZÄÖÜ]{1,5})\s?(\d{1,6})$/;
  const parseAmount = (text) => {
    const digits = text.replace(/[  ]/g, '');
    const decimal = Math.max(digits.lastIndexOf('.'), digits.lastIndexOf(','));
    return Number(digits.slice(0, decimal).replace(/[.,]/g, '') + '.' + digits.slice(decimal + 1));
  };
  // The texts of an element in document order, each trimmed, without the hidden ones.
  const texts = (root) => {
    const out = [];
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const text = node.nodeValue.replace(/\s+/g, ' ').trim();
      const parent = node.parentElement;
      if (text && parent && parent.getClientRects().length > 0) { out.push(text); }
    }
    return out;
  };
  const timesIn = (list) => list.filter((text) => TIME.test(text));
  const readCards = () => {
    if (!document.body) { return []; }
    const cards = new Set();
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (!MONEY.test(node.nodeValue)) { continue; }
      let element = node.parentElement;
      while (element && element !== document.body && timesIn(texts(element)).length < 2) {
        element = element.parentElement;
      }
      if (element && element !== document.body) { cards.add(element); }
    }
    // The smallest element with a price and two times is the head of a card. A head that holds
    // another head is a list, not a card.
    const heads = [...cards].filter((card) => ![...cards].some((other) => other !== card && card.contains(other)));
    // The whole card is the largest element around its head that holds no other head, so it
    // takes in the parts beside the head, such as the trains.
    const list = heads.map((head) => {
      let element = head;
      while (element.parentElement && element.parentElement !== document.body
        && !heads.some((other) => other !== head && element.parentElement.contains(other))) {
        element = element.parentElement;
      }
      return element;
    });
    return list.map((card) => {
      const all = texts(card);
      const times = timesIn(all);
      const amounts = all.flatMap((text) => {
        const match = MONEY.exec(text);
        return match ? [parseAmount(match[1] || match[2])] : [];
      });
      const trains = [];
      for (const text of all) {
        const match = TRAIN.exec(text);
        if (match && !trains.includes(match[1] + ' ' + match[2])) { trains.push(match[1] + ' ' + match[2]); }
      }
      return { departure: times[0], arrival: times[1], trains, price: amounts.length ? Math.min(...amounts) : null };
    });
  };
  const report = (result) => {
    try {
      history.replaceState(history.state, '', location.pathname + location.search + '#BTTPROBE=' + encodeURIComponent(JSON.stringify(result)));
    } catch (e) {}
  };
  const started = performance.now();
  let last = null;
  let stableSince = 0;
  let firstSeen = null;
  const check = () => {
    if (location.hash.startsWith('#BTTPROBE=')) { return; }
    const cards = readCards().filter((card) => card.departure && card.price !== null);
    const key = JSON.stringify(cards);
    const now = performance.now();
    if (key !== last) { last = key; stableSince = now; }
    if (cards.length > 0 && firstSeen === null) { firstSeen = now; }
    if (cards.length > 0 && now - stableSince >= 250) {
      report({ cards, milliseconds: Math.round(now - started), first_cards_milliseconds: Math.round(firstSeen - started), loaded_milliseconds: Math.round(performance.timing.domContentLoadedEventEnd - performance.timing.navigationStart) });
    } else {
      setTimeout(check, 100);
    }
  };
  setTimeout(check, 150);
})();
";

/// A connection as the page shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeCard {
    /// Departure time of day, such as `13:37`.
    pub departure: String,
    /// Arrival time of day.
    pub arrival: Option<String>,
    /// The trains, such as `ICE 600`.
    pub trains: Vec<String>,
    /// The lowest price on the card, in euro.
    pub price: f64,
}

/// What the page reported for one link.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PageResult {
    /// The connections, as the page reported them.
    cards: Vec<serde_json::Value>,
    /// Milliseconds from the start of the script to the result.
    milliseconds: u64,
    /// Milliseconds from the start of the script until the first cards were read.
    #[serde(default)]
    first_cards_milliseconds: u64,
    /// Milliseconds from the start of the navigation until the document was loaded.
    #[serde(default)]
    loaded_milliseconds: u64,
}

/// Returns the result that the page put into the address, if any.
fn result_in(url: &tauri::Url) -> Option<PageResult> {
    let fragment = url.fragment()?;
    let encoded = fragment.strip_prefix(RESULT_MARKER)?;
    let decoded = percent_decode(encoded);
    serde_json::from_str(&decoded).ok()
}

/// Decodes `%XX` sequences of a fragment.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Some(byte) = std::str::from_utf8(&bytes[index + 1..index + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            decoded.push(byte);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

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

/// Creates the probe window. `WebView2` can refuse for a moment with "the requested resource is
/// in use" while processes of an earlier run of the app still hold its data folder, so the
/// window is tried again a few times before giving up.
fn create_window(app: &AppHandle, first_url: &str) -> Result<WebviewWindow, String> {
    let mut last_error = String::new();
    for attempt in 1..=CREATE_ATTEMPTS {
        let url = first_url.parse().map_err(|error| format!("{error}"))?;
        match WebviewWindowBuilder::new(app, "probe", WebviewUrl::External(url))
            .title("Probe")
            .inner_size(1100.0, 800.0)
            .visible(std::env::var_os("BTT_PROBE_HIDDEN").is_none())
            .initialization_script(INITIALIZATION_SCRIPT)
            .build()
        {
            Ok(window) => return Ok(window),
            Err(error) => {
                last_error = error.to_string();
                tracing::warn!(attempt, %last_error, "the probe window could not be created");
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
    Err(last_error)
}

/// Waits until the result of an earlier lookup is gone from the address of the window, so that
/// it is not read again as the result of the next one.
fn wait_until_cleared(window: &WebviewWindow) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        if window.url().ok().as_ref().and_then(result_in).is_none() {
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
        if let Some(result) = window.url().ok().as_ref().and_then(result_in) {
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
        let window = match create_window(&app, first) {
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
                // Links differ in their fragment only, which the page does not search again on,
                // so the page is loaded anew. Its scripts come from the cache, which makes this
                // faster than the first load.
                let script = format!(
                    "location.replace({}); location.reload();",
                    serde_json::Value::from(link.as_str())
                );
                let _ = window.eval(&script);
                wait_until_cleared(&window);
            }
            let result = wait_for_result(&window);
            write_out(&serde_json::json!({
                "link": index,
                "total_ms": lookup_started.elapsed().as_millis(),
                "page_ms": result.as_ref().map(|result| result.milliseconds),
                "first_cards_ms": result.as_ref().map(|result| result.first_cards_milliseconds),
                "loaded_ms": result.as_ref().map(|result| result.loaded_milliseconds),
                "cards": result.map(|result| {
                    result
                        .cards
                        .into_iter()
                        .filter_map(|card| serde_json::from_value::<ProbeCard>(card).ok())
                        .collect::<Vec<_>>()
                }),
            }));
        }
        // A clean exit lets `WebView2` release its data folder, so the next run starts at once.
        let _ = window.destroy();
        app.exit(0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_is_read_from_the_fragment() {
        let url: tauri::Url = "https://int.bahn.de/en/x#BTTPROBE=%7B%22cards%22%3A%5B%7B%22departure%22%3A%2213%3A37%22%2C%22arrival%22%3A%2216%3A41%22%2C%22trains%22%3A%5B%22ICE%20600%22%5D%2C%22price%22%3A160.96%7D%5D%2C%22milliseconds%22%3A3000%7D"
            .parse()
            .unwrap();
        let result = result_in(&url).unwrap();
        let card: ProbeCard = serde_json::from_value(result.cards[0].clone()).unwrap();
        assert_eq!(card.trains, ["ICE 600"]);
        assert!((card.price - 160.96).abs() < 1e-9);
        let other: tauri::Url = "https://int.bahn.de/en/x#sts=true".parse().unwrap();
        assert!(result_in(&other).is_none());
    }
}
