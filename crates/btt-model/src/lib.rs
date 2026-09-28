//! Domain types shared by the BTT backend and frontend.
//!
//! This crate has no I/O. Every public type derives `specta::Type`, so the Tauri app can export
//! it to TypeScript.

mod bounded;
pub mod coordinates;
pub mod edit;
pub mod itinerary;
pub mod place_search;
pub mod plan;
pub mod time;
pub mod trip;

pub use bounded::ValueOutOfRangeError;
pub use coordinates::{GeoCoordinates, Latitude, Longitude};
pub use edit::{
    PlaceInsertionPoint, TripEdit, TripEditError, TripEditNotice, TripEditNotices, TripEditOutcome,
};
pub use itinerary::{
    AcceptedBerths, BerthType, Itinerary, ItineraryLeg, ItineraryRoute, LegTravelMode,
    OvernightLegOptions, PlaceStay, PlaceWindowOverride, SameDayStayRange, StationSearchRadius,
    TripPlace, TripPlaceId, ViaPlace,
};
pub use place_search::PlaceSearchResult;
pub use plan::{
    ArrivalFootpath, Journey, LegPlan, LegPlanStation, LegPlanStationIndex, LegPlanningResult,
    NoJourneyReason, PlanningRunId, PlanningSessionEvent, StationWay, TrainLabel, TrainRide,
    TransferAndRide, WayToStationMode,
};
pub use time::{
    DailyTimeWindow, DailyWindowLength, DurationMinutes, LocalTimeOfDay, TravelDayWindows,
    UnixTimeMinutes,
};
pub use trip::{
    CURRENT_TRIP_SCHEMA_VERSION, InterrailPassPolicy, PriceWeight, StoredTrip, TransferAversion,
    TravelClass, TravellerId, Trip, TripId, TripPreferences, TripSummary,
};
