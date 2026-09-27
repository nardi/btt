//! RAPTOR journey search over a timetable bundle.
//!
//! The search follows RAPTOR by Delling, Pajor and Werneck, with two extensions. All candidate
//! stations of the departure place start together, and range search runs the rounds once per
//! departure time in a window, from the latest to the earliest, without resetting the labels.

mod search;

pub use search::{
    BoardAnyTrip, BoardingFilter, CandidateStation, MinutesSinceBundleStart, ParetoJourneySet,
    RaptorJourney, RaptorLegQuery, RaptorRide, RaptorSearchBuffers, search_leg_journeys,
};
