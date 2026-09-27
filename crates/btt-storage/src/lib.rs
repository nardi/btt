//! SQLite storage for trips, settings and caches.
//!
//! Trips are stored as JSON documents, because the `.btt` export uses the same document and
//! JSON migrations can edit old documents without old type definitions. Caches use `postcard`,
//! because only BTT reads them and a format change only needs a cache flush.

mod trip_document;

use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use btt_model::{PlaceSearchResult, Trip, TripId, TripSummary};
use chrono::NaiveDate;
use rusqlite::{Connection, OptionalExtension, params};
use rusqlite_migration::{M, Migrations};
use thiserror::Error;
use uuid::Uuid;

pub use trip_document::{trip_from_document, trip_to_document};

/// Seconds a cached place search stays valid. Place names and positions rarely change.
const PLACE_SEARCH_CACHE_LIFETIME_SECONDS: i64 = 30 * 24 * 60 * 60;

/// Schema migrations in order. `rusqlite_migration` records the applied count in SQLite's
/// `user_version`.
const SCHEMA_MIGRATIONS: &[M<'static>] = &[M::up(
    "CREATE TABLE trips (
        trip_id BLOB PRIMARY KEY NOT NULL,
        name TEXT NOT NULL,
        start_date TEXT NOT NULL,
        place_count INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        document TEXT NOT NULL
    ) STRICT;
    CREATE TABLE settings (
        setting_key TEXT PRIMARY KEY NOT NULL,
        setting_value TEXT NOT NULL
    ) STRICT;
    CREATE TABLE geocode_cache (
        request_key TEXT PRIMARY KEY NOT NULL,
        fetched_at INTEGER NOT NULL,
        search_results BLOB NOT NULL
    ) STRICT;",
)];

/// Errors from reading or writing storage.
#[derive(Debug, Error)]
pub enum StorageError {
    /// SQLite reported an error.
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    /// The schema could not be brought to the current version.
    #[error("database migration failed: {0}")]
    Migration(#[from] rusqlite_migration::Error),
    /// A JSON document could not be read or written.
    #[error("invalid trip document: {0}")]
    TripDocument(#[from] serde_json::Error),
    /// A trip document has no schema version.
    #[error("trip document has no schema version")]
    MissingSchemaVersion,
    /// A trip document comes from a newer BTT version or is corrupt.
    #[error("trip document has unknown schema version {0}")]
    UnknownSchemaVersion(u16),
    /// A cache entry could not be read or written.
    #[error("invalid cache entry: {0}")]
    CacheEntry(#[from] postcard::Error),
}

/// The app's SQLite database. One connection behind a mutex, because the app has one writer.
pub struct Storage {
    /// The open connection.
    connection: Mutex<Connection>,
}

impl Storage {
    /// Opens or creates the database file and brings its schema up to date.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be opened or a migration fails.
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        let connection = Connection::open(path)?;
        // WAL mode makes the small write after every edit cheap.
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        Self::from_connection(connection)
    }

    /// Creates a database in memory, for tests.
    ///
    /// # Errors
    ///
    /// Returns an error when a migration fails.
    pub fn open_in_memory() -> Result<Self, StorageError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    /// Migrates a connection and wraps it.
    fn from_connection(mut connection: Connection) -> Result<Self, StorageError> {
        Migrations::from_slice(SCHEMA_MIGRATIONS).to_latest(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Locks the connection. A panic in another holder cannot leave SQLite in a half-written
    /// state, so a poisoned lock is used as is.
    fn connection(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Returns a summary of every trip, most recently changed first.
    ///
    /// # Errors
    ///
    /// Returns an error when the query fails.
    pub fn list_trips(&self) -> Result<Vec<TripSummary>, StorageError> {
        let connection = self.connection();
        let mut statement = connection.prepare_cached(
            "SELECT trip_id, name, start_date, place_count FROM trips ORDER BY updated_at DESC",
        )?;
        let summaries = statement
            .query_map([], |row| {
                Ok(TripSummary {
                    trip_id: TripId(Uuid::from_bytes(row.get(0)?)),
                    name: row.get::<_, String>(1)?.into(),
                    start_date: row.get::<_, NaiveDate>(2)?,
                    place_count: row.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(summaries)
    }

    /// Loads a trip, upgrading its document if it comes from an older schema version.
    ///
    /// # Errors
    ///
    /// Returns an error when the query fails or the document cannot be read.
    pub fn load_trip(&self, trip_id: TripId) -> Result<Option<Trip>, StorageError> {
        let document: Option<String> = self
            .connection()
            .prepare_cached("SELECT document FROM trips WHERE trip_id = ?1")?
            .query_row([trip_id.0.as_bytes()], |row| row.get(0))
            .optional()?;
        document.as_deref().map(trip_from_document).transpose()
    }

    /// Inserts or replaces a trip.
    ///
    /// # Errors
    ///
    /// Returns an error when the document cannot be written or the query fails.
    pub fn save_trip(&self, trip: &Trip) -> Result<(), StorageError> {
        let document = trip_to_document(trip)?;
        // The place limit in `btt_model::edit` keeps the count far below `i64::MAX`.
        let place_count = i64::try_from(trip.itinerary.place_count()).unwrap_or(i64::MAX);
        self.connection()
            .prepare_cached(
                "INSERT OR REPLACE INTO trips
                    (trip_id, name, start_date, place_count, updated_at, document)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?
            .execute(params![
                trip.id.0.as_bytes(),
                trip.name.as_str(),
                trip.start_date,
                place_count,
                unix_seconds_now(),
                document,
            ])?;
        Ok(())
    }

    /// Deletes a trip. Returns whether it existed.
    ///
    /// # Errors
    ///
    /// Returns an error when the query fails.
    pub fn delete_trip(&self, trip_id: TripId) -> Result<bool, StorageError> {
        let deleted_rows = self
            .connection()
            .prepare_cached("DELETE FROM trips WHERE trip_id = ?1")?
            .execute([trip_id.0.as_bytes()])?;
        Ok(deleted_rows > 0)
    }

    /// Returns the cached results of a place search, if they exist and have not expired.
    ///
    /// # Errors
    ///
    /// Returns an error when the query fails or the entry cannot be decoded.
    pub fn cached_place_search(
        &self,
        request_key: &str,
    ) -> Result<Option<Vec<PlaceSearchResult>>, StorageError> {
        let encoded: Option<Vec<u8>> = self
            .connection()
            .prepare_cached(
                "SELECT search_results FROM geocode_cache
                 WHERE request_key = ?1 AND fetched_at > ?2",
            )?
            .query_row(
                params![
                    request_key,
                    unix_seconds_now() - PLACE_SEARCH_CACHE_LIFETIME_SECONDS
                ],
                |row| row.get(0),
            )
            .optional()?;
        Ok(encoded.as_deref().map(postcard::from_bytes).transpose()?)
    }

    /// Stores the results of a place search.
    ///
    /// # Errors
    ///
    /// Returns an error when the entry cannot be encoded or the query fails.
    pub fn store_place_search(
        &self,
        request_key: &str,
        search_results: &[PlaceSearchResult],
    ) -> Result<(), StorageError> {
        let encoded = postcard::to_stdvec(search_results)?;
        self.connection()
            .prepare_cached(
                "INSERT OR REPLACE INTO geocode_cache (request_key, fetched_at, search_results)
                 VALUES (?1, ?2, ?3)",
            )?
            .execute(params![request_key, unix_seconds_now(), encoded])?;
        Ok(())
    }
}

/// Returns the current time in Unix seconds.
fn unix_seconds_now() -> i64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use btt_model::GeoCoordinates;

    use super::*;

    fn example_trip() -> Trip {
        trip_from_document(include_str!("../tests/fixtures/trip_v1.json")).unwrap()
    }

    #[test]
    fn saved_trips_load_and_list() {
        let storage = Storage::open_in_memory().unwrap();
        let trip = example_trip();
        storage.save_trip(&trip).unwrap();

        assert_eq!(storage.load_trip(trip.id).unwrap(), Some(trip.clone()));
        let summaries = storage.list_trips().unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].trip_id, trip.id);
        assert_eq!(summaries[0].place_count, 3);
        assert_eq!(summaries[0].start_date, trip.start_date);
    }

    #[test]
    fn saving_again_replaces_the_trip() {
        let storage = Storage::open_in_memory().unwrap();
        let mut trip = example_trip();
        storage.save_trip(&trip).unwrap();
        trip.name = "Renamed".into();
        storage.save_trip(&trip).unwrap();
        assert_eq!(storage.list_trips().unwrap().len(), 1);
        assert_eq!(storage.load_trip(trip.id).unwrap().unwrap().name, "Renamed");
    }

    #[test]
    fn deleted_trips_are_gone() {
        let storage = Storage::open_in_memory().unwrap();
        let trip = example_trip();
        storage.save_trip(&trip).unwrap();
        assert!(storage.delete_trip(trip.id).unwrap());
        assert!(!storage.delete_trip(trip.id).unwrap());
        assert_eq!(storage.load_trip(trip.id).unwrap(), None);
    }

    #[test]
    fn place_searches_round_trip_through_the_cache() {
        let storage = Storage::open_in_memory().unwrap();
        let results = vec![PlaceSearchResult {
            name: "Wien".into(),
            context: "Österreich".into(),
            coordinates: GeoCoordinates::from_degrees(48.208_4, 16.372_5).unwrap(),
        }];
        assert_eq!(storage.cached_place_search("en:wien").unwrap(), None);
        storage.store_place_search("en:wien", &results).unwrap();
        assert_eq!(
            storage.cached_place_search("en:wien").unwrap(),
            Some(results)
        );
    }

    #[test]
    fn data_survives_reopening_the_file() {
        let directory = std::env::temp_dir().join(format!("btt-storage-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("btt.sqlite3");
        let trip = example_trip();
        Storage::open(&path).unwrap().save_trip(&trip).unwrap();
        assert_eq!(
            Storage::open(&path).unwrap().load_trip(trip.id).unwrap(),
            Some(trip)
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
