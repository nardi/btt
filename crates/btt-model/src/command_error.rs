//! The error that every command returns to the frontend.

use chrono::NaiveDate;
use serde::Serialize;
use specta::Type;
use thiserror::Error;

use crate::edit::TripEditError;
use crate::trip::TripId;

/// Errors a command returns to the frontend.
#[derive(Debug, Error, Serialize, Type)]
pub enum CommandError {
    /// No trip with this ID exists.
    #[error("trip {0:?} does not exist")]
    TripNotFound(TripId),
    /// The frontend edited a trip it has not opened.
    #[error("trip {0:?} is not open")]
    TripNotOpen(TripId),
    /// The edit does not fit the trip.
    #[error("invalid edit: {0}")]
    InvalidEdit(TripEditError),
    /// Reading or writing the database failed.
    #[error("storage failed: {message}")]
    Storage {
        /// Description of the failure.
        message: String,
    },
    /// The place search service failed or could not be reached.
    #[error("place search failed: {message}")]
    PlaceSearch {
        /// Description of the failure.
        message: String,
    },
    /// The trip has more journeys than a link can carry.
    #[error("the trip is too big for a link")]
    TripTooLargeForLink,
    /// Writing the link or the trip file failed.
    #[error("sharing failed: {message}")]
    Sharing {
        /// Description of the failure.
        message: String,
    },
    /// The start date of a trip lies outside the days the timetable covers.
    #[error("the timetable covers {first} to {last}")]
    StartDateOutsideTimetable {
        /// First day the timetable covers.
        first: NaiveDate,
        /// Last day the timetable covers.
        last: NaiveDate,
    },
    /// Downloading or installing a timetable failed.
    #[error("timetable download failed: {message}")]
    Timetable {
        /// Description of the failure.
        message: String,
    },
    /// Saving the data location failed, or the new folder is not writable.
    #[error("changing the data location failed: {message}")]
    DataLocation {
        /// Description of the failure.
        message: String,
    },
}
