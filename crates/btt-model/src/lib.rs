//! Domain types shared by the BTT backend and frontend.
//!
//! This crate has no I/O. Every public type derives `specta::Type`, so the Tauri app can export
//! it to TypeScript.

// Lets the fixtures of the integration tests, which name this crate, compile inside it.
#[cfg(test)]
extern crate self as btt_model;

mod bounded;
pub mod coordinates;
pub mod edit;
pub mod history;
pub mod itinerary;
pub mod link;
pub mod place_search;
pub mod plan;
pub mod snapshot;
pub mod tickets;
pub mod time;
pub mod traveller;
pub mod trip;

pub use bounded::ValueOutOfRangeError;
pub use coordinates::{GeoCoordinates, Latitude, Longitude};
pub use edit::{
    PlaceInsertionPoint, TripEdit, TripEditError, TripEditNotice, TripEditNotices, TripEditOutcome,
};
pub use history::{MAXIMUM_HISTORY_STEPS, TripHistory, TripHistoryState};
pub use itinerary::{
    AcceptedBerths, BerthType, Itinerary, ItineraryLeg, ItineraryRoute, LegTravelMode,
    OvernightLegOptions, PlaceStay, PlaceWindowOverride, SameDayStayRange, StationSearchRadius,
    TripPlace, TripPlaceId, ViaPlace,
};
pub use link::{
    LinkAlphabet, SnapshotLink, SnapshotLinkError, decode_snapshot_link, encode_snapshot_link,
    encode_snapshot_link_in,
};
pub use place_search::PlaceSearchResult;
pub use plan::{
    AlternativeKind, ArrivalFootpath, Journey, JourneyChoice, JourneyCostParts, LayoutLink,
    LayoutLinkKind, LayoutLinkSource, LegExtras, LegPlan, LegPlanStation, LegPlanStationIndex,
    LegPlanningResult, NearbyDaySuggestion, NoJourneyReason, PassJourneyOption, PlanningRunId,
    PlanningSessionEvent, SearchedWindow, StationWay, TrainLabel, TrainRide, TransferAndRide,
    WayToStationMode,
};
pub use snapshot::{SNAPSHOT_SCHEMA_VERSION, SnapshotLeg, StoredSnapshot, TripSnapshot};
pub use tickets::{
    AccommodationRequest, BookingLink, BookingLinkKind, EuroCents, EuroRange, ForeignAmount,
    LiveQuote, LiveQuoteRequest, PassOptionSummary, PassScenario, PassScenarioLeg, Price,
    PriceKind, PriceObservation, PriceSource, PriceTotal, RideDescription, RunDescription,
    ScenarioPass, SiteSearch, StrategyKind, TicketItem, TicketLeg, TicketPlan, TicketProduct,
    TicketRide, TicketStrategy, UnorderedEuroRangeError,
};
pub use time::{
    DailyTimeWindow, DailyWindowLength, DurationMinutes, LocalTimeOfDay, TravelDayWindows,
    UnixTimeMinutes,
};
pub use traveller::{AgeGroup, DiscountCard, OwnedInterrailPass, Traveller};
pub use trip::{
    CURRENT_TRIP_SCHEMA_VERSION, ChosenJourney, FareType, InterrailPassPolicy, PassDays,
    PriceWeight, StoredTrip, TicketPreferences, TransferAversion, TravelClass, TravellerId, Trip,
    TripId, TripPreferences, TripSummary, TripSummaryStop,
};
