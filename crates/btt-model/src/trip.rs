//! Trips, their preferences and their stored form.

use std::num::{NonZeroU8, NonZeroU32};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use smol_str::SmolStr;
use specta::Type;
use uuid::Uuid;

use crate::bounded::ValueOutOfRangeError;
use crate::bounded::bounded_integer;
use crate::coordinates::GeoCoordinates;
use crate::itinerary::{Itinerary, SameDayStayRange, StationSearchRadius, TripPlaceId};
use crate::plan::JourneyChoice;
use crate::time::{DailyTimeWindow, DurationMinutes, LocalTimeOfDay, TravelDayWindows};

/// Identifies a trip. A UUID v7, so trips imported from another machine never collide.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Type,
)]
#[serde(transparent)]
pub struct TripId(pub Uuid);

impl TripId {
    /// Creates a new time-ordered trip ID.
    pub fn new_v7() -> Self {
        Self(Uuid::now_v7())
    }
}

/// Identifies a traveller profile, which lives outside any single trip.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Type,
)]
#[serde(transparent)]
pub struct TravellerId(pub Uuid);

/// How the planner treats trains the Interrail pass does not cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum InterrailPassPolicy {
    /// Never board trains the pass does not cover.
    PassOnly,
    /// Allow trains the pass does not cover, with a penalty.
    PreferPass,
    /// Treat all trains the same.
    Ignore,
}

/// Travel class for passes and fares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum TravelClass {
    /// First class.
    First,
    /// Second class.
    Second,
}

bounded_integer! {
    /// Trade-off between travel time and transfers. 0 favors the shortest travel time, 100 the
    /// fewest transfers.
    TransferAversion { percent: u8 } in 0, 100
}

/// The kind of single ticket the optimizer prices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum FareType {
    /// Full flexibility, valid on any train of the day.
    Flexible,
    /// The cheaper ticket that ties the traveller to one train.
    Saver,
}

/// How tickets for the trip are chosen. The options are independent switches.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct TicketPreferences {
    /// Whether the optimizer compares an Interrail pass with single tickets.
    pub consider_passes: bool,
    /// Whether children travelling with an adult on a pass are priced as free.
    pub consider_families: bool,
    /// The kind of single ticket to price.
    pub fare_type: FareType,
    /// Whether optional seat reservations are bought, so they count in the price.
    pub reserve_optional_seats: bool,
    /// Whether prices are looked up on the booking sites of operators. These sites are not
    /// meant to be read by programs, so the lookup is a choice.
    pub live_prices: bool,
    /// The day tickets are assumed to be bought, which decides the booking window. `None` means
    /// today.
    pub assumed_purchase_date: Option<NaiveDate>,
}

impl Default for TicketPreferences {
    fn default() -> Self {
        Self {
            consider_passes: true,
            consider_families: true,
            fare_type: FareType::Flexible,
            reserve_optional_seats: false,
            live_prices: true,
            assumed_purchase_date: None,
        }
    }
}

bounded_integer! {
    /// How many minutes of travel time one euro is worth in scoring, in hundredths of a minute.
    PriceWeight { hundredths_of_minute_per_euro: u16 } in 0, u16::MAX
}

impl PriceWeight {
    /// Prices count for nothing, so the planner picks by time alone.
    pub const TIME_FIRST: Self = Self {
        hundredths_of_minute_per_euro: 0,
    };
    /// An hour of travel is worth 60 euro, the balanced weight.
    pub const BALANCED: Self = Self {
        hundredths_of_minute_per_euro: 100,
    };
    /// An hour of travel is worth 20 euro, the frugal weight.
    pub const FRUGAL: Self = Self {
        hundredths_of_minute_per_euro: 300,
    };
    /// An hour of travel is worth 7.50 euro, the cheapest weight.
    pub const CHEAPEST: Self = Self {
        hundredths_of_minute_per_euro: 800,
    };

    /// Returns the weight of a value of an hour in euro, which the preference stores as the
    /// minutes per euro that follow from it. `TIME_FIRST` is the weight of no value at all,
    /// because zero minutes per euro is not a number of euro.
    ///
    /// # Errors
    ///
    /// Returns an error when `euro_per_hour` is negative, not a number, or so small that the
    /// weight no longer fits its type.
    pub fn of_euro_per_hour(euro_per_hour: f64) -> Result<Self, ValueOutOfRangeError> {
        if !euro_per_hour.is_finite() || euro_per_hour < 0.0 {
            return Err(ValueOutOfRangeError {
                type_name: "PriceWeight",
                value: 0,
                minimum: i64::from(Self::MINIMUM),
                maximum: i64::from(Self::MAXIMUM),
            });
        }
        // One euro is worth sixty minutes divided by the value of an hour. A very small value
        // saturates the cast, so the range check below rejects it.
        #[expect(clippy::cast_possible_truncation)]
        let hundredths = ((60.0 / euro_per_hour.max(f64::EPSILON)) * 100.0).round() as i64;
        Self::new(u16::try_from(hundredths).map_err(|_| ValueOutOfRangeError {
            type_name: "PriceWeight",
            value: hundredths,
            minimum: i64::from(Self::MINIMUM),
            maximum: i64::from(Self::MAXIMUM),
        })?)
    }

    /// Returns the value of an hour in euro the weight stands for, or `None` when prices
    /// count for nothing.
    #[must_use]
    pub fn euro_per_hour(self) -> Option<f64> {
        (self.hundredths_of_minute_per_euro > 0)
            .then(|| 6000.0 / f64::from(self.hundredths_of_minute_per_euro))
    }
}

/// The number of travel days a journey uses on a pass, from 1 to 3. A journey over midnight
/// uses the day it leaves on and the morning after, and a long one takes a third day.
///
/// The field stays private, so the only ways to build a value are `new` and `TryFrom`.
/// Deserialization goes through `new` as well, so a bad file cannot bypass the range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, specta::Type)]
#[specta(type = u8)]
pub struct PassDays {
    days: NonZeroU8,
}

impl PassDays {
    /// Smallest allowed number of days.
    pub const MINIMUM: u8 = 1;
    /// Largest allowed number of days.
    pub const MAXIMUM: u8 = 3;

    /// Checks the range and wraps the value.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is outside `MINIMUM..=MAXIMUM`.
    #[allow(unused_comparisons, clippy::cast_lossless)]
    pub const fn new(days: u8) -> Result<Self, ValueOutOfRangeError> {
        if days < Self::MINIMUM || days > Self::MAXIMUM {
            return Err(ValueOutOfRangeError {
                type_name: "PassDays",
                value: days as i64,
                minimum: Self::MINIMUM as i64,
                maximum: Self::MAXIMUM as i64,
            });
        }
        // The range check above keeps `days` away from zero.
        Ok(Self {
            days: match NonZeroU8::new(days) {
                Some(days) => days,
                None => std::unreachable!(),
            },
        })
    }

    /// Returns the number of travel days.
    #[must_use]
    pub const fn days(self) -> u8 {
        self.days.get()
    }
}

impl TryFrom<u8> for PassDays {
    type Error = ValueOutOfRangeError;

    fn try_from(days: u8) -> Result<Self, Self::Error> {
        Self::new(days)
    }
}

impl From<PassDays> for u8 {
    fn from(days: PassDays) -> Self {
        days.days.get()
    }
}

impl serde::Serialize for PassDays {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&self.days.get(), serializer)
    }
}

impl<'de> serde::Deserialize<'de> for PassDays {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let days = u8::deserialize(deserializer)?;
        Self::new(days).map_err(serde::de::Error::custom)
    }
}

/// Trip-wide planning preferences. Places and legs can override some of them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct TripPreferences {
    /// Windows for places without their own override.
    pub default_windows: TravelDayWindows,
    /// Trade-off between travel time and transfers.
    pub transfer_aversion: TransferAversion,
    /// Stay range for same-day places without their own override.
    pub default_same_day_stay: SameDayStayRange,
    /// How the planner treats trains the pass does not cover.
    pub pass_policy: InterrailPassPolicy,
    /// Shortest time allowed between arriving on one train and departing on the next.
    pub minimum_transfer_time: DurationMinutes,
    /// Travellers on this trip, referring to profiles from the traveller screen.
    #[specta(type = Vec<TravellerId>)]
    pub traveller_ids: SmallVec<[TravellerId; 4]>,
    /// Class for passes and fares.
    pub travel_class: TravelClass,
    /// Weight of price against travel time in scoring.
    pub price_weight: PriceWeight,
    /// How tickets are chosen.
    pub ticket_preferences: TicketPreferences,
}

impl Default for TripPreferences {
    fn default() -> Self {
        let time = |hour: u16| {
            LocalTimeOfDay::from_hour_and_minute(hour, 0).unwrap_or(LocalTimeOfDay::MIDNIGHT)
        };
        Self {
            default_windows: TravelDayWindows {
                departure: DailyTimeWindow::from_start_and_end(time(7), time(20)),
                arrival: DailyTimeWindow::from_start_and_end(time(7), time(23)),
            },
            transfer_aversion: TransferAversion { percent: 50 },
            default_same_day_stay: SameDayStayRange {
                minimum: DurationMinutes::from_hours_and_minutes(2, 0),
                maximum_beyond_minimum: None,
            },
            pass_policy: InterrailPassPolicy::PreferPass,
            minimum_transfer_time: DurationMinutes(10),
            traveller_ids: SmallVec::new(),
            travel_class: TravelClass::Second,
            price_weight: PriceWeight {
                hundredths_of_minute_per_euro: 0,
            },
            ticket_preferences: TicketPreferences::default(),
        }
    }
}

/// A planned multi-stop train trip.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct Trip {
    /// Identifies the trip across machines.
    pub id: TripId,
    /// Name shown in the trip list.
    pub name: SmolStr,
    /// Date of the first leg's departure, before any nights at via places.
    pub start_date: NaiveDate,
    /// The ordered places of the trip.
    pub itinerary: Itinerary,
    /// Trip-wide planning preferences.
    pub preferences: TripPreferences,
    /// Next free `TripPlaceId` in this trip. IDs of removed places are never reused.
    pub next_place_id: NonZeroU32,
    /// Journeys the traveller picked instead of the planner's pick, at most one per leg.
    #[serde(default)]
    pub chosen_journeys: Vec<ChosenJourney>,
    /// Legs whose minimum transfer time differs from the trip-wide one, at most one per leg.
    #[serde(default)]
    pub leg_transfer_overrides: Vec<LegTransferOverride>,
}

/// A minimum transfer time for the leg between two places, which replaces the trip-wide
/// `minimum_transfer_time` for that leg. A reorder that separates the two places drops it,
/// because that leg no longer exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct LegTransferOverride {
    /// Place the leg leaves from.
    pub departure_place_id: TripPlaceId,
    /// Place the leg arrives at.
    pub arrival_place_id: TripPlaceId,
    /// Shortest time allowed between two trains on this leg.
    pub minimum_transfer_time: DurationMinutes,
}

/// A journey the traveller picked for the leg between two places. A reorder that separates the
/// two places drops it, because that leg no longer exists.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct ChosenJourney {
    /// Place the leg leaves from.
    pub departure_place_id: TripPlaceId,
    /// Place the leg arrives at.
    pub arrival_place_id: TripPlaceId,
    /// The picked journey.
    pub choice: JourneyChoice,
}

impl Trip {
    /// Creates an empty trip with default preferences.
    pub fn new(name: SmolStr, start_date: NaiveDate) -> Self {
        Self {
            id: TripId::new_v7(),
            name,
            start_date,
            itinerary: Itinerary::Empty,
            preferences: TripPreferences::default(),
            next_place_id: NonZeroU32::MIN,
            chosen_journeys: Vec::new(),
            leg_transfer_overrides: Vec::new(),
        }
    }

    /// Returns the minimum transfer time that applies to the leg between two places, which is
    /// the override of that leg when it has one and the trip-wide time otherwise.
    pub fn minimum_transfer_time_of_leg(
        &self,
        departure_place_id: TripPlaceId,
        arrival_place_id: TripPlaceId,
    ) -> DurationMinutes {
        self.leg_transfer_overrides
            .iter()
            .find(|leg| {
                leg.departure_place_id == departure_place_id
                    && leg.arrival_place_id == arrival_place_id
            })
            .map_or(self.preferences.minimum_transfer_time, |leg| {
                leg.minimum_transfer_time
            })
    }

    /// Returns the places of the trip in travel order, for the small map of the trip list.
    pub fn summary_route(&self) -> Vec<TripSummaryStop> {
        let origin = self
            .itinerary
            .trip_places()
            .next()
            .map(|place| TripSummaryStop {
                coordinates: place.coordinates,
                station_search_radius: place.station_search_radius,
                arrives_by_night: false,
            });
        let arrivals = self.itinerary.legs().map(|leg| TripSummaryStop {
            coordinates: leg.arrival_place.coordinates,
            station_search_radius: leg.arrival_place.station_search_radius,
            arrives_by_night: leg.travel_mode.is_overnight(),
        });
        origin.into_iter().chain(arrivals).collect()
    }
}

/// Schema version that `StoredTrip` documents written by this build carry.
pub const CURRENT_TRIP_SCHEMA_VERSION: u16 = 5;

/// A trip as it is stored and exported, with the schema version of its document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct StoredTrip {
    /// Version of the document layout, used to pick migrations on load.
    pub schema_version: u16,
    /// The trip itself.
    pub trip: Trip,
}

/// Short description of a trip for the trip list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TripSummary {
    /// Identifies the trip.
    pub trip_id: TripId,
    /// Name of the trip.
    pub name: SmolStr,
    /// Date of the first departure.
    pub start_date: NaiveDate,
    /// Number of places in the itinerary.
    pub place_count: u16,
    /// The places in travel order, which the trip list draws as a small map. Empty when the
    /// stored trip cannot be read.
    pub route: Vec<TripSummaryStop>,
}

/// A place of the route of a trip, as the small map of the trip list needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TripSummaryStop {
    /// Where the place is.
    pub coordinates: GeoCoordinates,
    /// How far around the place the planner looks for stations. Two places that lie within
    /// each other's radius are the same place on the map.
    pub station_search_radius: StationSearchRadius,
    /// Whether the leg that arrives here includes a night train. False for the origin.
    pub arrives_by_night: bool,
}

#[cfg(test)]
mod summary_route_tests {
    use super::*;
    use crate::itinerary::tests::{overnight, place};
    use crate::itinerary::{ItineraryRoute, LegTravelMode, PlaceStay, ViaPlace};

    fn route_trip() -> Trip {
        let mut trip = Trip::new("Test".into(), NaiveDate::from_ymd_opt(2026, 10, 1).unwrap());
        trip.itinerary = Itinerary::Route(ItineraryRoute {
            origin: place(1, "Amsterdam", 52.3789, 4.9003),
            via_places: vec![ViaPlace {
                arriving_leg_travel_mode: LegTravelMode::Daytime,
                place: place(2, "Vienna", 48.1852, 16.3761),
                stay: PlaceStay::Nights(NonZeroU8::new(2).unwrap()),
            }],
            final_leg_travel_mode: overnight(),
            destination: place(3, "Rome", 41.901, 12.5007),
        });
        trip
    }

    #[test]
    fn the_route_lists_every_place_and_marks_the_night_legs() {
        let route = route_trip().summary_route();
        assert_eq!(route.len(), 3);
        assert_eq!(
            route
                .iter()
                .map(|stop| stop.arrives_by_night)
                .collect::<Vec<_>>(),
            [false, false, true]
        );
        assert_eq!(
            route[0].coordinates,
            GeoCoordinates::from_degrees(52.3789, 4.9003).unwrap()
        );
    }

    #[test]
    fn a_trip_with_one_place_has_a_route_of_one_stop() {
        let mut trip = route_trip();
        trip.itinerary = Itinerary::OriginOnly(place(1, "Amsterdam", 52.3789, 4.9003));
        assert_eq!(trip.summary_route().len(), 1);
        trip.itinerary = Itinerary::Empty;
        assert_eq!(trip.summary_route(), Vec::new());
    }
}

#[cfg(test)]
mod price_weight_tests {
    use super::*;

    #[test]
    fn the_presets_hold_the_weight_of_an_hour() {
        assert_eq!(PriceWeight::TIME_FIRST.hundredths_of_minute_per_euro(), 0);
        assert_eq!(PriceWeight::BALANCED.hundredths_of_minute_per_euro(), 100);
        assert_eq!(PriceWeight::FRUGAL.hundredths_of_minute_per_euro(), 300);
        assert_eq!(PriceWeight::CHEAPEST.hundredths_of_minute_per_euro(), 800);
    }

    #[test]
    fn the_weight_follows_the_value_of_an_hour() {
        assert_eq!(
            PriceWeight::of_euro_per_hour(60.0).unwrap(),
            PriceWeight::BALANCED
        );
        assert_eq!(
            PriceWeight::of_euro_per_hour(20.0).unwrap(),
            PriceWeight::FRUGAL
        );
        assert_eq!(
            PriceWeight::of_euro_per_hour(7.5).unwrap(),
            PriceWeight::CHEAPEST
        );
        assert!(PriceWeight::of_euro_per_hour(-1.0).is_err());
        assert!(PriceWeight::of_euro_per_hour(f64::NAN).is_err());
        assert!(PriceWeight::of_euro_per_hour(0.001).is_err());
        assert_eq!(
            PriceWeight::BALANCED.euro_per_hour(),
            Some(60.0),
            "the value of an hour follows from the weight"
        );
        assert_eq!(PriceWeight::TIME_FIRST.euro_per_hour(), None);
    }
}
