//! The compact timetable bundle: its format, a builder, the loader and a station index.
//!
//! A bundle holds a merged rail timetable in the layout the RAPTOR router needs. The builder
//! CLI writes it with `rkyv`, and the app maps the file into memory and reads it in place.

pub mod builder;
pub mod format;
pub mod loaded;
pub mod manifest;
pub mod station_index;

pub use builder::{
    BundleBuildError, BundleBuilder, FeedAttributionInput, StationInput, TrainLabelInput,
    TripInput, TripStopInput,
};
pub use format::{
    BundleStationIndex, BundleTripIndex, Footpath, LayoutPageKind, MinutesSinceServiceDateUtc,
    PackedPatternStop, RaptorRoute, RaptorRouteIndex, RouteStopPosition, ServiceDayBitset,
    StopEventTimes, TimetableBundle,
};
pub use loaded::{BundleLoadError, LayoutPage, LoadedTimetableBundle};
pub use station_index::{NearbyStation, StationIndex};
