//! Turns a journey into one cost in minutes, so the planner can pick among Pareto-optimal
//! journeys.

use btt_model::TransferAversion;

/// Weights of the journey cost. All values count in minutes of train travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScoringWeights {
    /// Cost of one transfer at the lowest transfer aversion.
    pub transfer_base_minutes: u32,
    /// Cost one transfer adds at the highest transfer aversion, on top of the base.
    pub transfer_aversion_minutes: u32,
    /// How much a minute on the way between a place and a station costs, in percent of a
    /// minute on a train. Above 100 favors stations close to the place.
    pub way_to_station_percent: u32,
}

impl ScoringWeights {
    /// Weights for when the user has not tuned anything. One transfer costs from 10 minutes at
    /// no aversion to two hours at full aversion, and a minute to or from a station counts
    /// one and a half times.
    pub const DEFAULT: Self = Self {
        transfer_base_minutes: 10,
        transfer_aversion_minutes: 110,
        way_to_station_percent: 150,
    };
}

/// The parts of a journey the cost depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JourneyCostInputs {
    /// Minutes from leaving the departure place to reaching the arrival place.
    pub door_to_door_minutes: u32,
    /// Minutes of those spent between a place and a station, or between stations at the end.
    pub way_to_station_minutes: u32,
    /// Number of changes between trains.
    pub transfer_count: u32,
}

/// Returns the cost of a journey in minutes. Lower is better.
pub fn journey_cost(
    inputs: JourneyCostInputs,
    transfer_aversion: TransferAversion,
    weights: ScoringWeights,
) -> u32 {
    let transfer_penalty = weights.transfer_base_minutes
        + weights.transfer_aversion_minutes * u32::from(transfer_aversion.percent()) / 100;
    // The door-to-door time already counts the way to the stations once, so only the extra
    // part of its weight is added.
    let way_to_station_extra = inputs.way_to_station_minutes
        * weights.way_to_station_percent.saturating_sub(100)
        / 100;
    inputs.door_to_door_minutes + way_to_station_extra + inputs.transfer_count * transfer_penalty
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cost(door_to_door: u32, way_to_station: u32, transfers: u32, aversion: u8) -> u32 {
        journey_cost(
            JourneyCostInputs {
                door_to_door_minutes: door_to_door,
                way_to_station_minutes: way_to_station,
                transfer_count: transfers,
            },
            TransferAversion::new(aversion).unwrap(),
            ScoringWeights::DEFAULT,
        )
    }

    #[test]
    fn a_direct_journey_costs_its_door_to_door_time_plus_the_extra_way_weight() {
        assert_eq!(cost(300, 20, 0, 50), 310);
    }

    #[test]
    fn transfer_aversion_scales_the_transfer_penalty() {
        assert_eq!(cost(300, 0, 2, 0), 320);
        assert_eq!(cost(300, 0, 2, 100), 540);
    }

    #[test]
    fn a_nearby_station_beats_a_slightly_faster_far_one() {
        // The far station saves 5 minutes door to door but adds 15 minutes of getting there.
        let nearby = cost(400, 5, 1, 50);
        let far = cost(395, 20, 1, 50);
        assert!(nearby < far, "nearby {nearby}, far {far}");
    }
}
