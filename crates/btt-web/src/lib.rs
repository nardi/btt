//! The BTT backend for the browser.
//!
//! The web app runs this module in a worker. The page calls `invoke` with the name of a command
//! and its arguments as JSON, which are the same commands and arguments the desktop app has. The
//! module has no live prices, because booking sites do not allow requests from a page.

mod host;
mod state;
mod storage;

use std::rc::Rc;

use btt_model::CommandError;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use wasm_bindgen::prelude::*;

use crate::host::Host;
use crate::state::WebBackend;

thread_local! {
    /// The backend of this worker, once `start` has run.
    static BACKEND: std::cell::RefCell<Option<Rc<WebBackend>>> = const { std::cell::RefCell::new(None) };
}

/// Starts the backend.
///
/// `send_message` receives the events of a channel as `(channel_id, json)`. `save_state` receives
/// the whole stored state as JSON whenever it changes, and `save_file` receives the name and the
/// text of a file the traveller exports. `save_leg` receives the timetable namespace, the cache key
/// and the JSON of each planned leg. `stored_state` is the text `save_state` received last, or
/// `undefined` on the first start.
///
/// # Errors
///
/// Returns a message when the stored state cannot be read.
// The glue code passes optional text by value, so the arguments cannot be references.
#[allow(clippy::needless_pass_by_value)]
#[wasm_bindgen]
pub fn start(
    send_message: js_sys::Function,
    save_state: js_sys::Function,
    save_file: js_sys::Function,
    save_leg: js_sys::Function,
    stored_state: Option<String>,
) -> Result<(), String> {
    console_error_panic_hook::set_once();
    let host = Host::new(send_message, save_state, save_file, save_leg);
    let backend = WebBackend::new(host, stored_state.as_deref())?;
    BACKEND.with(|slot| *slot.borrow_mut() = Some(Rc::new(backend)));
    Ok(())
}

/// Makes the planner use a timetable bundle, given as the bytes of the unpacked bundle file.
///
/// # Errors
///
/// Returns a message when the bytes are not a bundle this build reads.
#[wasm_bindgen]
pub fn install_bundle(bytes: &[u8]) -> Result<(), String> {
    backend()?.install_bundle(bytes)
}

/// Returns the namespace of the timetable in use, which keys the stored legs.
///
/// # Errors
///
/// Returns a message when no timetable is installed.
#[wasm_bindgen]
pub fn leg_cache_namespace() -> Result<String, String> {
    backend()?.leg_cache_namespace()
}

/// Keeps the planned legs that the page read back from the browser's storage. `entries_json` is
/// an array of objects with the cache `key` in decimal and the planned leg as JSON `result`.
///
/// # Errors
///
/// Returns a message when the entries are not an array of such objects.
#[wasm_bindgen]
pub fn load_leg_results(entries_json: &str) -> Result<(), String> {
    backend()?.load_leg_results(entries_json)
}

/// Records the manifest of the timetables on the server, or the reason the check for it failed.
///
/// # Errors
///
/// Returns a message when the backend has not started or the manifest is not valid JSON.
// The glue code passes optional text by value, so the arguments cannot be references.
#[allow(clippy::needless_pass_by_value)]
#[wasm_bindgen]
pub fn record_manifest(
    manifest_json: Option<String>,
    check_error: Option<String>,
) -> Result<(), String> {
    backend()?.record_manifest(manifest_json.as_deref(), check_error)
}

/// Lists the places of a trip that still have the name made from their coordinates, as JSON.
/// The page looks up the town of each and names the place with it.
///
/// # Errors
///
/// Returns a message when the text is not a trip.
#[wasm_bindgen]
pub fn provisional_places(trip_json: &str) -> Result<String, String> {
    let trip: btt_model::Trip =
        serde_json::from_str(trip_json).map_err(|error| error.to_string())?;
    let places: Vec<serde_json::Value> = trip
        .itinerary
        .trip_places()
        .filter(|place| btt_backend::place_naming::has_provisional_name(place))
        .map(|place| {
            serde_json::json!({
                "placeId": place.id,
                "coordinates": place.coordinates,
                "latitude": place.coordinates.latitude.degrees(),
                "longitude": place.coordinates.longitude.degrees(),
            })
        })
        .collect();
    Ok(serde_json::Value::Array(places).to_string())
}

/// Returns the address of a Photon name search, or `None` for a query that makes no address.
#[wasm_bindgen]
pub fn photon_search_url(query: &str, language: &str) -> Option<String> {
    btt_geo::search_request_url(btt_geo::PUBLIC_PHOTON_BASE_URL, query, language)
}

/// Returns the address of a Photon search by position.
#[wasm_bindgen]
pub fn photon_reverse_url(latitude: f64, longitude: f64, language: &str) -> Option<String> {
    let coordinates = btt_model::GeoCoordinates::from_degrees(latitude, longitude).ok()?;
    btt_geo::reverse_request_url(btt_geo::PUBLIC_PHOTON_BASE_URL, coordinates, language)
}

/// Reads the answer of a Photon name search and returns the places as JSON.
///
/// # Errors
///
/// Returns a message when the text is not a Photon answer.
#[wasm_bindgen]
pub fn photon_search_results(body: &str) -> Result<String, String> {
    let results = btt_geo::parse_search_answer(body).map_err(|error| error.to_string())?;
    serde_json::to_string(&results.into_vec()).map_err(|error| error.to_string())
}

/// Reads the answer of a Photon search by position and returns the name of the town.
///
/// # Errors
///
/// Returns a message when the text is not a Photon answer.
#[wasm_bindgen]
pub fn photon_reverse_name(body: &str) -> Result<Option<String>, String> {
    let place = btt_geo::parse_reverse_answer(body).map_err(|error| error.to_string())?;
    Ok(place.map(|place| place.name.to_string()))
}

/// Runs a command and returns its result as JSON. A command that fails rejects with the error as
/// JSON.
///
/// # Errors
///
/// Rejects with the JSON of a `CommandError`, or with a plain message when the arguments or the
/// command are unknown.
// The page awaits a promise, which an async function gives it. The commands themselves finish at
// once, because the planner works in tasks that the event loop runs.
#[allow(clippy::unused_async)]
#[wasm_bindgen]
pub async fn invoke(command: String, arguments_json: String) -> Result<String, String> {
    let backend = backend()?;
    let arguments = Arguments::parse(&arguments_json)?;
    match backend.run(&command, &arguments) {
        Ok(value) => Ok(value.to_string()),
        Err(Failure::Command(error)) => Err(serde_json::to_string(&error).unwrap_or_default()),
        Err(Failure::Other(message)) => Err(message),
    }
}

/// Returns the running backend.
fn backend() -> Result<Rc<WebBackend>, String> {
    BACKEND
        .with(|slot| slot.borrow().clone())
        .ok_or_else(|| "the backend has not started".to_owned())
}

/// Why a command failed.
pub(crate) enum Failure {
    /// An error the frontend shows to the traveller.
    Command(CommandError),
    /// A call the frontend should never make, such as an unknown command.
    Other(String),
}

impl From<CommandError> for Failure {
    fn from(error: CommandError) -> Self {
        Self::Command(error)
    }
}

/// The arguments of a command, by name.
pub(crate) struct Arguments(Map<String, Value>);

impl Arguments {
    /// Parses the JSON object that the frontend sends.
    fn parse(json: &str) -> Result<Self, String> {
        match serde_json::from_str::<Value>(json) {
            Ok(Value::Object(map)) => Ok(Self(map)),
            Ok(Value::Null) => Ok(Self(Map::new())),
            Ok(_) => Err("the arguments are not an object".to_owned()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Reads one argument.
    pub(crate) fn get<T: DeserializeOwned>(&self, name: &str) -> Result<T, Failure> {
        let value = self.0.get(name).cloned().unwrap_or(Value::Null);
        serde_json::from_value(value)
            .map_err(|error| Failure::Other(format!("the argument `{name}` is wrong: {error}")))
    }
}
