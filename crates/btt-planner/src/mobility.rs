//! Which trains and stations a traveller with a bike or in a wheelchair can use.
//!
//! The verdict for a train combines the value of the feed with a committed rule of its
//! operator, which the pricer reads from the fare data. A `No` from either source excludes the
//! train. A `Yes` from the feed or from a checked rule confirms it. Anything else allows the
//! train but leaves it unconfirmed, so that the plan can mark the ride.

use btt_model::MobilityMode;
use btt_router::FootpathMode;
use btt_timetable::{Availability, BundleStationIndex, BundleTripIndex, LoadedTimetableBundle};
use fixedbitset::FixedBitSet;
use rustc_hash::FxHashMap;

use crate::operators::OperatorTable;
use crate::pricing::JourneyPricer;

/// A way of travelling that limits which trains and stations the traveller can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RestrictedMode {
    /// With an assembled bike.
    Bike,
    /// In a wheelchair.
    Wheelchair,
}

/// Extra minutes on top of the minimum transfer time with a bike, to take it off one train and
/// onto the next.
const BIKE_TRANSFER_EXTRA_MINUTES: u16 = 5;

/// Extra minutes on top of the minimum transfer time in a wheelchair, for lifts and the ramp
/// of the staff.
const WHEELCHAIR_TRANSFER_EXTRA_MINUTES: u16 = 10;

/// Returns how the traveller moves between stations in a mobility mode. A folded bike is
/// unfolded for the ways, so it moves like a bike.
pub(crate) const fn footpath_mode(mode: MobilityMode) -> FootpathMode {
    match mode {
        MobilityMode::Foot => FootpathMode::Walk,
        MobilityMode::Bike | MobilityMode::FoldingBike => FootpathMode::Cycle,
        MobilityMode::Wheelchair => FootpathMode::Wheel,
    }
}

/// Returns the mode that limits the trains and stations, or `None` when the traveller can use
/// all of them. A folded bike travels as luggage, so it limits nothing.
pub(crate) const fn restricted_mode(mode: MobilityMode) -> Option<RestrictedMode> {
    match mode {
        MobilityMode::Foot | MobilityMode::FoldingBike => None,
        MobilityMode::Bike => Some(RestrictedMode::Bike),
        MobilityMode::Wheelchair => Some(RestrictedMode::Wheelchair),
    }
}

/// Returns the minutes that a mobility mode adds to the minimum transfer time.
pub(crate) const fn transfer_extra_minutes(mode: MobilityMode) -> u16 {
    match mode {
        MobilityMode::Foot => 0,
        MobilityMode::Bike | MobilityMode::FoldingBike => BIKE_TRANSFER_EXTRA_MINUTES,
        MobilityMode::Wheelchair => WHEELCHAIR_TRANSFER_EXTRA_MINUTES,
    }
}

/// What the committed rules of an operator say about a train.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RuleVerdict {
    /// No rule covers the train.
    #[default]
    NoRule,
    /// A rule allows the train.
    Allows {
        /// Whether the rule is checked against the page of the operator.
        verified: bool,
    },
    /// A rule forbids the train.
    Forbids,
}

/// How sure the planner is that a traveller can use a train or a station.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModeVerdict {
    /// The feed or a checked rule confirms it.
    Confirmed,
    /// Nothing excludes it, but nothing confirms it either.
    Unconfirmed,
    /// The feed or a rule excludes it.
    Excluded,
}

/// Combines a feed value with a rule verdict.
fn combined_verdict(feed: Availability, rule: RuleVerdict) -> ModeVerdict {
    match (feed, rule) {
        (Availability::No, _) | (_, RuleVerdict::Forbids) => ModeVerdict::Excluded,
        (Availability::Yes, _) | (_, RuleVerdict::Allows { verified: true }) => {
            ModeVerdict::Confirmed
        }
        _ => ModeVerdict::Unconfirmed,
    }
}

/// The verdicts of every trip and station of a bundle for one restricted mode.
#[derive(Debug)]
pub(crate) struct ModeVerdicts {
    /// Trips the traveller cannot use.
    excluded_trips: FixedBitSet,
    /// Trips that the feed or a checked rule confirms.
    confirmed_trips: FixedBitSet,
    /// Stations the traveller cannot use. Empty for a bike, which can use every station.
    excluded_stations: FixedBitSet,
    /// Stations that the feed confirms. Every station counts as confirmed for a bike.
    confirmed_stations: FixedBitSet,
    /// Whether the mode looks at stations at all.
    checks_stations: bool,
}

impl ModeVerdicts {
    /// Computes the verdicts for a mode. The rule of a train label is looked up once per
    /// agency and category, because many labels share them.
    pub(crate) fn new(
        bundle: &LoadedTimetableBundle,
        operators: &OperatorTable,
        pricer: &impl JourneyPricer,
        mode: RestrictedMode,
    ) -> Self {
        let mut rule_by_train: FxHashMap<(&str, &str), RuleVerdict> = FxHashMap::default();
        let label_verdicts: Vec<ModeVerdict> = bundle
            .train_labels()
            .zip(bundle.train_label_availability())
            .map(
                |((category, _, agency), (bike_carriage, wheelchair_access))| {
                    let rule = *rule_by_train.entry((agency, category)).or_insert_with(|| {
                        pricer.carriage_rule(
                            mode,
                            &operators.operator_of(agency, category),
                            category,
                        )
                    });
                    let feed = match mode {
                        RestrictedMode::Bike => bike_carriage,
                        RestrictedMode::Wheelchair => wheelchair_access,
                    };
                    combined_verdict(feed, rule)
                },
            )
            .collect();
        let trip_count = bundle.trip_count();
        let mut excluded_trips = FixedBitSet::with_capacity(trip_count);
        let mut confirmed_trips = FixedBitSet::with_capacity(trip_count);
        for trip_index in 0..trip_count {
            let trip = BundleTripIndex(u32::try_from(trip_index).unwrap_or(u32::MAX));
            match label_verdicts[bundle.trip_train_label_index(trip)] {
                ModeVerdict::Excluded => excluded_trips.insert(trip_index),
                ModeVerdict::Confirmed => confirmed_trips.insert(trip_index),
                ModeVerdict::Unconfirmed => {}
            }
        }
        let station_count = bundle.station_count();
        let mut excluded_stations = FixedBitSet::with_capacity(station_count);
        let mut confirmed_stations = FixedBitSet::with_capacity(station_count);
        let checks_stations = mode == RestrictedMode::Wheelchair;
        if checks_stations {
            for station_index in 0..station_count {
                let station = BundleStationIndex(u32::try_from(station_index).unwrap_or(u32::MAX));
                match bundle.station_wheelchair_boarding(station) {
                    Availability::No => excluded_stations.insert(station_index),
                    Availability::Yes => confirmed_stations.insert(station_index),
                    Availability::Unknown => {}
                }
            }
        }
        Self {
            excluded_trips,
            confirmed_trips,
            excluded_stations,
            confirmed_stations,
            checks_stations,
        }
    }

    /// Returns whether the traveller can board or leave a trip at a station.
    pub(crate) fn allows(&self, trip: BundleTripIndex, station: BundleStationIndex) -> bool {
        !self.excluded_trips.contains(trip.as_usize())
            && !self.excluded_stations.contains(station.as_usize())
    }

    /// Returns whether the traveller can use a station.
    pub(crate) fn allows_station(&self, station: BundleStationIndex) -> bool {
        !self.excluded_stations.contains(station.as_usize())
    }

    /// Returns the verdict of a trip.
    pub(crate) fn trip_verdict(&self, trip: BundleTripIndex) -> ModeVerdict {
        if self.excluded_trips.contains(trip.as_usize()) {
            ModeVerdict::Excluded
        } else if self.confirmed_trips.contains(trip.as_usize()) {
            ModeVerdict::Confirmed
        } else {
            ModeVerdict::Unconfirmed
        }
    }

    /// Returns the verdict of a station.
    pub(crate) fn station_verdict(&self, station: BundleStationIndex) -> ModeVerdict {
        if !self.checks_stations || self.confirmed_stations.contains(station.as_usize()) {
            ModeVerdict::Confirmed
        } else if self.excluded_stations.contains(station.as_usize()) {
            ModeVerdict::Excluded
        } else {
            ModeVerdict::Unconfirmed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_no_from_either_source_excludes_the_train() {
        assert_eq!(
            combined_verdict(Availability::No, RuleVerdict::Allows { verified: true }),
            ModeVerdict::Excluded
        );
        assert_eq!(
            combined_verdict(Availability::Yes, RuleVerdict::Forbids),
            ModeVerdict::Excluded
        );
    }

    #[test]
    fn an_unchecked_rule_does_not_confirm_the_train() {
        assert_eq!(
            combined_verdict(
                Availability::Unknown,
                RuleVerdict::Allows { verified: false }
            ),
            ModeVerdict::Unconfirmed
        );
        assert_eq!(
            combined_verdict(
                Availability::Unknown,
                RuleVerdict::Allows { verified: true }
            ),
            ModeVerdict::Confirmed
        );
        assert_eq!(
            combined_verdict(Availability::Yes, RuleVerdict::NoRule),
            ModeVerdict::Confirmed
        );
    }
}
