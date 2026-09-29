//! Steps 6 and 7: finds copies of the same train in different feeds and merges them.
//!
//! After UTC resolution the trips already sit in Rust memory, and trips with the same train
//! number form small groups, so both steps run here without a round trip through the database.

use btt_timetable::{BundleStationIndex, ServiceDayBitset, TripStopInput};
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

/// Largest distance between two stations that copies of a train may use for the same stop, the
/// distance nigiri uses for this check. Feeds can place one station a few hundred meters apart,
/// for example at the building in one feed and at a platform in the other.
pub const SAME_STOP_RADIUS_METERS: f64 = 600.0;

/// Pairs of distinct stations within `SAME_STOP_RADIUS_METERS`, in both directions.
#[derive(Debug, Default)]
pub struct NearbyStationPairs(pub FxHashSet<(BundleStationIndex, BundleStationIndex)>);

impl NearbyStationPairs {
    /// Returns whether two stations can stand for the same stop of a train.
    fn same_stop(&self, first: BundleStationIndex, second: BundleStationIndex) -> bool {
        first == second || self.0.contains(&(first, second))
    }
}

/// Two trips that are the same train on some days.
struct DuplicateMatch {
    /// Days the second trip's service date lies after the first trip's.
    date_offset_days: i32,
    /// Days, counted on the first trip's service dates, on which both run.
    shared_days: ServiceDayBitset,
}

/// Merges copies of the same train and returns the remaining trips with merge counts. Two
/// copies share a stop when they use the same station or two stations in `nearby`.
pub fn merge_duplicate_trips(
    trips: Vec<ResolvedTrip>,
    nearby: &NearbyStationPairs,
) -> (Vec<ResolvedTrip>, MergeCounts) {
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
        result.extend(merge_group(group, &mut merge_counts, nearby));
    }
    (result, merge_counts)
}

/// Merges duplicates within one group of trips that share a train number.
fn merge_group(
    mut group: Vec<ResolvedTrip>,
    merge_counts: &mut MergeCounts,
    nearby: &NearbyStationPairs,
) -> Vec<ResolvedTrip> {
    // Each merge removes the shared days from both copies, so the loop ends. The bound only
    // guards against a mistake in that reasoning.
    let merge_limit = group.len() * group.len() * 4 + 4;
    for _ in 0..merge_limit {
        let Some((first, second, found)) = find_duplicate_pair(&group, nearby) else {
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
            nearby,
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
fn find_duplicate_pair(
    group: &[ResolvedTrip],
    nearby: &NearbyStationPairs,
) -> Option<(usize, usize, DuplicateMatch)> {
    for first in 0..group.len() {
        for second in first + 1..group.len() {
            if let Some(found) = duplicate_match(&group[first], &group[second], nearby) {
                return Some((first, second, found));
            }
        }
    }
    None
}

/// Checks whether two trips are the same train on at least one day. The second trip's service
/// date may lie a day before or after the first's, when one feed starts the train before
/// midnight in UTC and the other after.
fn duplicate_match(
    first: &ResolvedTrip,
    second: &ResolvedTrip,
    nearby: &NearbyStationPairs,
) -> Option<DuplicateMatch> {
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
                    nearby.same_stop(second_stop.station, first_stop.station)
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
    nearby: &NearbyStationPairs,
) -> Vec<ResolvedTrip> {
    let mut parts = Vec::with_capacity(3);
    let primary_rest = subtract(primary.trip.service_days, shared_days);
    let secondary_rest = subtract(
        secondary.trip.service_days,
        shift_days(shared_days, date_offset_days),
    );
    if let Some(merged) = extended_trip(primary, secondary, date_offset_days, shared_days, nearby) {
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
    nearby: &NearbyStationPairs,
) -> Option<ResolvedTrip> {
    let primary_serves = |station: BundleStationIndex| {
        primary
            .trip
            .stops
            .iter()
            .any(|primary_stop| nearby.same_stop(primary_stop.station, station))
    };
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
        .position(|(stop, ..)| primary_serves(stop.station));
    let last_shared = secondary_stops
        .iter()
        .rposition(|(stop, ..)| primary_serves(stop.station));
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
    // Some feeds file night trains as ordinary long-distance trains. When only one copy knows
    // that the train has berths, its label is the better one.
    if secondary.trip.train_label.offers_berths && !primary.trip.train_label.offers_berths {
        merged.trip.train_label = secondary.trip.train_label.clone();
    }
    Some(merged)
}

/// Largest difference between the times of a train with berths and a copy without them at a
/// shared station, for `link_berths`. Split copies often differ by a few minutes at the
/// station where one feed's part ends and the other's begins.
const BERTH_LINK_TOLERANCE_MINUTES: i32 = 10;

/// Returns the part of a train number that stays the same across operators. DB files
/// codeshared night trains with a leading 4 or 6 on a five-digit number, such as 60470 for
/// Nightjet 470. The merge doesn't use this key, because such numbers also mark portions of
/// one night train, such as Nightjet 421 to Innsbruck and 40421 to Vienna.
fn train_number_key(train_number: &str) -> &str {
    let is_prefixed = train_number.len() == 5
        && train_number.bytes().all(|byte| byte.is_ascii_digit())
        && matches!(train_number.as_bytes()[0], b'4' | b'6');
    if !is_prefixed {
        return train_number;
    }
    let key = train_number[1..].trim_start_matches('0');
    if key.is_empty() { train_number } else { key }
}

/// Gives the label of a train with berths to trips without berths that are the same train in
/// another feed, and returns how many trips it changed.
///
/// This covers copies that the merge can't join, because they overlap at a single station,
/// such as SBB's Nightjet 470 from Zürich to Singen and DB's IC 60470 from Singen onwards.
/// Two trips count as the same train when their train number keys match, they run on a
/// shared day, and they stop at one station within `BERTH_LINK_TOLERANCE_MINUTES` of each
/// other. The whole trip gets the label, including days the other copy doesn't run on,
/// because a train with berths on one day has them on the others too.
pub fn link_berths(trips: &mut [ResolvedTrip], nearby: &NearbyStationPairs) -> usize {
    let mut positions_by_key: FxHashMap<&str, Vec<usize>> = FxHashMap::default();
    for (position, trip) in trips.iter().enumerate() {
        if !trip.train_number.is_empty() {
            positions_by_key
                .entry(train_number_key(&trip.train_number))
                .or_default()
                .push(position);
        }
    }
    let mut new_labels = Vec::new();
    for positions in positions_by_key.values() {
        for &without in positions {
            if trips[without].trip.train_label.offers_berths {
                continue;
            }
            // A copy with the same number goes first. Night trains run in portions with
            // related numbers, such as Nightjet 421 to Innsbruck and 40421 to Vienna, and a
            // trip should take the label of its own portion.
            let berth_copy = positions
                .iter()
                .copied()
                .filter(|&with| {
                    trips[with].trip.train_label.offers_berths
                        && touches(&trips[with], &trips[without], nearby)
                })
                .min_by_key(|&with| trips[with].train_number != trips[without].train_number);
            if let Some(with) = berth_copy {
                new_labels.push((without, trips[with].trip.train_label.clone()));
            }
        }
    }
    let linked = new_labels.len();
    for (position, label) in new_labels {
        trips[position].trip.train_label = label;
    }
    linked
}

/// Returns whether two trips run on a shared day, allowing a one-day offset between their
/// service dates, and stop at one station within `BERTH_LINK_TOLERANCE_MINUTES`.
fn touches(first: &ResolvedTrip, second: &ResolvedTrip, nearby: &NearbyStationPairs) -> bool {
    (-1..=1).any(|date_offset_days| {
        let shared_days = intersect(
            first.trip.service_days,
            shift_days(second.trip.service_days, -date_offset_days),
        );
        let offset_minutes = date_offset_days * MINUTES_PER_DAY;
        !shared_days.is_empty()
            && first.trip.stops.iter().any(|first_stop| {
                second.trip.stops.iter().any(|second_stop| {
                    let close = |first_minute: u16, second_minute: u16| {
                        (i32::from(second_minute) + offset_minutes - i32::from(first_minute)).abs()
                            <= BERTH_LINK_TOLERANCE_MINUTES
                    };
                    nearby.same_stop(first_stop.station, second_stop.station)
                        && (close(first_stop.arrival, second_stop.arrival)
                            || close(first_stop.departure, second_stop.departure))
                })
            })
    })
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
    use btt_timetable::{TrainLabelInput, TripInput};

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
                    offers_berths: false,
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
        let (trips, counts) =
            merge_duplicate_trips(vec![full, partial], &NearbyStationPairs::default());
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
        let (trips, counts) =
            merge_duplicate_trips(vec![morning, evening], &NearbyStationPairs::default());
        assert_eq!(trips.len(), 2);
        assert!(counts.is_empty());
    }

    #[test]
    fn copies_at_nearby_stations_merge_and_extend() {
        // Feed 1 uses station 11 for Salzburg and 12 for Vienna, each close to feed 0's.
        let nearby = NearbyStationPairs(
            [(1, 11), (11, 1), (3, 12), (12, 3)]
                .into_iter()
                .map(|(first, second)| (BundleStationIndex(first), BundleStationIndex(second)))
                .collect(),
        );
        let full = trip(
            0,
            vec![stop(0, 600, 600), stop(1, 690, 692), stop(3, 840, 845)],
            days(&[0]),
        );
        let partial = trip(
            1,
            vec![stop(11, 690, 692), stop(12, 840, 845), stop(4, 1000, 1000)],
            days(&[0]),
        );
        let (trips, counts) = merge_duplicate_trips(vec![full, partial], &nearby);
        assert_eq!(counts.get(&(0, 1)), Some(&1));
        assert_eq!(trips.len(), 1);
        let stations: Vec<u32> = trips[0]
            .trip
            .stops
            .iter()
            .map(|stop| stop.station.0)
            .collect();
        assert_eq!(
            stations,
            [0, 1, 3, 4],
            "Budapest extends the train past Vienna"
        );
    }

    #[test]
    fn a_copy_that_knows_about_berths_gives_its_label() {
        // The longer copy files the train as an Intercity, the shorter one as a Nightjet.
        let intercity = trip(
            0,
            vec![stop(0, 600, 600), stop(1, 690, 692), stop(3, 840, 845)],
            days(&[0]),
        );
        let mut nightjet = trip(1, vec![stop(0, 600, 600), stop(1, 690, 692)], days(&[0]));
        nightjet.trip.train_label.category = "NJ".into();
        nightjet.trip.train_label.offers_berths = true;
        let (trips, _) =
            merge_duplicate_trips(vec![intercity, nightjet], &NearbyStationPairs::default());
        assert_eq!(trips.len(), 1);
        assert_eq!(trips[0].trip.stops.len(), 3);
        assert_eq!(trips[0].trip.train_label.category, "NJ");
        assert!(trips[0].trip.train_label.offers_berths);
    }

    #[test]
    fn copies_at_distant_stations_stay_apart() {
        let first = trip(0, vec![stop(1, 690, 692), stop(3, 840, 845)], days(&[0]));
        let second = trip(1, vec![stop(11, 690, 692), stop(12, 840, 845)], days(&[0]));
        let (trips, counts) =
            merge_duplicate_trips(vec![first, second], &NearbyStationPairs::default());
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
        let (trips, counts) = merge_duplicate_trips(
            vec![evening_start, after_midnight],
            &NearbyStationPairs::default(),
        );
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

    #[test]
    fn codeshare_prefixes_are_dropped_from_the_key() {
        assert_eq!(train_number_key("40421"), "421");
        assert_eq!(train_number_key("60470"), "470");
        assert_eq!(train_number_key("12345"), "12345");
        assert_eq!(train_number_key("4211"), "4211");
        assert_eq!(train_number_key("40000"), "40000");
    }

    #[test]
    fn a_split_night_train_passes_its_berths_on() {
        // SBB runs the Nightjet from Zürich (0) to Singen (1). DB continues it from Singen to
        // Hamburg (2) as an IC with another number and without berths.
        let mut swiss_part = trip(
            0,
            vec![stop(0, 1140, 1140), stop(1, 1207, 1207)],
            days(&[3]),
        );
        swiss_part.train_number = "470".into();
        swiss_part.trip.train_label.category = "NJ".into();
        swiss_part.trip.train_label.number = "470".into();
        swiss_part.trip.train_label.offers_berths = true;
        let mut german_part = trip(
            1,
            vec![stop(1, 1210, 1240), stop(2, 1880, 1880)],
            days(&[3]),
        );
        german_part.train_number = "60470".into();
        german_part.trip.train_label.category = "IC".into();
        german_part.trip.train_label.number = "60470".into();
        // Another IC at Singen at the same time, with an unrelated number, stays as it is.
        let other = trip(
            1,
            vec![stop(1, 1205, 1215), stop(2, 1900, 1900)],
            days(&[3]),
        );

        let mut trips = vec![swiss_part, german_part, other];
        assert_eq!(link_berths(&mut trips, &NearbyStationPairs::default()), 1);
        assert!(trips[1].trip.train_label.offers_berths);
        assert_eq!(trips[1].trip.train_label.number, "470");
        assert!(!trips[2].trip.train_label.offers_berths);
    }

    #[test]
    fn a_trip_takes_the_label_of_its_own_portion() {
        // Nightjet 421 and 40421 run coupled from Amsterdam (0) to Köln (1). A copy of 40421
        // without berths touches both at Köln and takes the label with its own number.
        let berth_portion = |number: &str| {
            let mut portion = trip(
                0,
                vec![stop(0, 1100, 1100), stop(1, 1250, 1255)],
                days(&[3]),
            );
            portion.train_number = number.into();
            portion.trip.train_label.category = "NJ".into();
            portion.trip.train_label.number = number.into();
            portion.trip.train_label.offers_berths = true;
            portion
        };
        let mut seated_copy = trip(
            1,
            vec![stop(1, 1250, 1255), stop(2, 1700, 1700)],
            days(&[3]),
        );
        seated_copy.train_number = "40421".into();

        let mut trips = vec![berth_portion("421"), berth_portion("40421"), seated_copy];
        assert_eq!(link_berths(&mut trips, &NearbyStationPairs::default()), 1);
        assert_eq!(trips[2].trip.train_label.number, "40421");
    }
}
