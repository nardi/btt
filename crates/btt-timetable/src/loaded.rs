//! A bundle loaded from a file or from memory, validated once and read without copying.

use std::fs::File;
use std::path::Path;

use btt_model::{GeoCoordinates, Latitude, Longitude};
use chrono::{Days, NaiveDate};
use memmap2::Mmap;
use rkyv::rancor;
use rkyv::util::AlignedVec;
use thiserror::Error;

use crate::format::{
    ArchivedBundleStation, ArchivedFeedAttribution, ArchivedFootpath, ArchivedIndex,
    ArchivedLayoutPageKind, ArchivedPooledString, ArchivedRouteStopPosition,
    ArchivedTimetableBundle, BUNDLE_FORMAT_VERSION, BUNDLE_MAGIC, BundleStationIndex,
    BundleTripIndex, Footpath, LayoutPageKind, MinutesSinceServiceDateUtc, PackedPatternStop,
    RaptorRoute, RaptorRouteIndex, RouteStopPosition, ServiceDayBitset, StopEventTimes,
    TimetableBundle,
};

/// A page that shows the layout of a train, read from a bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutPage<'bundle> {
    /// What the page is about.
    pub kind: LayoutPageKind,
    /// Code of the operator or country on the site, such as "DB".
    pub operator: &'bundle str,
    /// Category on the site, such as "ICE". Empty for the list of an operator.
    pub category: &'bundle str,
    /// Train number on the site. Empty for lists.
    pub number: &'bundle str,
    /// Timetable year of the page, such as 2026.
    pub year: u16,
}

/// Errors from loading a bundle.
#[derive(Debug, Error)]
pub enum BundleLoadError {
    /// The file could not be opened, mapped or written.
    #[error("could not access the bundle file: {0}")]
    Io(#[from] std::io::Error),
    /// The bytes are not a valid archived bundle.
    #[error("the bundle is corrupt: {0}")]
    Corrupt(rancor::Error),
    /// The bundle is not a BTT bundle or has a layout this build does not read.
    #[error("the bundle has format version {found}, but this build reads version {expected}")]
    UnsupportedFormat {
        /// Version in the bundle, or 0 when the magic number is wrong.
        found: u32,
        /// Version this build reads.
        expected: u32,
    },
}

/// Where the bundle bytes live. Tests keep them in memory, the app maps a file.
enum BundleBytes {
    /// A memory-mapped bundle file.
    Mapped(Mmap),
    /// Bytes in memory, aligned for `rkyv`.
    Owned(AlignedVec),
}

impl BundleBytes {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Mapped(map) => map,
            Self::Owned(bytes) => bytes,
        }
    }
}

/// A validated bundle. Accessors read the archived data in place and return native values.
pub struct LoadedTimetableBundle {
    /// The archived bundle.
    bytes: BundleBytes,
}

impl LoadedTimetableBundle {
    /// Maps a bundle file into memory and validates it.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be mapped, is corrupt or has another format
    /// version.
    pub fn open(path: &Path) -> Result<Self, BundleLoadError> {
        let file = File::open(path)?;
        // SAFETY: The map is read-only and BTT never writes to a bundle file after publishing
        // it, because every bundle version gets its own file name.
        #[allow(unsafe_code)]
        let map = unsafe { Mmap::map(&file)? };
        Self::validated(BundleBytes::Mapped(map))
    }

    /// Archives a bundle in memory and validates the result, for tests and for writing files.
    ///
    /// # Errors
    ///
    /// Returns an error when archiving fails.
    pub fn from_bundle(bundle: &TimetableBundle) -> Result<Self, BundleLoadError> {
        let bytes = rkyv::to_bytes::<rancor::Error>(bundle).map_err(BundleLoadError::Corrupt)?;
        Self::validated(BundleBytes::Owned(bytes))
    }

    /// Writes a bundle to a file.
    ///
    /// # Errors
    ///
    /// Returns an error when archiving or writing fails.
    pub fn write_bundle(bundle: &TimetableBundle, path: &Path) -> Result<(), BundleLoadError> {
        let bytes = rkyv::to_bytes::<rancor::Error>(bundle).map_err(BundleLoadError::Corrupt)?;
        std::fs::write(path, &bytes)?;
        Ok(())
    }

    /// Returns the size of the archived bundle in bytes.
    pub fn byte_size(&self) -> usize {
        self.bytes.as_slice().len()
    }

    /// Checks the archive structure and the format version once, so later reads can skip the
    /// check.
    fn validated(bytes: BundleBytes) -> Result<Self, BundleLoadError> {
        let archived = rkyv::access::<ArchivedTimetableBundle, rancor::Error>(bytes.as_slice())
            .map_err(BundleLoadError::Corrupt)?;
        let header = &archived.header;
        if header.magic != BUNDLE_MAGIC {
            return Err(BundleLoadError::UnsupportedFormat {
                found: 0,
                expected: BUNDLE_FORMAT_VERSION,
            });
        }
        let format_version = header.format_version.to_native();
        if format_version != BUNDLE_FORMAT_VERSION {
            return Err(BundleLoadError::UnsupportedFormat {
                found: format_version,
                expected: BUNDLE_FORMAT_VERSION,
            });
        }
        Ok(Self { bytes })
    }

    /// Returns the archived root.
    #[inline]
    fn archived(&self) -> &ArchivedTimetableBundle {
        // SAFETY: `validated` checked these exact bytes with `rkyv::access` before the value
        // was built, and the bytes never change afterwards. Mapped files are read-only and
        // never rewritten, see `open`.
        #[allow(unsafe_code)]
        unsafe {
            rkyv::access_unchecked::<ArchivedTimetableBundle>(self.bytes.as_slice())
        }
    }

    /// Returns the text of a pooled string.
    #[inline]
    fn pooled(&self, pooled: ArchivedPooledString) -> &str {
        let start = pooled.byte_offset.to_native() as usize;
        let end = start + usize::from(pooled.byte_length.to_native());
        self.archived()
            .string_pool
            .get(start..end)
            .unwrap_or_default()
    }

    /// Returns the version of the data as `yyyymmddNN`.
    pub fn bundle_version(&self) -> u32 {
        self.archived().header.bundle_version.to_native()
    }

    /// Returns the first day of the validity window.
    pub fn validity_start(&self) -> NaiveDate {
        let days = self
            .archived()
            .header
            .validity_start_days_since_epoch
            .to_native();
        let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or_default();
        u64::try_from(days)
            .ok()
            .and_then(|days| epoch.checked_add_days(Days::new(days)))
            .unwrap_or(epoch)
    }

    /// Returns the number of days in the validity window.
    pub fn validity_days(&self) -> u16 {
        self.archived().header.validity_days.to_native()
    }

    /// Returns the number of stations.
    pub fn station_count(&self) -> usize {
        self.archived().stations.len()
    }

    /// Returns a station's archived record.
    #[inline]
    fn station(&self, station: BundleStationIndex) -> &ArchivedBundleStation {
        &self.archived().stations[station.as_usize()]
    }

    /// Returns a station's name.
    pub fn station_name(&self, station: BundleStationIndex) -> &str {
        self.pooled(self.station(station).name)
    }

    /// Returns a station's location.
    pub fn station_coordinates(&self, station: BundleStationIndex) -> GeoCoordinates {
        let record = self.station(station);
        // Coordinates come from `GeoCoordinates` in the builder, so they are always in range.
        GeoCoordinates {
            latitude: Latitude::from_microdegrees(record.latitude_microdegrees.to_native())
                .unwrap_or(Latitude::ZERO),
            longitude: Longitude::from_microdegrees(record.longitude_microdegrees.to_native())
                .unwrap_or(Longitude::ZERO),
        }
    }

    /// Returns a station's IANA time zone name.
    pub fn station_time_zone(&self, station: BundleStationIndex) -> &str {
        let index = self.station(station).time_zone.0.to_native();
        self.archived()
            .time_zone_names
            .get(usize::from(index))
            .map_or("UTC", |name| self.pooled(*name))
    }

    /// Returns a station's ID in the Trainline stations dataset, if the builder matched one.
    pub fn station_trainline_id(&self, station: BundleStationIndex) -> Option<u32> {
        self.station(station)
            .trainline_station_id
            .as_ref()
            .map(|id| id.get())
    }

    /// Returns the days on which any trip stops at a station.
    #[inline]
    pub fn station_service_days(&self, station: BundleStationIndex) -> ServiceDayBitset {
        self.archived().service_days_by_station[station.as_usize()].to_native()
    }

    /// Returns the routes that stop at a station, with the stop position on each route.
    #[inline]
    pub fn route_stops_at(
        &self,
        station: BundleStationIndex,
    ) -> impl ExactSizeIterator<Item = RouteStopPosition> + '_ {
        row_of(
            &self.archived().route_stops_by_station.row_offsets,
            &self.archived().route_stops_by_station.values,
            station,
        )
        .iter()
        .map(|position: &ArchivedRouteStopPosition| RouteStopPosition {
            raptor_route: position.raptor_route.to_native(),
            stop_position: position.stop_position.to_native(),
        })
    }

    /// Returns the footpaths that start at a station.
    #[inline]
    pub fn footpaths_from(
        &self,
        station: BundleStationIndex,
    ) -> impl ExactSizeIterator<Item = Footpath> + '_ {
        row_of(
            &self.archived().footpaths_by_station.row_offsets,
            &self.archived().footpaths_by_station.values,
            station,
        )
        .iter()
        .map(|footpath: &ArchivedFootpath| Footpath {
            to_station: footpath.to_station.to_native(),
            duration_minutes: footpath.duration_minutes.to_native(),
        })
    }

    /// Returns the number of RAPTOR routes.
    pub fn route_count(&self) -> usize {
        self.archived().raptor_routes.len()
    }

    /// Returns the number of trips.
    pub fn trip_count(&self) -> usize {
        self.archived().trips.len()
    }

    /// Returns a route's layout.
    #[inline]
    pub fn route(&self, route: RaptorRouteIndex) -> RaptorRoute {
        let archived = &self.archived().raptor_routes[route.as_usize()];
        RaptorRoute {
            first_pattern_stop_index: archived.first_pattern_stop_index.to_native(),
            stop_count: archived.stop_count.to_native(),
            first_trip_index: archived.first_trip_index.to_native(),
            trip_count: archived.trip_count.to_native(),
            first_stop_event_index: archived.first_stop_event_index.to_native(),
            longest_trip_days: archived.longest_trip_days,
        }
    }

    /// Returns the stop at a position of a route.
    #[inline]
    pub fn pattern_stop(&self, route: &RaptorRoute, stop_position: u16) -> PackedPatternStop {
        let index = route.first_pattern_stop_index as usize + usize::from(stop_position);
        PackedPatternStop(self.archived().pattern_stops[index].0.to_native())
    }

    /// Returns the times of the `trip_offset`-th trip of a route at a stop position.
    #[inline]
    pub fn stop_event(
        &self,
        route: &RaptorRoute,
        trip_offset: u32,
        stop_position: u16,
    ) -> StopEventTimes {
        let index = route.first_stop_event_index as usize
            + trip_offset as usize * usize::from(route.stop_count)
            + usize::from(stop_position);
        let archived = &self.archived().stop_events[index];
        StopEventTimes {
            arrival: MinutesSinceServiceDateUtc(archived.arrival.0.to_native()),
            departure: MinutesSinceServiceDateUtc(archived.departure.0.to_native()),
        }
    }

    /// Returns the days on which a trip departs from its first stop.
    #[inline]
    pub fn trip_service_days(&self, trip: BundleTripIndex) -> ServiceDayBitset {
        let archived = self.archived();
        let calendar = archived.trips[trip.as_usize()].service_days.to_native();
        archived.distinct_service_days[calendar.as_usize()].to_native()
    }

    /// Returns a trip's train category, number and agency.
    pub fn trip_train_label(&self, trip: BundleTripIndex) -> (&str, &str, &str) {
        let archived = self.archived();
        let label_index = archived.trips[trip.as_usize()].train_label.to_native();
        let label = &archived.train_labels[label_index.as_usize()];
        (
            self.pooled(label.category),
            self.pooled(label.number),
            self.pooled(label.agency),
        )
    }

    /// Returns the category, number and agency of every train label, in label order.
    pub fn train_labels(&self) -> impl Iterator<Item = (&str, &str, &str)> + '_ {
        self.archived().train_labels.iter().map(|label| {
            (
                self.pooled(label.category),
                self.pooled(label.number),
                self.pooled(label.agency),
            )
        })
    }

    /// Returns, for every train label in label order, the last day of the validity window on which
    /// a trip with that label departs from its first stop. The entry is `None` when no trip with
    /// the label runs on any day.
    pub fn last_departure_day_by_train_label(&self) -> Vec<Option<u16>> {
        let archived = self.archived();
        let last_day_by_calendar: Vec<Option<u16>> = archived
            .distinct_service_days
            .iter()
            .map(|days| days.to_native().last_day())
            .collect();
        let mut last_days = vec![None; archived.train_labels.len()];
        for trip in archived.trips.iter() {
            let calendar = trip.service_days.to_native().as_usize();
            let label = trip.train_label.to_native().as_usize();
            last_days[label] = last_days[label].max(last_day_by_calendar[calendar]);
        }
        last_days
    }

    /// Returns the page that shows the layout of a trip's train, if the bundle has one.
    pub fn trip_layout_page(&self, trip: BundleTripIndex) -> Option<LayoutPage<'_>> {
        let archived = self.archived();
        let label_index = archived.trips[trip.as_usize()].train_label.to_native();
        let page_index = archived.train_labels[label_index.as_usize()]
            .layout_page
            .as_ref()?
            .to_native();
        // The index comes from the builder, but the archive check does not compare it with the
        // length of the list, so a damaged bundle gets no page instead of a panic.
        let page = archived.layout_pages.get(page_index.as_usize())?;
        Some(LayoutPage {
            kind: match page.kind {
                ArchivedLayoutPageKind::Train => LayoutPageKind::Train,
                ArchivedLayoutPageKind::ApproximateTrain => LayoutPageKind::ApproximateTrain,
                ArchivedLayoutPageKind::CategoryList => LayoutPageKind::CategoryList,
                ArchivedLayoutPageKind::OperatorList => LayoutPageKind::OperatorList,
            },
            operator: self.pooled(page.operator),
            category: self.pooled(page.category),
            number: self.pooled(page.number),
            year: page.year.to_native(),
        })
    }

    /// Returns whether a trip's train has couchettes or sleepers.
    pub fn trip_offers_berths(&self, trip: BundleTripIndex) -> bool {
        let archived = self.archived();
        let label_index = archived.trips[trip.as_usize()].train_label.to_native();
        archived.train_labels[label_index.as_usize()].offers_berths
    }

    /// Returns the feeds that went into the bundle as name, license and URL.
    pub fn feed_attributions(&self) -> impl Iterator<Item = (&str, &str, &str)> + '_ {
        self.archived()
            .feed_attributions
            .iter()
            .map(|attribution: &ArchivedFeedAttribution| {
                (
                    self.pooled(attribution.feed_name),
                    self.pooled(attribution.license),
                    self.pooled(attribution.url),
                )
            })
    }
}

/// Returns row `station` of compressed sparse rows, or an empty slice for an unknown station.
#[inline]
fn row_of<'a, T>(
    row_offsets: &'a rkyv::vec::ArchivedVec<rkyv::rend::u32_le>,
    values: &'a rkyv::vec::ArchivedVec<T>,
    station: BundleStationIndex,
) -> &'a [T] {
    let row = station.as_usize();
    match (row_offsets.get(row), row_offsets.get(row + 1)) {
        (Some(start), Some(end)) => values
            .get(start.to_native() as usize..end.to_native() as usize)
            .unwrap_or_default(),
        _ => &[],
    }
}
