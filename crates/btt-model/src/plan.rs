//! Planned journeys and the events that stream them to the frontend.

use chrono::NaiveDate;
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
    /// The operator as the app names it, such as `db-fernverkehr` or `regiojet`. Empty in plans
    /// stored without it.
    #[serde(default)]
    pub operator: SmolStr,
    /// The operating agency as the timetable names it, such as "DB Fernverkehr AG". Empty in
    /// plans stored without it.
    #[serde(default)]
    pub agency: SmolStr,
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
    /// Whether the train has couchettes or sleepers, whatever the leg's travel mode.
    pub offers_berths: bool,
    /// Stations the train stops at between boarding and alighting, in order.
    #[specta(type = Vec<GeoCoordinates>)]
    pub intermediate_station_coordinates: SmallVec<[GeoCoordinates; 6]>,
    /// Every point of the line between boarding and alighting, in order: the stops and the
    /// stations the train most likely passes without stopping. Empty in plans stored
    /// without it, in which case the stops describe the line.
    #[serde(default)]
    pub path_coordinates: Vec<GeoCoordinates>,
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
    /// Position in `rides()` of the train with berths the traveller sleeps on, when one covers
    /// at least half of the night of an overnight leg.
    #[serde(default)]
    pub sleeping_ride: Option<u8>,
    /// What the planner's cost of the journey is made of, for explaining the choice. `None`
    /// in plans stored without it and from backends that don't rank journeys.
    #[serde(default)]
    pub cost_parts: Option<JourneyCostParts>,
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

/// The parts of a journey's cost, each in minutes of train travel, so the difference between
/// two journeys splits into what causes it. The parts add up to `total_cost`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct JourneyCostParts {
    /// Minutes from leaving the departure place to reaching the arrival place, which the cost
    /// counts once.
    pub door_to_door_minutes: u32,
    /// Minutes of those spent on the ways to and from stations.
    pub way_to_station_minutes: u32,
    /// What the ways to and from stations cost on top of their duration.
    pub way_to_station_extra_cost: u32,
    /// Number of changes between trains, and walks between stations at the end.
    pub transfer_count: u32,
    /// What each change costs at the traveller's transfer aversion.
    pub transfer_cost_each: u32,
    /// Minutes by which the arrival falls before or after the arrival window.
    pub minutes_outside_arrival_window: u32,
    /// What those minutes cost.
    pub arrival_window_cost: u32,
    /// Minutes of the night spent asleep in a berth, which the cost takes off.
    pub sleeping_minutes: u32,
    /// Minutes of the night before arrival spent without a berth.
    pub night_minutes_awake: u32,
    /// What those minutes cost.
    pub night_awake_cost: u32,
    /// For a leg joined to others by same-day stops, what the best rest of the chain costs with
    /// this journey: the other legs and the stays between them. Zero for a leg on its own.
    pub rest_of_chain_cost: i32,
    /// The cost the planner ranked the journey by.
    pub total_cost: i32,
}

/// A span of time the planner searched in, such as a departure window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct SearchedWindow {
    /// First moment of the span.
    pub start: UnixTimeMinutes,
    /// Moment the span ends.
    pub end: UnixTimeMinutes,
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
    /// Whether `chosen_journey` is the one the traveller picked, instead of the planner's pick.
    pub follows_traveller_choice: bool,
    /// Cost of the planner's best journey for the leg on its own, in minutes of train travel,
    /// which lets the planner compare the leg with the same leg on another day. For a leg joined
    /// to others by same-day stops, the best journey is the one on the best chain.
    #[serde(default)]
    pub best_journey_cost: u32,
    /// The moments between which the planner let the traveller leave the departure place.
    /// `None` in plans stored without it.
    #[serde(default)]
    pub departure_window: Option<SearchedWindow>,
}

/// A day before or after a leg's date on which the leg can be travelled much better, such as
/// the day a night train runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct NearbyDaySuggestion {
    /// Days from the leg's departure date, `-1` or `1`.
    pub day_offset: i8,
    /// The departure date on that day.
    pub departure_date: NaiveDate,
    /// The plan of the leg on that day.
    pub plan: Box<LegPlan>,
}

/// A journey the traveller picked for a leg, recognised by its first departure and its trains.
/// It survives replanning as long as a journey with the same trains at the same time exists.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct JourneyChoice {
    /// Departure of the journey's first train.
    pub first_departure_time: UnixTimeMinutes,
    /// Every train of the journey in travel order.
    #[specta(type = Vec<TrainLabel>)]
    pub trains: SmallVec<[TrainLabel; 4]>,
}

impl JourneyChoice {
    /// Describes a journey as a choice.
    pub fn of_journey(journey: &Journey) -> Self {
        Self {
            first_departure_time: journey.first_ride.departure_time,
            trains: journey
                .rides()
                .map(|ride| ride.train_label.clone())
                .collect(),
        }
    }
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
    /// A day before or after a leg's date is much better for the leg. Sent after
    /// `PlanningFinished`, and a leg that is planned again loses its suggestion.
    NearbyDaySuggested {
        /// Identifies the run.
        run_id: PlanningRunId,
        /// Place at which the leg arrives.
        arrival_place_id: TripPlaceId,
        /// The better day.
        suggestion: NearbyDaySuggestion,
    },
}
