//! Steps 6 and 7: finds copies of the same train in different feeds and merges them.
//!
//! The plan put detection in SQL. After UTC resolution the trips already sit in Rust memory,
//! and trips with the same train number form small groups, so both steps run here without a
//! round trip through the database.

use btt_timetable::{ServiceDayBitset, TripStopInput};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::resolve::ResolvedTrip;

/// Minutes in one day.
const MINUTES_PER_DAY: i32 = 1440;

/// Largest difference between the times of two copies at the same station.
const MATCHING_TIME_TOLERANCE_MINUTES: i32 = 2;

/// Stations two trips must share at matching times to count as the same train.
const MINIMUM_SHARED_STATIONS: usize = 2;

/// How many merges happened between each pair of feeds, keyed by their positions in the
/// configuration with the smaller one first.
pub type MergeCounts = FxHashMap<(usize, usize), usize>;

/// Two trips that are the same train on some days.
struct DuplicateMatch {
    /// Days the second trip's service date lies after the first trip's.
    date_offset_days: i32,
    /// Days, counted on the first trip's service dates, on which both run.
    shared_days: ServiceDayBitset,
}

/// Merges copies of the same train and returns the remaining trips with merge counts.
pub fn merge_duplicate_trips(trips: Vec<ResolvedTrip>) -> (Vec<ResolvedTrip>, MergeCounts) {
    let mut groups: FxHashMap<String, Vec<ResolvedTrip>> = FxHashMap::default();
    let mut result = Vec::with_capacity(trips.len());
    for trip in trips {
        if trip.train_number.is_empty() {
            result.push(trip);
        } else {
            groups
                .entry(trip.train_number.clone())
                .or_default()
                .push(trip);
        }
    }
    let mut merge_counts = MergeCounts::default();
    let mut group_keys: Vec<_> = groups.keys().cloned().collect();
    // A fixed order keeps the bundle the same for the same input.
    group_keys.sort_unstable();
    for key in group_keys {
        let Some(group) = groups.remove(&key) else {
            continue;
        };
        result.extend(merge_group(group, &mut merge_counts));
    }
    (result, merge_counts)
}

/// Merges duplicates within one group of trips that share a train number.
fn merge_group(mut group: Vec<ResolvedTrip>, merge_counts: &mut MergeCounts) -> Vec<ResolvedTrip> {
    // Each merge removes the shared days from both copies, so the loop ends. The bound only
    // guards against a mistake in that reasoning.
    let merge_limit = group.len() * group.len() * 4 + 4;
    for _ in 0..merge_limit {
        let Some((first, second, found)) = find_duplicate_pair(&group) else {
            break;
        };
        let (primary, secondary, date_offset_days, shared_days) =
            if group[first].trip.stops.len() >= group[second].trip.stops.len() {
                (first, second, found.date_offset_days, found.shared_days)
            } else {
                let shared_days = shift_days(found.shared_days, found.date_offset_days);
                (second, first, -found.date_offset_days, shared_days)
            };
        let feed_pair = ordered_pair(group[primary].feed_index, group[secondary].feed_index);
        *merge_counts.entry(feed_pair).or_default() += 1;
        let parts = merge_pair(
            &group[primary],
            &group[secondary],
            date_offset_days,
            shared_days,
        );
        // Remove the higher index first so the lower one stays valid.
        let (higher, lower) = if primary > secondary {
            (primary, secondary)
        } else {
            (secondary, primary)
        };
        group.swap_remove(higher);
        group.swap_remove(lower);
        group.extend(parts);
    }
    group
}

/// Finds the first pair of trips in a group that are the same train on some day.
fn find_duplicate_pair(group: &[ResolvedTrip]) -> Option<(usize, usize, DuplicateMatch)> {
    for first in 0..group.len() {
        for second in first + 1..group.len() {
            if let Some(found) = duplicate_match(&group[first], &group[second]) {
                return Some((first, second, found));
            }
        }
    }
    None
}

/// Checks whether two trips are the same train on at least one day. The second trip's service
/// date may lie a day before or after the first's, when one feed starts the train before
/// midnight in UTC and the other after.
fn duplicate_match(first: &ResolvedTrip, second: &ResolvedTrip) -> Option<DuplicateMatch> {
    for date_offset_days in -1..=1 {
        let shared_days = intersect(
            first.trip.service_days,
            shift_days(second.trip.service_days, -date_offset_days),
        );
        if shared_days.is_empty() {
            continue;
        }
        let offset_minutes = date_offset_days * MINUTES_PER_DAY;
        let shared_stations = first
            .trip
            .stops
            .iter()
            .filter(|first_stop| {
                second.trip.stops.iter().any(|second_stop| {
                    second_stop.station == first_stop.station
                        && (i32::from(second_stop.departure) + offset_minutes
                            - i32::from(first_stop.departure))
                        .abs()
                            <= MATCHING_TIME_TOLERANCE_MINUTES
                })
            })
            .count();
        if shared_stations >= MINIMUM_SHARED_STATIONS {
            return Some(DuplicateMatch {
                date_offset_days,
                shared_days,
            });
        }
    }
    None
}

/// Merges two copies of a train on their shared days and keeps each copy on its other days.
///
/// The merged trip is the primary copy, extended by the secondary copy's stops before the
/// primary's first station and after its last. Stops in the middle are not merged, because a
/// feed that skips one usually has a reason, such as a stop without domestic boarding.
fn merge_pair(
    primary: &ResolvedTrip,
    secondary: &ResolvedTrip,
    date_offset_days: i32,
    shared_days: ServiceDayBitset,
) -> Vec<ResolvedTrip> {
    let mut parts = Vec::with_capacity(3);
    let primary_rest = subtract(primary.trip.service_days, shared_days);
    let secondary_rest = subtract(
        secondary.trip.service_days,
        shift_days(shared_days, date_offset_days),
    );
    if let Some(merged) = extended_trip(primary, secondary, date_offset_days, shared_days) {
        parts.push(merged);
    }
    if !primary_rest.is_empty() {
        let mut rest = primary.clone();
        rest.trip.service_days = primary_rest;
        parts.push(rest);
    }
    if !secondary_rest.is_empty() {
        let mut rest = secondary.clone();
        rest.trip.service_days = secondary_rest;
        parts.push(rest);
    }
    parts
}

/// Returns the primary copy with the secondary's extra stops at both ends, running on the shared
/// days. Falls back to the plain primary copy when the extension would go back in time.
fn extended_trip(
    primary: &ResolvedTrip,
    secondary: &ResolvedTrip,
    date_offset_days: i32,
    shared_days: ServiceDayBitset,
) -> Option<ResolvedTrip> {
    let primary_stations: FxHashSet<_> =
        primary.trip.stops.iter().map(|stop| stop.station).collect();
    let offset_minutes = date_offset_days * MINUTES_PER_DAY;
    let secondary_stops: Vec<(TripStopInput, i32, i32)> = secondary
        .trip
        .stops
        .iter()
        .map(|stop| {
            (
                *stop,
                i32::from(stop.arrival) + offset_minutes,
                i32::from(stop.departure) + offset_minutes,
            )
        })
        .collect();
    let first_shared = secondary_stops
        .iter()
        .position(|(stop, ..)| primary_stations.contains(&stop.station));
    let last_shared = secondary_stops
        .iter()
        .rposition(|(stop, ..)| primary_stations.contains(&stop.station));
    let (Some(first_shared), Some(last_shared)) = (first_shared, last_shared) else {
        return None;
    };

    let mut stops: Vec<(TripStopInput, i32, i32)> = secondary_stops[..first_shared].to_vec();
    stops.extend(
        primary
            .trip
            .stops
            .iter()
            .map(|stop| (*stop, i32::from(stop.arrival), i32::from(stop.departure))),
    );
    stops.extend_from_slice(&secondary_stops[last_shared + 1..]);

    let goes_forward = stops.windows(2).all(|pair| pair[1].1 >= pair[0].2);
    let (stops, mut service_days) = if goes_forward {
        (stops, shared_days)
    } else {
        let plain = primary
            .trip
            .stops
            .iter()
            .map(|stop| (*stop, i32::from(stop.arrival), i32::from(stop.departure)));
        (plain.collect(), shared_days)
    };

    // A stop added in front can move the first departure to the previous UTC date, and the
    // times then count from that date.
    let first_departure = stops.first()?.2;
    let date_shift = first_departure.div_euclid(MINUTES_PER_DAY);
    service_days = shift_days(service_days, date_shift);
    let trip_stops = stops
        .into_iter()
        .map(|(stop, arrival, departure)| {
            Some(TripStopInput {
                arrival: u16::try_from(arrival - date_shift * MINUTES_PER_DAY).ok()?,
                departure: u16::try_from(departure - date_shift * MINUTES_PER_DAY).ok()?,
                ..stop
            })
        })
        .collect::<Option<Vec<_>>>()?;
    if service_days.is_empty() {
        return None;
    }
    let mut merged = primary.clone();
    merged.trip.stops = trip_stops;
    merged.trip.service_days = service_days;
    Some(merged)
}

/// Moves every day of a bitset by a number of days. Days that leave the window drop out.
fn shift_days(days: ServiceDayBitset, offset: i32) -> ServiceDayBitset {
    let mut shifted = ServiceDayBitset::default();
    for day in (0..128_u16).filter(|&day| days.runs_on(day)) {
        if let Ok(target) = u16::try_from(i32::from(day) + offset) {
            shifted.insert(target);
        }
    }
    shifted
}

/// Returns the days in both bitsets.
fn intersect(left: ServiceDayBitset, right: ServiceDayBitset) -> ServiceDayBitset {
    ServiceDayBitset([left.0[0] & right.0[0], left.0[1] & right.0[1]])
}

/// Returns the days in `left` that are not in `right`.
fn subtract(left: ServiceDayBitset, right: ServiceDayBitset) -> ServiceDayBitset {
    ServiceDayBitset([left.0[0] & !right.0[0], left.0[1] & !right.0[1]])
}

/// Orders a pair of feed positions with the smaller one first.
fn ordered_pair(left: usize, right: usize) -> (usize, usize) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}

#[cfg(test)]
mod tests {
    use btt_timetable::{BundleStationIndex, TrainLabelInput, TripInput};

    use super::*;

    fn stop(station: u32, arrival: u16, departure: u16) -> TripStopInput {
        TripStopInput {
            station: BundleStationIndex(station),
            arrival,
            departure,
            boarding_allowed: true,
            alighting_allowed: true,
        }
    }

    fn days(days: &[u16]) -> ServiceDayBitset {
        let mut bitset = ServiceDayBitset::default();
        for &day in days {
            bitset.insert(day);
        }
        bitset
    }

    fn trip(
        feed_index: usize,
        stops: Vec<TripStopInput>,
        service_days: ServiceDayBitset,
    ) -> ResolvedTrip {
        ResolvedTrip {
            feed_index,
            train_number: "65".into(),
            trip: TripInput {
                stops,
                service_days,
                train_label: TrainLabelInput {
                    category: "RJX".into(),
                    number: "65".into(),
                    agency: String::new(),
                },
            },
        }
    }

    #[test]
    fn copies_merge_and_extend_the_longer_trip() {
        // Feed 0 runs Munich (0), Salzburg (1), Linz (2), Vienna (3). Feed 1 only covers
        // Salzburg to Vienna but goes on to Budapest (4).
        let full = trip(
            0,
            vec![
                stop(0, 600, 600),
                stop(1, 690, 692),
                stop(2, 760, 762),
                stop(3, 840, 845),
            ],
            days(&[0, 1]),
        );
        let partial = trip(
            1,
            vec![stop(1, 690, 692), stop(3, 840, 846), stop(4, 1000, 1000)],
            days(&[0, 1, 2]),
        );
        let (trips, counts) = merge_duplicate_trips(vec![full, partial]);
        assert_eq!(counts.get(&(0, 1)), Some(&1));
        let merged = trips
            .iter()
            .find(|trip| trip.trip.stops.len() == 5)
            .unwrap();
        assert_eq!(merged.trip.service_days, days(&[0, 1]));
        assert_eq!(
            merged.trip.stops.last().unwrap().station,
            BundleStationIndex(4)
        );
        let rest = trips.iter().find(|trip| trip.feed_index == 1).unwrap();
        assert_eq!(
            rest.trip.service_days,
            days(&[2]),
            "the partial copy keeps its extra day"
        );
    }

    #[test]
    fn different_trains_with_the_same_number_stay_apart() {
        let morning = trip(0, vec![stop(0, 600, 600), stop(1, 690, 690)], days(&[0]));
        let evening = trip(
            1,
            vec![stop(0, 1200, 1200), stop(1, 1290, 1290)],
            days(&[0]),
        );
        let (trips, counts) = merge_duplicate_trips(vec![morning, evening]);
        assert_eq!(trips.len(), 2);
        assert!(counts.is_empty());
    }

    #[test]
    fn copies_on_adjacent_utc_dates_merge() {
        // Feed 1 starts the train after midnight UTC, so its service date is a day later.
        let evening_start = trip(
            0,
            vec![
                stop(0, 1400, 1400),
                stop(1, 1500, 1500),
                stop(2, 1560, 1560),
            ],
            days(&[3]),
        );
        let after_midnight = trip(
            1,
            vec![stop(1, 60, 60), stop(2, 120, 121), stop(5, 200, 200)],
            days(&[4]),
        );
        let (trips, counts) = merge_duplicate_trips(vec![evening_start, after_midnight]);
        assert_eq!(counts.len(), 1);
        assert_eq!(trips.len(), 1);
        let merged = &trips[0];
        assert_eq!(merged.trip.stops.len(), 4);
        assert_eq!(
            merged.trip.stops[3].arrival, 1640,
            "times count from the first departure's date"
        );
        assert_eq!(merged.trip.service_days, days(&[3]));
    }
}
