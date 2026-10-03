//! Trips, their preferences and their stored form.

use std::num::NonZeroU32;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use smol_str::SmolStr;
use specta::Type;
use uuid::Uuid;

use crate::bounded::bounded_integer;
use crate::itinerary::{Itinerary, SameDayStayRange, TripPlaceId};
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

/// How tickets for the trip are chosen.
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
            assumed_purchase_date: None,
        }
    }
}

bounded_integer! {
    /// How many minutes of travel time one euro is worth in scoring, in hundredths of a minute.
    PriceWeight { hundredths_of_minute_per_euro: u16 } in 0, u16::MAX
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
        }
    }
}

/// Schema version that `StoredTrip` documents written by this build carry.
pub const CURRENT_TRIP_SCHEMA_VERSION: u16 = 4;

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
}
