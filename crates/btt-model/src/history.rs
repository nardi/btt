//! Undo and redo of trip edits, as snapshots of the whole trip.
//!
//! A snapshot restores everything an edit changed at once, such as a leg's travel mode and a
//! traveller's pick that go away with a removed place. A trip is a few kilobytes, so a history
//! of `MAXIMUM_HISTORY_STEPS` stays small.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use specta::Type;

use crate::trip::Trip;

/// Most steps the history keeps in each direction. The oldest step drops out first.
pub const MAXIMUM_HISTORY_STEPS: usize = 100;

/// One step of the history: a trip and what the step between it and its neighbour did.
#[derive(Debug, Clone, PartialEq)]
struct TripHistoryStep {
    /// The trip on the other side of the step.
    trip: Trip,
    /// What the edit of this step did, such as "Move Berlin".
    description: SmolStr,
}

/// The trips before the recent edits and after the undone ones.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TripHistory {
    /// Trips before each edit, oldest first, with the edit's description.
    undo_steps: VecDeque<TripHistoryStep>,
    /// Trips after each undone edit, the most recently undone last.
    redo_steps: Vec<TripHistoryStep>,
}

/// What the frontend shows about the history of an open trip.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TripHistoryState {
    /// What undo would take back, or `None` when there is nothing to undo.
    pub undo_description: Option<SmolStr>,
    /// What redo would apply again, or `None` when there is nothing to redo.
    pub redo_description: Option<SmolStr>,
}

impl TripHistory {
    /// Records an edit from `before` to `after`. An edit that changed nothing records no step.
    /// A new edit ends the redo history, because it branches off from the current trip.
    pub fn record(&mut self, before: Trip, after: &Trip, description: SmolStr) {
        if before == *after {
            return;
        }
        self.redo_steps.clear();
        if self.undo_steps.len() == MAXIMUM_HISTORY_STEPS {
            self.undo_steps.pop_front();
        }
        self.undo_steps.push_back(TripHistoryStep {
            trip: before,
            description,
        });
    }

    /// Takes back the most recent edit and returns the trip before it, or `None` when there is
    /// nothing to undo. `current` becomes the trip a redo returns to.
    pub fn undo(&mut self, current: Trip) -> Option<Trip> {
        let step = self.undo_steps.pop_back()?;
        self.redo_steps.push(TripHistoryStep {
            trip: current,
            description: step.description,
        });
        if self.redo_steps.len() > MAXIMUM_HISTORY_STEPS {
            self.redo_steps.remove(0);
        }
        Some(step.trip)
    }

    /// Applies the most recently undone edit again and returns the trip after it, or `None`
    /// when there is nothing to redo.
    pub fn redo(&mut self, current: Trip) -> Option<Trip> {
        let step = self.redo_steps.pop()?;
        if self.undo_steps.len() == MAXIMUM_HISTORY_STEPS {
            self.undo_steps.pop_front();
        }
        self.undo_steps.push_back(TripHistoryStep {
            trip: current,
            description: step.description,
        });
        Some(step.trip)
    }

    /// Returns what undo and redo would do.
    pub fn state(&self) -> TripHistoryState {
        TripHistoryState {
            undo_description: self.undo_steps.back().map(|step| step.description.clone()),
            redo_description: self.redo_steps.last().map(|step| step.description.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    /// Returns one trip under different names. The trips share their ID, because `Trip::new`
    /// makes a new one each time.
    fn trip_named(name: &str) -> Trip {
        static BASE: std::sync::OnceLock<Trip> = std::sync::OnceLock::new();
        let mut trip = BASE
            .get_or_init(|| Trip::new("Trip".into(), NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()))
            .clone();
        trip.name = name.into();
        trip
    }

    #[test]
    fn undo_and_redo_walk_the_edits() {
        let mut history = TripHistory::default();
        let (first, second, third) = (trip_named("A"), trip_named("B"), trip_named("C"));
        history.record(first.clone(), &second, "Rename to B".into());
        history.record(second.clone(), &third, "Rename to C".into());
        assert_eq!(
            history.state().undo_description.as_deref(),
            Some("Rename to C")
        );

        assert_eq!(history.undo(third.clone()), Some(second.clone()));
        assert_eq!(history.undo(second.clone()), Some(first.clone()));
        assert_eq!(history.undo(first.clone()), None);
        assert_eq!(
            history.state().redo_description.as_deref(),
            Some("Rename to B")
        );

        assert_eq!(history.redo(first), Some(second.clone()));
        assert_eq!(history.redo(second), Some(third));
        assert_eq!(history.state().redo_description, None);
    }

    #[test]
    fn a_new_edit_ends_the_redo_history() {
        let mut history = TripHistory::default();
        let (first, second) = (trip_named("A"), trip_named("B"));
        history.record(first.clone(), &second, "Rename to B".into());
        assert_eq!(history.undo(second), Some(first.clone()));
        history.record(first, &trip_named("D"), "Rename to D".into());
        assert_eq!(history.state().redo_description, None);
    }

    #[test]
    fn an_edit_that_changes_nothing_records_no_step() {
        let mut history = TripHistory::default();
        history.record(trip_named("A"), &trip_named("A"), "Rename to A".into());
        assert_eq!(history.state(), TripHistoryState::default());
    }

    #[test]
    fn the_oldest_step_drops_out() {
        let mut history = TripHistory::default();
        for step in 0..=MAXIMUM_HISTORY_STEPS {
            let before = trip_named(&step.to_string());
            let after = trip_named(&(step + 1).to_string());
            history.record(before, &after, SmolStr::new(format!("Step {step}")));
        }
        let mut undone = 0;
        let mut current = trip_named(&(MAXIMUM_HISTORY_STEPS + 1).to_string());
        while let Some(previous) = history.undo(current.clone()) {
            current = previous;
            undone += 1;
        }
        assert_eq!(undone, MAXIMUM_HISTORY_STEPS);
        assert_eq!(current.name, "1", "the step from 0 to 1 dropped out");
    }
}
