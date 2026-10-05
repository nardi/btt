//! Experiment: opens a booking page in a window of the app and relays the page's own price
//! answers back, to see whether a booking site serves prices to the app's web view.

// Tauri passes arguments by value, which the command signatures must match.
#![allow(clippy::needless_pass_by_value)]

use std::io::Write as _;

use tauri::{AppHandle, Manager as _, WebviewUrl, WebviewWindowBuilder};

/// Environment variable that names the page to open at startup.
pub const PROBE_URL_VARIABLE: &str = "BTT_PROBE_URL";

/// Environment variable that names the file the answers are appended to.
const PROBE_OUT_VARIABLE: &str = "BTT_PROBE_OUT";

/// Script that runs in every page of the window. It relays the answers of the price request
/// of the page, and after a few seconds the title and the address of the page.
const INITIALIZATION_SCRIPT: &str = r"
(function () {
  const summarize = (status, body) => {
    try {
      const data = JSON.parse(body);
      const rows = (data.verbindungen || []).map((v) => [
        v.verbindungsAbschnitte.map((a) => a.verkehrsmittel.name).join('+'),
        v.verbindungsAbschnitte[0].abfahrt.sollzeit.slice(11, 16),
        v.angebotsPreis && v.angebotsPreis.betrag,
      ]);
      return 'BTTPROBE ' + status + ' ' + JSON.stringify(rows);
    } catch (e) {
      return 'BTTPROBE ' + status + ' unparsed ' + String(body).slice(0, 80);
    }
  };
  const send = (url, status, body) => {
    // Rust cannot read the document title, but it can read the address of the window, so the
    // summary goes into the fragment of the address.
    try {
      const text = url === 'page' ? 'BTTPAGE ' + body : summarize(status, body);
      history.replaceState(history.state, '', location.pathname + location.search + '#' + encodeURIComponent(text));
    } catch (e) {}
    try { window.__TAURI_INTERNALS__.invoke('probe_report', { url, status, body }); } catch (e) {}
  };
  const watched = (url) => (url || '').includes('/web/api/angebote/fahrplan');
  const originalFetch = window.fetch;
  window.fetch = async function (...args) {
    const response = await originalFetch.apply(this, args);
    try {
      const url = typeof args[0] === 'string' ? args[0] : (args[0] && args[0].url) || '';
      if (watched(url)) { response.clone().text().then((text) => send(url, response.status, text)); }
    } catch (e) {}
    return response;
  };
  const originalOpen = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method, url) {
    this.addEventListener('load', () => {
      if (watched(url)) { send(String(url), this.status, this.responseText); }
    });
    return originalOpen.apply(this, arguments);
  };
  const readPrices = () => {
    const lines = (document.body ? document.body.innerText : '').split('\n').map((l) => l.trim()).filter(Boolean);
    const cards = [];
    for (let i = 0; i < lines.length; i++) {
      if (/^\d\d:\d\d$/.test(lines[i]) && lines[i + 1] === '–' && /^\d\d:\d\d$/.test(lines[i + 2] || '')) {
        const card = { departure: lines[i], arrival: lines[i + 2], trains: [], price: null };
        for (let j = i + 3; j < lines.length && !(/^\d\d:\d\d$/.test(lines[j]) && lines[j + 1] === '–'); j++) {
          const price = /^from ?€\s?([0-9]+[.,][0-9]{2})$/.exec(lines[j]);
          if (price && card.price === null) { card.price = price[1]; }
          if (/^(ICE|IC|EC|RJX?|NJ|EN|IRE?|RE|RB|S|FLX|TGV|BUS)\s?\d+$/.test(lines[j]) && !card.trains.includes(lines[j])) { card.trains.push(lines[j]); }
        }
        cards.push(card);
      }
    }
    return 'BTTPROBE cards ' + JSON.stringify(cards);
  };
  if (window.top === window) {
    for (const delay of [8000, 20000, 35000]) {
      setTimeout(() => {
        try {
          history.replaceState(history.state, '', location.pathname + location.search + '#' + encodeURIComponent(readPrices()));
        } catch (e) {}
      }, delay);
    }
  }
  if (window.top === window) {
    setTimeout(() => { if (!location.hash.startsWith('#BTTPROBE')) { send('page', 0, document.title); } }, 10000);
  }
})();
";

/// Receives an answer that the page relayed and appends it to the output file.
#[tauri::command]
#[specta::specta]
pub fn probe_report(url: String, status: u16, body: String) {
    tracing::info!(%url, status, bytes = body.len(), "probe answer");
    let Some(path) = std::env::var_os(PROBE_OUT_VARIABLE) else {
        return;
    };
    let line = serde_json::json!({ "url": url, "status": status, "body": body });
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Opens the page in a window with the relaying script.
pub fn open_probe_window(app: &AppHandle, url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = url.parse()?;
    WebviewWindowBuilder::new(app, "probe", WebviewUrl::External(parsed))
        .title("Probe")
        .inner_size(1100.0, 800.0)
        .visible(std::env::var_os("BTT_PROBE_HIDDEN").is_none())
        .initialization_script(INITIALIZATION_SCRIPT)
        .build()?;
    let window = app
        .get_webview_window("probe")
        .ok_or("the probe window did not open")?;
    std::thread::spawn(move || {
        let mut last = String::new();
        for _ in 0..90 {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let state = format!(
                "title={:?} url={}",
                window.title().unwrap_or_default(),
                window.url().map(|url| url.to_string()).unwrap_or_default()
            );
            if state != last {
                tracing::info!("probe window: {state}");
                if let Some(path) = std::env::var_os(PROBE_OUT_VARIABLE) {
                    if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                    {
                        let _ = writeln!(file, "{}", serde_json::json!({ "window": state }));
                    }
                }
                last = state;
            }
        }
    });
    Ok(())
}
