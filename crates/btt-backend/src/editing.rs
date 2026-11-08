//! Applying an edit that the traveller made to a trip.

use btt_model::{CommandError, Trip, TripEdit, TripEditNotices};
use smol_str::SmolStr;

use crate::place_naming::{has_provisional_name, provisional_place_name};

/// The trip after an edit, before anything saves it.
pub struct PreparedEdit {
    /// The trip with the edit applied.
    pub trip: Trip,
    /// What the edit changed besides what the traveller asked for.
    pub notices: TripEditNotices,
    /// Text for the undo and redo buttons.
    pub description: SmolStr,
}

/// Applies an edit to a copy of a trip.
///
/// Moving a place that still has its provisional name also updates that name, so it keeps
/// matching the coordinates and reverse geocoding can replace it.
///
/// # Errors
///
/// Returns an error when the edit does not fit the trip.
pub fn prepare_edit(trip: &Trip, edit: TripEdit) -> Result<PreparedEdit, CommandError> {
    let mut trip = trip.clone();
    let description = edit.describe(&trip);
    let provisional_rename = match &edit {
        TripEdit::MovePlace {
            place_id,
            coordinates,
        } if trip
            .itinerary
            .place(*place_id)
            .is_some_and(has_provisional_name) =>
        {
            Some(TripEdit::RenamePlace {
                place_id: *place_id,
                name: provisional_place_name(*coordinates),
            })
        }
        _ => None,
    };
    let mut notices = trip.apply_edit(edit).map_err(CommandError::InvalidEdit)?;
    if let Some(rename) = provisional_rename {
        notices.extend(trip.apply_edit(rename).map_err(CommandError::InvalidEdit)?);
    }
    Ok(PreparedEdit {
        trip,
        notices,
        description,
    })
}
