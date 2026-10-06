//! Reads the connections and prices that a booking site shows, from a window of the app.
//!
//! The DB site, the ÖBB shop and PKP Intercity refuse scripted clients but serve their pages to the app's own web view. A window
//! opens a booking link, and a script in the page waits until the connection list has rendered,
//! reads it, and puts the result into the fragment of the page's address, where the app reads it.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// Marks the fragment of the address that carries a result back from the page.
const RESULT_MARKER: &str = "BTTPROBE=";

/// How often a window is tried again when `WebView2` refuses to create it.
const CREATE_ATTEMPTS: u32 = 20;

/// Script that runs in every page of a reading window. On the sites it reads, it waits until the connection
/// list has rendered and stopped changing, reads it, and puts the result as JSON into the
/// fragment of the address, which the app can read without any channel into the page.
///
/// The reader works from the content and the structure of the page, not from its layout or
/// wording. The head of a card is the smallest element around a time of day that holds two
/// times, and the card is the largest element around its head that holds no other head.
/// Within a card the first two times are the departure and the arrival, the lowest amount is the
/// price, with its currency in the same text or the next one, and texts that look like a train category followed by a number are the trains.
pub(crate) const INITIALIZATION_SCRIPT: &str = r"(function () {
  if (window.top !== window || !/(^|\.)(bahn\.de|oebbtickets\.at|intercity\.pl)$/.test(location.hostname)) { return; }
  const TIME = /^([01]\d|2[0-3]):[0-5]\d$/;
  const NUMBER = '(\\d{1,4}(?:[.,\\u202f\\u00a0]\\d{3})*[.,]\\d{2})';
  const CURRENCY = '(€|EUR|zł|PLN|CHF|Fr\\.)';
  const MONEY_BEFORE = new RegExp(CURRENCY + '\\s*' + NUMBER);
  const MONEY_AFTER = new RegExp(NUMBER + '\\s*' + CURRENCY);
  const AMOUNT_ONLY = new RegExp('^' + NUMBER + '$');
  const CURRENCY_ONLY = new RegExp('^' + CURRENCY + '$');
  const currencyCode = (symbol) => ({ '€': 'EUR', 'zł': 'PLN', 'Fr.': 'CHF' })[symbol] || symbol;
  const TRAIN = /^([A-ZÄÖÜ]{1,5})\s?(\d{1,6})$/;
  const parseAmount = (text) => {
    const digits = text.replace(/[\u202f\u00a0]/g, '');
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
    // The head of a card is the smallest element around a time of day that holds two times,
    // the departure and the arrival. Heads come from times alone, so a card without a price
    // still counts and keeps its neighbours apart.
    const found = new Set();
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (!TIME.test(node.nodeValue.trim())) { continue; }
      let element = node.parentElement;
      while (element && element !== document.body && timesIn(texts(element)).length < 2) {
        element = element.parentElement;
      }
      if (element && element !== document.body) { found.add(element); }
    }
    // A head that holds another head is a list, not a card.
    const heads = [...found].filter((head) => ![...found].some((other) => other !== head && head.contains(other)));
    // The whole card is the largest element around its head that holds no other head, so it
    // takes in the parts beside the head, such as the price and the trains.
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
      const amounts = [];
      all.forEach((text, index) => {
        const before = MONEY_BEFORE.exec(text);
        const after = MONEY_AFTER.exec(text);
        if (before) {
          amounts.push({ amount: parseAmount(before[2]), currency: currencyCode(before[1]) });
        } else if (after) {
          amounts.push({ amount: parseAmount(after[1]), currency: currencyCode(after[2]) });
        } else if (AMOUNT_ONLY.test(text) && CURRENCY_ONLY.test(all[index + 1] || '')) {
          amounts.push({ amount: parseAmount(text), currency: currencyCode(all[index + 1]) });
        }
      });
      const lowest = amounts.reduce((best, entry) => (best === null || entry.amount < best.amount ? entry : best), null);
      const trains = [];
      for (const text of all) {
        const match = TRAIN.exec(text);
        if (match && !trains.includes(match[1] + ' ' + match[2])) { trains.push(match[1] + ' ' + match[2]); }
      }
      return { departure: times[0], arrival: times[1], trains, price: lowest && lowest.amount, currency: lowest && lowest.currency };
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
    const cards = readCards().filter((card) => card.departure);
    const priced = cards.filter((card) => card.price !== null);
    const key = JSON.stringify(cards);
    const now = performance.now();
    if (key !== last) { last = key; stableSince = now; }
    if (priced.length > 0 && firstSeen === null) { firstSeen = now; }
    // A page without prices reports once its cards have stood still for three seconds, or
    // after twenty seconds when it shows no cards at all.
    if (priced.length === 0 && ((cards.length > 0 && now - stableSince >= 3000) || now - started > 20000)) {
      report({ cards, milliseconds: Math.round(now - started), page_text: texts(document.body).slice(0, 80) });
    } else if (priced.length > 0 && now - stableSince >= 250) {
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
pub struct PageCard {
    /// Departure time of day, such as `13:37`.
    pub departure: String,
    /// Arrival time of day.
    pub arrival: Option<String>,
    /// The trains, such as `ICE 600`.
    pub trains: Vec<String>,
    /// The lowest price on the card, or `None` when the site sells no ticket for it.
    pub price: Option<f64>,
    /// The ISO code of the currency of the price, such as `EUR` or `PLN`.
    #[serde(default)]
    pub currency: Option<String>,
}

/// What the page reported for one link.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PageResult {
    /// The connections, as the page reported them.
    pub(crate) cards: Vec<serde_json::Value>,
    /// Milliseconds from the start of the script to the result.
    pub(crate) milliseconds: u64,
    /// Milliseconds from the start of the script until the first cards were read.
    #[serde(default)]
    pub(crate) first_cards_milliseconds: u64,
    /// Milliseconds from the start of the navigation until the document was loaded.
    #[serde(default)]
    pub(crate) loaded_milliseconds: u64,
    /// The first texts of the page, when it showed no connections.
    #[serde(default)]
    pub(crate) page_text: Option<Vec<String>>,
}

/// Returns the result that the page put into the address, if any.
pub(crate) fn result_in(url: &tauri::Url) -> Option<PageResult> {
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

/// Returns the result that the page in a window has reported, if it has.
pub(crate) fn window_result(window: &WebviewWindow) -> Option<PageResult> {
    window.url().ok().as_ref().and_then(result_in)
}

/// Creates a reading window with the script. `WebView2` can refuse for a moment with "the requested resource is
/// in use" while processes of an earlier run of the app still hold its data folder, so the
/// window is tried again a few times before giving up.
pub(crate) fn create_window(
    app: &AppHandle,
    label: &str,
    first_url: &str,
    visible: bool,
) -> Result<WebviewWindow, String> {
    let mut last_error = String::new();
    for attempt in 1..=CREATE_ATTEMPTS {
        let url = first_url.parse().map_err(|error| format!("{error}"))?;
        match WebviewWindowBuilder::new(app, label, WebviewUrl::External(url))
            .title("Booking site")
            .inner_size(1100.0, 800.0)
            .visible(visible)
            .initialization_script(INITIALIZATION_SCRIPT)
            .build()
        {
            Ok(window) => return Ok(window),
            Err(error) => {
                last_error = error.to_string();
                tracing::warn!(attempt, %last_error, "a reading window could not be created");
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
    Err(last_error)
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
        let card: PageCard = serde_json::from_value(result.cards[0].clone()).unwrap();
        assert_eq!(card.trains, ["ICE 600"]);
        assert!((card.price.unwrap() - 160.96).abs() < 1e-9);
        let other: tauri::Url = "https://int.bahn.de/en/x#sts=true".parse().unwrap();
        assert!(result_in(&other).is_none());
    }
}
