//! Reading stored trip documents, including documents from older schema versions.

use btt_model::{CURRENT_TRIP_SCHEMA_VERSION, StoredTrip, Trip};
use serde::Serialize;
use serde_json::Value;

use crate::StorageError;

/// Upgrades a trip document by one schema version, in place.
type TripDocumentMigration = fn(&mut Value);

/// Migrations in order. Entry `i` upgrades a document from version `i + 1` to `i + 2`.
///
/// A change to the `Trip` layout adds a function here, bumps `CURRENT_TRIP_SCHEMA_VERSION` and
/// adds a fixture for the old version.
const TRIP_DOCUMENT_MIGRATIONS: [TripDocumentMigration; 1] = [wrap_window_overrides];

/// Version 2 lets a place override only its arrival or departure window. A version 1 override
/// always held both, so it becomes the `Both` case.
fn wrap_window_overrides(document: &mut Value) {
    match document {
        Value::Object(fields) => {
            for (key, value) in fields.iter_mut() {
                if key == "window_override" && value.is_object() {
                    *value = serde_json::json!({ "Both": value.take() });
                } else {
                    wrap_window_overrides(value);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(wrap_window_overrides),
        _ => {}
    }
}

/// Borrowed form of `StoredTrip` with the same layout, so writing a document needs no clone.
#[derive(Serialize)]
struct StoredTripReference<'a> {
    /// Always `CURRENT_TRIP_SCHEMA_VERSION`.
    schema_version: u16,
    /// The trip to write.
    trip: &'a Trip,
}

/// Serializes a trip into a document of the current schema version.
///
/// # Errors
///
/// Returns an error when serialization fails, which a valid `Trip` never causes.
pub fn trip_to_document(trip: &Trip) -> Result<String, StorageError> {
    let stored_trip = StoredTripReference {
        schema_version: CURRENT_TRIP_SCHEMA_VERSION,
        trip,
    };
    Ok(serde_json::to_string(&stored_trip)?)
}

/// Parses a trip document of any known schema version and upgrades it to the current one.
///
/// # Errors
///
/// Returns an error when the document is not valid JSON, has no known schema version or does
/// not match the trip layout after migration.
pub fn trip_from_document(document: &str) -> Result<Trip, StorageError> {
    let mut value: Value = serde_json::from_str(document)?;
    let schema_version = value
        .get("schema_version")
        .and_then(Value::as_u64)
        .and_then(|version| u16::try_from(version).ok())
        .ok_or(StorageError::MissingSchemaVersion)?;
    if schema_version == 0 || schema_version > CURRENT_TRIP_SCHEMA_VERSION {
        return Err(StorageError::UnknownSchemaVersion(schema_version));
    }
    for migration in &TRIP_DOCUMENT_MIGRATIONS[usize::from(schema_version - 1)..] {
        migration(&mut value);
    }
    value["schema_version"] = Value::from(CURRENT_TRIP_SCHEMA_VERSION);
    let stored_trip: StoredTrip = serde_json::from_value(value)?;
    Ok(stored_trip.trip)
}

#[cfg(test)]
mod tests {
    use btt_model::PlaceWindowOverride;

    use super::*;

    /// A document written by schema version 1. It must keep loading in every later version.
    const TRIP_DOCUMENT_VERSION_1: &str = include_str!("../tests/fixtures/trip_v1.json");

    #[test]
    fn every_schema_version_fixture_loads() {
        assert_eq!(
            TRIP_DOCUMENT_MIGRATIONS.len() + 1,
            usize::from(CURRENT_TRIP_SCHEMA_VERSION),
            "each migration needs a fixture for the version it upgrades"
        );
        let trip = trip_from_document(TRIP_DOCUMENT_VERSION_1).unwrap();
        assert_eq!(trip.name, "Amsterdam to Rome");
        assert_eq!(trip.itinerary.place_count(), 3);
        let vienna = trip.itinerary.trip_places().nth(1).unwrap();
        assert!(matches!(
            vienna.window_override,
            Some(PlaceWindowOverride::Both(_))
        ));
    }

    #[test]
    fn documents_from_newer_versions_are_rejected() {
        let document = TRIP_DOCUMENT_VERSION_1.replacen(
            "\"schema_version\": 1",
            &format!("\"schema_version\": {}", CURRENT_TRIP_SCHEMA_VERSION + 1),
            1,
        );
        assert!(matches!(
            trip_from_document(&document),
            Err(StorageError::UnknownSchemaVersion(_))
        ));
    }

    #[test]
    fn written_documents_read_back() {
        let trip = trip_from_document(TRIP_DOCUMENT_VERSION_1).unwrap();
        let document = trip_to_document(&trip).unwrap();
        assert_eq!(trip_from_document(&document).unwrap(), trip);
    }
}
