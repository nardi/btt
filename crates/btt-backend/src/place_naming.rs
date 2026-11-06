//! The names that places get before reverse geocoding finds their town.

use btt_model::{GeoCoordinates, TripPlace};
use smol_str::{SmolStr, format_smolstr};

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
