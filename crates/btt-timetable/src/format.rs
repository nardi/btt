//! The bundle layout. Every list is a flat array and every reference between lists is an index.
//!
//! `rkyv` archives these types so the app can map a bundle file into memory and read it without
//! parsing. The archived forms keep the same field names, with integers in little-endian order.

use std::num::NonZeroU32;

use rkyv::{Archive, Deserialize, Serialize};

/// Changes whenever the layout of `TimetableBundle` changes, so an app never reads a bundle it
/// does not understand.
pub const BUNDLE_FORMAT_VERSION: u32 = 2;

/// Marks the start of every bundle file.
pub const BUNDLE_MAGIC: [u8; 8] = *b"BTTBUNDL";

/// Longest validity window a bundle can cover, limited by the width of `ServiceDayBitset`.
pub const MAXIMUM_VALIDITY_DAYS: u16 = 128;

/// Defines a `u32` index into one array of the bundle, so an index into one array cannot be used
/// on another.
macro_rules! bundle_index {
    ($(#[$type_meta:meta])* $type_name:ident) => {
        $(#[$type_meta])*
        #[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[rkyv(derive(Debug, Clone, Copy, PartialEq, Eq, Hash))]
        pub struct $type_name(pub u32);

        impl $type_name {
            /// Returns the index as a `usize` for slice access.
            pub const fn as_usize(self) -> usize {
                self.0 as usize
            }
        }

        impl $crate::format::ArchivedIndex for <$type_name as Archive>::Archived {
            type Native = $type_name;

            fn to_native(&self) -> $type_name {
                $type_name(self.0.to_native())
            }
        }
    };
}

/// Converts an archived index back to its native type.
pub trait ArchivedIndex {
    /// The native index type.
    type Native;

    /// Returns the native index.
    fn to_native(&self) -> Self::Native;
}

bundle_index! {
    /// Points into `TimetableBundle::stations`.
    BundleStationIndex
}

bundle_index! {
    /// Points into `TimetableBundle::raptor_routes`.
    RaptorRouteIndex
}

bundle_index! {
    /// Points into `TimetableBundle::trips`.
    BundleTripIndex
}

bundle_index! {
    /// Points into `TimetableBundle::distinct_service_days`.
    ServiceDayBitsetIndex
}

bundle_index! {
    /// Points into `TimetableBundle::train_labels`.
    TrainLabelIndex
}

/// Points into `TimetableBundle::time_zone_names`.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[rkyv(derive(Debug, Clone, Copy, PartialEq, Eq))]
pub struct TimeZoneIndex(pub u16);

/// A string stored in `TimetableBundle::string_pool`.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[rkyv(derive(Debug, Clone, Copy))]
pub struct PooledString {
    /// Byte offset of the string in the pool.
    pub byte_offset: u32,
    /// Length of the string in bytes.
    pub byte_length: u16,
}

/// Days on which a trip runs, as bits over the bundle's validity window.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[rkyv(derive(Debug, Clone, Copy))]
pub struct ServiceDayBitset(pub [u64; 2]);

impl ServiceDayBitset {
    /// Returns whether bit `day` is set. Days outside the window are never set.
    pub const fn runs_on(self, day: u16) -> bool {
        if day >= MAXIMUM_VALIDITY_DAYS {
            return false;
        }
        self.0[(day / 64) as usize] & (1 << (day % 64)) != 0
    }

    /// Sets bit `day`. Days outside the window are ignored.
    pub fn insert(&mut self, day: u16) {
        if day < MAXIMUM_VALIDITY_DAYS {
            self.0[usize::from(day / 64)] |= 1 << (day % 64);
        }
    }

    /// Returns the days set in either bitset.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self([self.0[0] | other.0[0], self.0[1] | other.0[1]])
    }

    /// Returns whether no day is set.
    pub const fn is_empty(self) -> bool {
        self.0[0] == 0 && self.0[1] == 0
    }

    /// Returns the highest day that is set, or `None` when no day is set.
    pub fn last_day(self) -> Option<u16> {
        let (word, first_day_of_word) = if self.0[1] == 0 {
            (self.0[0], 0)
        } else {
            (self.0[1], 64)
        };
        // The highest set bit of the word is the base 2 logarithm rounded down.
        let day = first_day_of_word + word.checked_ilog2()?;
        u16::try_from(day).ok()
    }
}

impl ArchivedServiceDayBitset {
    /// Returns the native bitset.
    pub fn to_native(&self) -> ServiceDayBitset {
        ServiceDayBitset([self.0[0].to_native(), self.0[1].to_native()])
    }
}

/// Minutes since 00:00 UTC on the UTC service date of a trip. Can pass 1,440 for trips that run
/// past midnight.
#[derive(
    Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord,
)]
#[rkyv(derive(Debug, Clone, Copy))]
pub struct MinutesSinceServiceDateUtc(pub u16);

/// Arrival and departure of one trip at one stop.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[rkyv(derive(Debug, Clone, Copy))]
pub struct StopEventTimes {
    /// When the trip arrives.
    pub arrival: MinutesSinceServiceDateUtc,
    /// When the trip departs, never before the arrival.
    pub departure: MinutesSinceServiceDateUtc,
}

/// A stop of a stop pattern, packed into 32 bits.
///
/// The low 30 bits hold the bundle station index, bit 30 means passengers may not board and
/// bit 31 means passengers may not alight.
#[derive(
    Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord,
)]
#[rkyv(derive(Debug, Clone, Copy))]
pub struct PackedPatternStop(pub u32);

impl PackedPatternStop {
    /// Largest station index that fits in the packed form.
    pub const MAXIMUM_STATION_INDEX: u32 = (1 << 30) - 1;
    const NO_BOARDING_BIT: u32 = 1 << 30;
    const NO_ALIGHTING_BIT: u32 = 1 << 31;

    /// Packs a station and its boarding rules. Returns `None` for a station index that does
    /// not fit.
    pub const fn new(
        station: BundleStationIndex,
        boarding_allowed: bool,
        alighting_allowed: bool,
    ) -> Option<Self> {
        if station.0 > Self::MAXIMUM_STATION_INDEX {
            return None;
        }
        let mut packed = station.0;
        if !boarding_allowed {
            packed |= Self::NO_BOARDING_BIT;
        }
        if !alighting_allowed {
            packed |= Self::NO_ALIGHTING_BIT;
        }
        Some(Self(packed))
    }

    /// Returns the station.
    pub const fn station(self) -> BundleStationIndex {
        BundleStationIndex(self.0 & Self::MAXIMUM_STATION_INDEX)
    }

    /// Returns whether passengers may board here.
    pub const fn boarding_allowed(self) -> bool {
        self.0 & Self::NO_BOARDING_BIT == 0
    }

    /// Returns whether passengers may alight here.
    pub const fn alighting_allowed(self) -> bool {
        self.0 & Self::NO_ALIGHTING_BIT == 0
    }
}

/// A station that trips stop at.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct BundleStation {
    /// Name shown to the traveller.
    pub name: PooledString,
    /// Latitude in millionths of a degree.
    pub latitude_microdegrees: i32,
    /// Longitude in millionths of a degree.
    pub longitude_microdegrees: i32,
    /// IANA time zone of the station.
    pub time_zone: TimeZoneIndex,
    /// Station ID in the Trainline stations dataset, when the builder matched one.
    pub trainline_station_id: Option<NonZeroU32>,
}

/// A walking or local transit connection between two stations.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[rkyv(derive(Debug, Clone, Copy))]
pub struct Footpath {
    /// Station the footpath leads to.
    pub to_station: BundleStationIndex,
    /// Time to get there.
    pub duration_minutes: u16,
}

/// A place where a RAPTOR route stops at a station, as its position in the route's stop
/// sequence.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[rkyv(derive(Debug, Clone, Copy))]
pub struct RouteStopPosition {
    /// The route.
    pub raptor_route: RaptorRouteIndex,
    /// Position of the station in the route's stop sequence.
    pub stop_position: u16,
}

/// Trips with the same stop pattern where no trip overtakes another. RAPTOR scans these.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[rkyv(derive(Debug, Clone, Copy))]
pub struct RaptorRoute {
    /// Index of the route's first stop in `TimetableBundle::pattern_stops`.
    pub first_pattern_stop_index: u32,
    /// Number of stops of every trip on the route.
    pub stop_count: u16,
    /// Index of the route's first trip in `TimetableBundle::trips`. Trips are sorted by
    /// departure from the first stop.
    pub first_trip_index: u32,
    /// Number of trips on the route.
    pub trip_count: u32,
    /// Times of trip `t` on this route start at `first_stop_event_index + t * stop_count`.
    pub first_stop_event_index: u32,
    /// Longest trip on this route in whole days, rounded up. A trip can reach a stop this many
    /// days after its service date.
    pub longest_trip_days: u8,
}

/// One trip, which runs on the days of its service day bitset.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[rkyv(derive(Debug, Clone, Copy))]
pub struct BundleTrip {
    /// Days on which the trip runs.
    pub service_days: ServiceDayBitsetIndex,
    /// How the train is shown to the traveller.
    pub train_label: TrainLabelIndex,
}

/// How a train is shown to the traveller, shared by all trips of the same train.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[rkyv(derive(Debug, Clone, Copy))]
pub struct BundleTrainLabel {
    /// Product or category, such as "ICE".
    pub category: PooledString,
    /// Train number within the category.
    pub number: PooledString,
    /// Name of the operating agency.
    pub agency: PooledString,
    /// Whether the train has couchettes or sleepers.
    pub offers_berths: bool,
}

/// Source and license of a feed that went into the bundle.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[rkyv(derive(Debug, Clone, Copy))]
pub struct FeedAttribution {
    /// Name of the feed, such as "DELFI".
    pub feed_name: PooledString,
    /// License of the feed, such as "CC-BY-4.0".
    pub license: PooledString,
    /// Where the feed or its license is published.
    pub url: PooledString,
}

/// Variable-length lists per item, stored as one array of values and one of row offsets.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct CompressedSparseRows<T> {
    /// Values of row `i` are `values[row_offsets[i]..row_offsets[i + 1]]`.
    pub row_offsets: Vec<u32>,
    /// All values, row after row.
    pub values: Vec<T>,
}

/// Facts about the bundle as a whole.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[rkyv(derive(Debug, Clone, Copy))]
pub struct BundleHeader {
    /// Always `BUNDLE_MAGIC`.
    pub magic: [u8; 8],
    /// Layout version, `BUNDLE_FORMAT_VERSION` when written.
    pub format_version: u32,
    /// Version of the data as `yyyymmddNN`.
    pub bundle_version: u32,
    /// First day of the validity window, in days since 1970-01-01.
    pub validity_start_days_since_epoch: i32,
    /// Number of days in the validity window.
    pub validity_days: u16,
}

/// A merged, deduplicated timetable in the layout RAPTOR needs.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TimetableBundle {
    /// Facts about the bundle as a whole.
    pub header: BundleHeader,
    /// Every station with at least one trip.
    pub stations: Vec<BundleStation>,
    /// IANA names of the time zones `BundleStation::time_zone` refers to.
    pub time_zone_names: Vec<PooledString>,
    /// Per station, the routes that stop there.
    pub route_stops_by_station: CompressedSparseRows<RouteStopPosition>,
    /// Per station, the footpaths that start there.
    pub footpaths_by_station: CompressedSparseRows<Footpath>,
    /// Union of the service days of all trips that stop at each station.
    pub service_days_by_station: Vec<ServiceDayBitset>,
    /// Routes as RAPTOR scans them.
    pub raptor_routes: Vec<RaptorRoute>,
    /// Stop sequences of all routes, route after route.
    pub pattern_stops: Vec<PackedPatternStop>,
    /// All trips, route after route.
    pub trips: Vec<BundleTrip>,
    /// Trip-major. See `RaptorRoute::first_stop_event_index`.
    pub stop_events: Vec<StopEventTimes>,
    /// Each distinct service day bitset once.
    pub distinct_service_days: Vec<ServiceDayBitset>,
    /// Each distinct train label once.
    pub train_labels: Vec<BundleTrainLabel>,
    /// Text of every `PooledString`.
    pub string_pool: String,
    /// Feeds that went into the bundle.
    pub feed_attributions: Vec<FeedAttribution>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_day_is_the_highest_set_bit_of_either_word() {
        let mut days = ServiceDayBitset::default();
        assert_eq!(days.last_day(), None);
        days.insert(0);
        assert_eq!(days.last_day(), Some(0));
        days.insert(63);
        assert_eq!(days.last_day(), Some(63));
        days.insert(64);
        assert_eq!(days.last_day(), Some(64));
        days.insert(127);
        assert_eq!(days.last_day(), Some(127));
    }

    #[test]
    fn packed_pattern_stop_keeps_station_and_flags() {
        let packed = PackedPatternStop::new(BundleStationIndex(12_345), false, true).unwrap();
        assert_eq!(packed.station(), BundleStationIndex(12_345));
        assert!(!packed.boarding_allowed());
        assert!(packed.alighting_allowed());
        assert!(PackedPatternStop::new(BundleStationIndex(1 << 30), true, true).is_none());
    }

    #[test]
    fn service_day_bitset_covers_both_words() {
        let mut days = ServiceDayBitset::default();
        days.insert(3);
        days.insert(100);
        days.insert(200);
        assert!(days.runs_on(3) && days.runs_on(100));
        assert!(!days.runs_on(4) && !days.runs_on(200));
    }
}
