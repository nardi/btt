//! The ordered places of a trip and the legs between them.

use std::num::{NonZeroU8, NonZeroU32};

use chrono::{Days, NaiveDate};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use smol_str::SmolStr;
use specta::Type;

use crate::bounded::bounded_integer;
use crate::coordinates::GeoCoordinates;
use crate::time::{DailyTimeWindow, DurationMinutes, TravelDayWindows};

/// Identifies a place within one trip. Unique within that trip only.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Type,
)]
#[serde(transparent)]
pub struct TripPlaceId(pub NonZeroU32);

bounded_integer! {
    /// Largest distance from a place to a station the planner may use, from 100 m to 50 km.
    StationSearchRadius { meters: u16 } in 100, 50_000
}

impl StationSearchRadius {
    /// Radius for new places, which covers the stations of most city centers.
    pub const DEFAULT: Self = Self { meters: 5_000 };
}

/// A place the traveller visits, as chosen on the map or through search.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct TripPlace {
    /// Identifies the place within its trip.
    pub id: TripPlaceId,
    /// Name shown in the itinerary.
    pub name: SmolStr,
    /// Point the traveller travels to or from.
    pub coordinates: GeoCoordinates,
    /// Largest distance to a station the planner may use for this place.
    pub station_search_radius: StationSearchRadius,
    /// Replaces the trip's default windows for legs that leave or reach this place.
    pub window_override: Option<TravelDayWindows>,
}

/// Time spent at a same-day stop, measured from train arrival to train departure.
///
/// The maximum is stored as a length beyond the minimum, so a maximum below the minimum cannot
/// be represented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct SameDayStayRange {
    /// Shortest acceptable stay.
    pub minimum: DurationMinutes,
    /// How much longer than `minimum` the stay may be. `None` means no upper bound.
    pub maximum_beyond_minimum: Option<DurationMinutes>,
}

impl SameDayStayRange {
    /// Builds a range from a minimum and an optional maximum.
    ///
    /// Returns `None` when the maximum is below the minimum.
    pub fn from_minimum_and_maximum(
        minimum: DurationMinutes,
        maximum: Option<DurationMinutes>,
    ) -> Option<Self> {
        let maximum_beyond_minimum = match maximum {
            Some(maximum) => Some(DurationMinutes(
                maximum.minutes().checked_sub(minimum.minutes())?,
            )),
            None => None,
        };
        Some(Self {
            minimum,
            maximum_beyond_minimum,
        })
    }

    /// Returns the longest acceptable stay, or `None` when there is no upper bound.
    pub fn maximum(self) -> Option<DurationMinutes> {
        self.maximum_beyond_minimum
            .map(|extra| self.minimum.saturating_add(extra))
    }
}

/// How long the traveller stays at a via place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum PlaceStay {
    /// The traveller sleeps here this many nights.
    Nights(NonZeroU8),
    /// The traveller leaves on the day of arrival.
    SameDay {
        /// Replaces the trip's default same-day stay range for this place.
        stay_range_override: Option<SameDayStayRange>,
        /// Turns extra stay time into a small bonus in scoring.
        prefers_longer_stay: bool,
    },
}

impl PlaceStay {
    /// Stay for a place that becomes a via place without an explicit choice.
    pub const DEFAULT: Self = Self::Nights(NonZeroU8::MIN);

    /// Returns the number of nights, which is zero for a same-day stay.
    pub fn nights(self) -> u8 {
        match self {
            Self::Nights(nights) => nights.get(),
            Self::SameDay { .. } => 0,
        }
    }
}

/// Kind of berth on a night train.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum BerthType {
    /// A bunk in a shared compartment of four or six.
    Couchette,
    /// A bed in a compartment of one to three.
    Sleeper,
}

/// Berth types the traveller accepts on an overnight leg. At least one is always accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum AcceptedBerths {
    /// Only this berth type.
    Only(BerthType),
    /// Both berth types, optionally with a preference that scoring rewards.
    Either {
        /// Berth type that scores better when both are available.
        preferred: Option<BerthType>,
    },
}

/// Options for a leg that includes one night train.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct OvernightLegOptions {
    /// Local-time window in which the night train is boarded.
    pub boarding_window: DailyTimeWindow,
    /// Local-time window in which the night train is left.
    pub alighting_window: DailyTimeWindow,
    /// Berth types the traveller accepts.
    pub accepted_berths: AcceptedBerths,
    /// Shortest acceptable time on the night train.
    pub minimum_time_on_night_train: Option<DurationMinutes>,
}

/// Whether a leg happens during the day or includes a night train.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum LegTravelMode {
    /// Any trains during the day.
    Daytime,
    /// Exactly one night train with a berth, plus optional trains before and after it.
    Overnight(OvernightLegOptions),
}

impl LegTravelMode {
    /// Returns whether the leg includes a night train, which moves every later date by a day.
    pub const fn is_overnight(&self) -> bool {
        matches!(self, Self::Overnight(_))
    }
}

/// A place between the origin and the destination, with the leg that arrives there.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct ViaPlace {
    /// Travel mode of the leg that ends at this place.
    pub arriving_leg_travel_mode: LegTravelMode,
    /// The place itself.
    pub place: TripPlace,
    /// How long the traveller stays here.
    pub stay: PlaceStay,
}

/// An itinerary with at least an origin and a destination.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct ItineraryRoute {
    /// Where the trip starts.
    pub origin: TripPlace,
    /// Places between origin and destination, in travel order.
    pub via_places: Vec<ViaPlace>,
    /// Travel mode of the leg from the last via place, or from the origin, to the destination.
    pub final_leg_travel_mode: LegTravelMode,
    /// Where the trip ends.
    pub destination: TripPlace,
}

/// The ordered places of a trip.
///
/// Only via places have a stay, because nights at the origin or the destination have no meaning
/// for planning.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum Itinerary {
    /// No places yet.
    Empty,
    /// Only a starting place, so there is nothing to plan yet.
    OriginOnly(TripPlace),
    /// An origin, a destination and any places between them.
    Route(ItineraryRoute),
}

/// One leg of an itinerary, borrowed from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItineraryLeg<'a> {
    /// Place the leg leaves from.
    pub departure_place: &'a TripPlace,
    /// Place the leg arrives at. Its ID identifies the leg.
    pub arrival_place: &'a TripPlace,
    /// Whether the leg includes a night train.
    pub travel_mode: &'a LegTravelMode,
    /// Stay at the departure place, or `None` for the leg that leaves the origin.
    pub stay_at_departure_place: Option<&'a PlaceStay>,
}

impl Itinerary {
    /// Returns every place in travel order.
    pub fn trip_places(&self) -> impl Iterator<Item = &TripPlace> {
        let (origin, via_places, destination) = self.parts();
        origin
            .into_iter()
            .chain(via_places.iter().map(|via_place| &via_place.place))
            .chain(destination)
    }

    /// Returns the number of places.
    pub fn place_count(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::OriginOnly(_) => 1,
            Self::Route(route) => route.via_places.len() + 2,
        }
    }

    /// Returns the legs in travel order. There is one leg fewer than there are places.
    pub fn legs(&self) -> impl Iterator<Item = ItineraryLeg<'_>> {
        let route = match self {
            Self::Route(route) => Some(route),
            Self::Empty | Self::OriginOnly(_) => None,
        };
        route.into_iter().flat_map(|route| {
            // Each via place ends the leg before it and starts the leg after it.
            let departures = std::iter::once((&route.origin, None)).chain(
                route
                    .via_places
                    .iter()
                    .map(|via_place| (&via_place.place, Some(&via_place.stay))),
            );
            let arrivals = route
                .via_places
                .iter()
                .map(|via_place| (&via_place.place, &via_place.arriving_leg_travel_mode))
                .chain(std::iter::once((
                    &route.destination,
                    &route.final_leg_travel_mode,
                )));
            departures.zip(arrivals).map(
                |((departure_place, stay_at_departure_place), (arrival_place, travel_mode))| {
                    ItineraryLeg {
                        departure_place,
                        arrival_place,
                        travel_mode,
                        stay_at_departure_place,
                    }
                },
            )
        })
    }

    /// Returns the departure date of each leg, in travel order.
    ///
    /// Each via place adds its nights, and each overnight leg adds one day, because it arrives
    /// the morning after it departs. The night on the train does not count as a night at either
    /// place.
    pub fn leg_departure_dates(&self, start_date: NaiveDate) -> SmallVec<[NaiveDate; 16]> {
        let mut departure_date = start_date;
        let mut dates = SmallVec::new();
        for leg in self.legs() {
            if let Some(stay) = leg.stay_at_departure_place {
                departure_date = add_days(departure_date, u64::from(stay.nights()));
            }
            dates.push(departure_date);
            if leg.travel_mode.is_overnight() {
                departure_date = add_days(departure_date, 1);
            }
        }
        dates
    }

    /// Returns the place with this ID, if the itinerary has one.
    pub fn place(&self, place_id: TripPlaceId) -> Option<&TripPlace> {
        self.trip_places().find(|place| place.id == place_id)
    }

    /// Splits the itinerary into its optional origin, via places and optional destination.
    fn parts(&self) -> (Option<&TripPlace>, &[ViaPlace], Option<&TripPlace>) {
        match self {
            Self::Empty => (None, &[], None),
            Self::OriginOnly(origin) => (Some(origin), &[], None),
            Self::Route(route) => (
                Some(&route.origin),
                &route.via_places,
                Some(&route.destination),
            ),
        }
    }
}

/// Adds days to a date, saturating at the largest date `chrono` supports.
fn add_days(date: NaiveDate, days: u64) -> NaiveDate {
    date.checked_add_days(Days::new(days))
        .unwrap_or(NaiveDate::MAX)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::time::LocalTimeOfDay;

    pub(crate) fn place(id: u32, name: &str, latitude: f64, longitude: f64) -> TripPlace {
        TripPlace {
            id: TripPlaceId(NonZeroU32::new(id).unwrap()),
            name: name.into(),
            coordinates: GeoCoordinates::from_degrees(latitude, longitude).unwrap(),
            station_search_radius: StationSearchRadius::DEFAULT,
            window_override: None,
        }
    }

    pub(crate) fn overnight() -> LegTravelMode {
        let time = |hour| LocalTimeOfDay::from_hour_and_minute(hour, 0).unwrap();
        LegTravelMode::Overnight(OvernightLegOptions {
            boarding_window: DailyTimeWindow::from_start_and_end(time(19), time(23)),
            alighting_window: DailyTimeWindow::from_start_and_end(time(6), time(10)),
            accepted_berths: AcceptedBerths::Either { preferred: None },
            minimum_time_on_night_train: None,
        })
    }

    fn nights(count: u8) -> PlaceStay {
        PlaceStay::Nights(NonZeroU8::new(count).unwrap())
    }

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    /// Amsterdam, two nights in Vienna, an overnight leg to Rome.
    fn amsterdam_vienna_rome() -> Itinerary {
        Itinerary::Route(ItineraryRoute {
            origin: place(1, "Amsterdam", 52.378_9, 4.900_3),
            via_places: vec![ViaPlace {
                arriving_leg_travel_mode: LegTravelMode::Daytime,
                place: place(2, "Vienna", 48.185_2, 16.376_1),
                stay: nights(2),
            }],
            final_leg_travel_mode: overnight(),
            destination: place(3, "Rome", 41.901_0, 12.500_7),
        })
    }

    #[test]
    fn empty_and_origin_only_itineraries_have_no_legs() {
        assert_eq!(Itinerary::Empty.legs().count(), 0);
        let origin_only = Itinerary::OriginOnly(place(1, "Amsterdam", 52.378_9, 4.900_3));
        assert_eq!(origin_only.legs().count(), 0);
        assert_eq!(origin_only.place_count(), 1);
    }

    #[test]
    fn legs_connect_adjacent_places() {
        let itinerary = amsterdam_vienna_rome();
        let names: Vec<_> = itinerary
            .legs()
            .map(|leg| {
                (
                    leg.departure_place.name.as_str(),
                    leg.arrival_place.name.as_str(),
                )
            })
            .collect();
        assert_eq!(names, [("Amsterdam", "Vienna"), ("Vienna", "Rome")]);
        assert_eq!(itinerary.place_count(), 3);
        assert_eq!(itinerary.trip_places().count(), 3);
    }

    #[test]
    fn overnight_leg_reaches_rome_on_third_morning_after_vienna() {
        let itinerary = amsterdam_vienna_rome();
        let start = date(2026, 10, 1);
        let dates = itinerary.leg_departure_dates(start);
        assert_eq!(dates.as_slice(), [start, date(2026, 10, 3)]);
        // The overnight leg departs on 3 October and arrives on the 4th, which is the third
        // morning after reaching Vienna on the 1st.
    }

    #[test]
    fn same_day_stay_keeps_the_date() {
        let mut itinerary = amsterdam_vienna_rome();
        if let Itinerary::Route(route) = &mut itinerary {
            route.via_places[0].stay = PlaceStay::SameDay {
                stay_range_override: None,
                prefers_longer_stay: false,
            };
            route.via_places[0].arriving_leg_travel_mode = overnight();
        }
        let start = date(2026, 10, 1);
        assert_eq!(
            itinerary.leg_departure_dates(start).as_slice(),
            [start, date(2026, 10, 2)]
        );
    }

    #[test]
    fn stay_range_maximum_cannot_be_below_minimum() {
        let two_hours = DurationMinutes::from_hours_and_minutes(2, 0);
        let five_hours = DurationMinutes::from_hours_and_minutes(5, 0);
        let range =
            SameDayStayRange::from_minimum_and_maximum(two_hours, Some(five_hours)).unwrap();
        assert_eq!(range.maximum(), Some(five_hours));
        assert!(SameDayStayRange::from_minimum_and_maximum(five_hours, Some(two_hours)).is_none());
    }
}
