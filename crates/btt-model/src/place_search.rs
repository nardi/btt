//! Results of searching for places by name or by position.

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use specta::Type;

use crate::coordinates::GeoCoordinates;

/// A place found by a geocoding search.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct PlaceSearchResult {
    /// Name of the place, such as "Vienna" or "Wien Hauptbahnhof".
    pub name: SmolStr,
    /// Context that tells places with the same name apart, such as "Lower Austria, Austria".
    pub context: SmolStr,
    /// Location of the place.
    pub coordinates: GeoCoordinates,
}
