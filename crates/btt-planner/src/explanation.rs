//! What a leg plan came from, for problem reports: the candidate stations and every journey
//! the search found, with its cost.

use btt_model::{GeoCoordinates, Journey, LegPlanStation, LegPlanningResult};
use serde::Serialize;
use smol_str::SmolStr;

use crate::scoring::JourneyCostInputs;

/// A leg plan together with what it came from.
#[derive(Debug, Clone, Serialize)]
pub struct LegExplanation {
    /// The plan, as the app received it.
    pub result: LegPlanningResult,
    /// Stations near the departure place the search started from.
    pub departure_candidates: Vec<CandidateExplanation>,
    /// Stations near the arrival place the search could end at.
    pub arrival_candidates: Vec<CandidateExplanation>,
    /// Every journey of the Pareto set in the order the planner ranked them, chosen first.
    pub ranked_journeys: Vec<RankedJourneyExplanation>,
    /// Stations the journeys of `ranked_journeys` refer to.
    pub station_table: Vec<LegPlanStation>,
    /// Whether the first journey is the traveller's pick.
    pub follows_traveller_choice: bool,
}

impl LegExplanation {
    /// Explains a leg whose search never ran, such as one without stations near a place.
    pub fn without_search(result: LegPlanningResult) -> Self {
        Self {
            result,
            departure_candidates: Vec::new(),
            arrival_candidates: Vec::new(),
            ranked_journeys: Vec::new(),
            station_table: Vec::new(),
            follows_traveller_choice: false,
        }
    }
}

/// A station near a place that the search considered.
#[derive(Debug, Clone, Serialize)]
pub struct CandidateExplanation {
    /// Name of the station.
    pub station_name: SmolStr,
    /// Location of the station.
    pub coordinates: GeoCoordinates,
    /// Straight-line distance from the place.
    pub distance_meters: u32,
    /// Estimated minutes between the place and the station.
    pub minutes_to_place: u16,
}

/// A journey of the Pareto set with its cost.
#[derive(Debug, Clone, Serialize)]
pub struct RankedJourneyExplanation {
    /// The journey, with station indices into `LegExplanation::station_table`.
    pub journey: Journey,
    /// The cost the ranking used, lower is better.
    pub cost: u32,
    /// What the cost came from.
    pub cost_inputs: JourneyCostInputs,
}
