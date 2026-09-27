//! Names for places added on the map.
//!
//! A place added by a map click first gets a name made from its coordinates. Reverse geocoding
//! then looks up the town in the background and replaces the name, unless the user renamed the
//! place or moved it in the meantime.

use btt_model::{GeoCoordinates, Trip, TripEdit, TripId, TripPlace, TripPlaceId};
use smol_str::{SmolStr, format_smolstr};
use tauri::{AppHandle, Manager};

use crate::state::AppState;

/// Returns the name a place gets until reverse geocoding finds a better one, such as
/// "48.2085° N, 16.3725° E".
pub fn provisional_place_name(coordinates: GeoCoordinates) -> SmolStr {
    let latitude = coordinates.latitude.degrees();
    let longitude = coordinates.longitude.degrees();
    let north_or_south = if latitude < 0.0 { 'S' } else { 'N' };
    let east_or_west = if longitude < 0.0 { 'W' } else { 'E' };
    format_smolstr!(
        "{:.4}° {north_or_south}, {:.4}° {east_or_west}",
        latitude.abs(),
        longitude.abs()
    )
}

/// Returns whether a place still has the name made from its coordinates.
pub fn has_provisional_name(place: &TripPlace) -> bool {
    place.name == provisional_place_name(place.coordinates)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provisional_names_show_hemispheres() {
        let vienna = GeoCoordinates::from_degrees(48.208_49, 16.372_52).unwrap();
        assert_eq!(provisional_place_name(vienna), "48.2085° N, 16.3725° E");
        let lisbon = GeoCoordinates::from_degrees(38.722_2, -9.139_3).unwrap();
        assert_eq!(provisional_place_name(lisbon), "38.7222° N, 9.1393° W");
    }
}
