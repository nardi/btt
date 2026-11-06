//! The trips and travellers of the traveller, kept in memory and handed to the page as JSON.

use btt_model::{
    CURRENT_TRIP_SCHEMA_VERSION, StoredTrip, Traveller, TravellerId, Trip, TripId, TripSummary,
};
use serde::{Deserialize, Serialize};

/// A trip with the moment it last changed.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TripEntry {
    /// The trip and the schema version it was written with.
    stored: StoredTrip,
    /// When the trip last changed, in milliseconds since the Unix epoch.
    updated_at: u64,
}

/// What the page keeps between visits.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct WebStorage {
    /// Every trip.
    trips: Vec<TripEntry>,
    /// Every traveller profile.
    travellers: Vec<Traveller>,
    /// Whether the app checks for a newer timetable on startup. A missing value means yes.
    #[serde(default = "default_checks_enabled")]
    pub(crate) timetable_checks_enabled: bool,
}

/// The check for a newer timetable is on until the traveller turns it off.
fn default_checks_enabled() -> bool {
    true
}

impl WebStorage {
    /// Reads the state that `to_json` wrote.
    pub(crate) fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json)
            .map_err(|error| format!("the stored state is unreadable: {error}"))
    }

    /// Writes the whole state as JSON.
    pub(crate) fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Returns a summary of every trip, most recently changed first.
    pub(crate) fn list_trips(&self) -> Vec<TripSummary> {
        let mut entries: Vec<&TripEntry> = self.trips.iter().collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.updated_at));
        entries
            .into_iter()
            .map(|entry| {
                let trip = &entry.stored.trip;
                TripSummary {
                    trip_id: trip.id,
                    name: trip.name.clone(),
                    start_date: trip.start_date,
                    place_count: u16::try_from(trip.itinerary.trip_places().count())
                        .unwrap_or(u16::MAX),
                    route: trip.summary_route(),
                }
            })
            .collect()
    }

    /// Returns a trip.
    pub(crate) fn load_trip(&self, trip_id: TripId) -> Option<Trip> {
        self.trips
            .iter()
            .find(|entry| entry.stored.trip.id == trip_id)
            .map(|entry| entry.stored.trip.clone())
    }

    /// Inserts a trip, or replaces the one with the same ID.
    pub(crate) fn save_trip(&mut self, trip: &Trip, now: u64) {
        let stored = StoredTrip {
            schema_version: CURRENT_TRIP_SCHEMA_VERSION,
            trip: trip.clone(),
        };
        match self
            .trips
            .iter_mut()
            .find(|entry| entry.stored.trip.id == trip.id)
        {
            Some(entry) => {
                entry.stored = stored;
                entry.updated_at = now;
            }
            None => self.trips.push(TripEntry {
                stored,
                updated_at: now,
            }),
        }
    }

    /// Deletes a trip.
    pub(crate) fn delete_trip(&mut self, trip_id: TripId) {
        self.trips.retain(|entry| entry.stored.trip.id != trip_id);
    }

    /// Returns every traveller profile.
    pub(crate) fn travellers(&self) -> &[Traveller] {
        &self.travellers
    }

    /// Inserts a traveller profile, or replaces the one with the same ID.
    pub(crate) fn save_traveller(&mut self, traveller: &Traveller) {
        match self
            .travellers
            .iter_mut()
            .find(|existing| existing.id == traveller.id)
        {
            Some(existing) => *existing = traveller.clone(),
            None => self.travellers.push(traveller.clone()),
        }
    }

    /// Deletes a traveller profile.
    pub(crate) fn delete_traveller(&mut self, traveller_id: TravellerId) {
        self.travellers
            .retain(|traveller| traveller.id != traveller_id);
    }
}
