//! Planned journeys and the events that stream them to the frontend.

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use smol_str::SmolStr;
use specta::Type;

use crate::coordinates::GeoCoordinates;
use crate::itinerary::TripPlaceId;
use crate::time::{DurationMinutes, UnixTimeMinutes};
use crate::trip::Trip;

/// Points into `LegPlan::station_table`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(transparent)]
pub struct LegPlanStationIndex(pub u8);

/// A station used by the journeys of one leg plan.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct LegPlanStation {
    /// Name shown in the plan.
    pub name: SmolStr,
    /// Location of the station, for the map.
    pub coordinates: GeoCoordinates,
    /// IANA time zone of the station, such as "Europe/Vienna", for showing local train times.
    pub time_zone: SmolStr,
}

/// How a train is shown to the traveller, such as "ICE 27".
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct TrainLabel {
    /// Product or category, such as "ICE", "RJX" or "NJ".
    pub category: SmolStr,
    /// Train number within the category.
    pub number: SmolStr,
}

/// One train ridden from boarding to alighting.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct TrainRide {
    /// Station where the traveller boards.
    pub boarding_station: LegPlanStationIndex,
    /// Departure from the boarding station.
    pub departure_time: UnixTimeMinutes,
    /// Station where the traveller leaves the train.
    pub alighting_station: LegPlanStationIndex,
    /// Arrival at the alighting station.
    pub arrival_time: UnixTimeMinutes,
    /// How the train is shown to the traveller.
    pub train_label: TrainLabel,
    /// Stations the train passes between boarding and alighting, in order, so the map can
    /// draw its line along them.
    #[specta(type = Vec<GeoCoordinates>)]
    pub intermediate_station_coordinates: SmallVec<[GeoCoordinates; 6]>,
}

/// A transfer from the previous ride, followed by the next ride.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct TransferAndRide {
    /// Time from arriving on the previous ride to departing on this one.
    pub transfer_duration: DurationMinutes,
    /// The next ride.
    pub ride: TrainRide,
}

/// How the planner assumes a traveller covers a way off the train.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum WayToStationMode {
    /// On foot, for ways shorter than a kilometer.
    Walk,
    /// By bus, tram or metro, for longer ways.
    LocalTransport,
}

/// A way off the train, between a place and a station or between two stations. Its duration
/// is an estimate from the straight-line distance, not a planned connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct StationWay {
    /// Estimated time the way takes.
    pub duration: DurationMinutes,
    /// Straight-line distance of the way.
    pub distance_meters: u32,
    /// How the estimate assumes the traveller goes.
    pub mode: WayToStationMode,
}

/// A walk or local transit trip from the last alighting station to another station near the
/// arrival place, for when no train serves that station in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct ArrivalFootpath {
    /// Station the footpath ends at.
    pub to_station: LegPlanStationIndex,
    /// The way from the last alighting station to `to_station`.
    pub way: StationWay,
}

/// A door-to-door journey for one leg.
///
/// A journey always has at least one train, and every later train has exactly one transfer
/// before it, so two transfers in a row cannot be represented.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct Journey {
    /// The way from the departure place to the first boarding station.
    pub access: StationWay,
    /// The first train.
    pub first_ride: TrainRide,
    /// Later trains, each with the transfer before it.
    #[specta(type = Vec<TransferAndRide>)]
    pub later_rides: SmallVec<[TransferAndRide; 3]>,
    /// The way from the last alighting station to another station, when the journey ends with
    /// one.
    pub arrival_footpath: Option<ArrivalFootpath>,
    /// The way to the arrival place from the station the journey ends at, which is the end of
    /// `arrival_footpath` or else the last alighting station.
    pub egress: StationWay,
}

impl Journey {
    /// Returns every ride in travel order.
    pub fn rides(&self) -> impl Iterator<Item = &TrainRide> {
        std::iter::once(&self.first_ride).chain(self.later_rides.iter().map(|later| &later.ride))
    }

    /// Returns the ride that ends the journey.
    pub fn last_ride(&self) -> &TrainRide {
        self.later_rides
            .last()
            .map_or(&self.first_ride, |later| &later.ride)
    }
}

/// The planned journey for one leg and its alternatives.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct LegPlan {
    /// The journey the planner picked.
    pub chosen_journey: Journey,
    /// Other good journeys the traveller can switch to.
    #[specta(type = Vec<Journey>)]
    pub alternative_journeys: SmallVec<[Journey; 4]>,
    /// Stations used by the journeys in this leg.
    #[specta(type = Vec<LegPlanStation>)]
    pub station_table: SmallVec<[LegPlanStation; 8]>,
}

/// Why the planner found no journey for a leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum NoJourneyReason {
    /// No station with service lies within the departure place's search radius.
    NoStationNearDeparturePlace,
    /// No station with service lies within the arrival place's search radius.
    NoStationNearArrivalPlace,
    /// Stations exist, but no connection fits the time windows.
    NoConnectionInWindow,
    /// The planning backend failed, for example because a network request failed.
    BackendUnavailable,
    /// The travel date lies outside the days the timetable covers.
    TravelDateOutsideTimetable,
}

/// Result of planning one leg.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum LegPlanningResult {
    /// The planner found at least one journey.
    // Boxed, because a leg plan keeps its journeys inline and is far larger than the other
    // variant.
    Found(Box<LegPlan>),
    /// The planner found no journey.
    NoJourneyFound(NoJourneyReason),
}

/// Counts planning runs within one session. A newer run replaces the results of older ones.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Type,
)]
#[serde(transparent)]
pub struct PlanningRunId(pub u32);

/// Messages from a planning session to the frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum PlanningSessionEvent {
    /// The trip changed outside the frontend, for example when reverse geocoding named a place.
    TripChanged(Box<Trip>),
    /// A run started. Legs it replans are pending until their `LegPlanned` event.
    PlanningStarted {
        /// Identifies the run.
        run_id: PlanningRunId,
        /// Places whose arriving legs this run replans.
        replanned_arrival_place_ids: Vec<TripPlaceId>,
    },
    /// One leg of a run finished.
    LegPlanned {
        /// Identifies the run.
        run_id: PlanningRunId,
        /// Place at which the planned leg arrives.
        arrival_place_id: TripPlaceId,
        /// The planned journeys, or why there are none.
        result: LegPlanningResult,
    },
    /// Every leg of a run finished.
    PlanningFinished {
        /// Identifies the run.
        run_id: PlanningRunId,
    },
}
