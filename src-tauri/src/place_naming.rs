//! Names for places added on the map.
//!
//! A place added by a map click first gets a name made from its coordinates. Reverse geocoding
//! then looks up the town in the background and replaces the name, unless the user renamed the
//! place or moved it in the meantime.

use btt_backend::place_naming::has_provisional_name;
use btt_model::{GeoCoordinates, Trip, TripEdit, TripId, TripPlaceId};
use smol_str::SmolStr;
use tauri::{AppHandle, Manager};

use crate::state::AppState;

/// Starts reverse geocoding for every place of a trip that still has a provisional name.
pub fn name_provisional_places(app: &AppHandle, trip: &Trip) {
    for place in trip
        .itinerary
        .trip_places()
        .filter(|place| has_provisional_name(place))
    {
        let app = app.clone();
        let (trip_id, place_id, coordinates) = (trip.id, place.id, place.coordinates);
        tauri::async_runtime::spawn(async move {
            name_place(&app, trip_id, place_id, coordinates).await;
        });
    }
}

/// Looks up the town at a place and renames the place if it still has its provisional name and
/// has not moved.
async fn name_place(
    app: &AppHandle,
    trip_id: TripId,
    place_id: TripPlaceId,
    coordinates: GeoCoordinates,
) {
    let state = app.state::<AppState>();
    let Some(name) = reverse_geocoded_name(&state, coordinates).await else {
        return;
    };
    let rename = TripEdit::RenamePlace { place_id, name };
    let still_provisional = |trip: &Trip| {
        trip.itinerary
            .place(place_id)
            .is_some_and(|place| place.coordinates == coordinates && has_provisional_name(place))
    };
    if let Err(error) = state.apply_edit_from_backend(trip_id, rename, still_provisional) {
        tracing::warn!(%error, "could not rename a place after reverse geocoding");
    }
}

/// Returns the name of the town at a position, from the cache or from Photon.
async fn reverse_geocoded_name(state: &AppState, coordinates: GeoCoordinates) -> Option<SmolStr> {
    // Three decimals are about 100 m, so small drags reuse the cached answer.
    let request_key = format!(
        "reverse:{}:{:.3}:{:.3}",
        state.language,
        coordinates.latitude.degrees(),
        coordinates.longitude.degrees()
    );
    match state.storage.cached_place_search(&request_key) {
        Ok(Some(cached)) => return cached.into_iter().next().map(|result| result.name),
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "could not read the geocoding cache"),
    }
    let result = match state.photon.reverse(coordinates, &state.language).await {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!(%error, "reverse geocoding failed");
            return None;
        }
    };
    let results: Vec<_> = result.into_iter().collect();
    if let Err(error) = state.storage.store_place_search(&request_key, &results) {
        tracing::warn!(%error, "could not write the geocoding cache");
    }
    results.into_iter().next().map(|result| result.name)
}
