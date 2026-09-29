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
    /// How much each minute of arriving outside the arrival window costs, in percent of a
    /// minute on a train.
    pub arrival_window_miss_percent: u32,
    /// How much each minute of the night spent off a train with berths costs, in percent of a
    /// minute on a train. Only overnight legs have a night.
    pub night_awake_percent: u32,
}

impl ScoringWeights {
    /// Weights for when the user has not tuned anything. One transfer costs from 10 minutes at
    /// no aversion to two hours at full aversion, a minute to or from a station counts one and
    /// a half times, and a minute of arriving outside the window counts twice. A minute of the
    /// night without a berth counts three times, so missing a whole night of 7.5 hours costs
    /// almost a day of travel.
    pub const DEFAULT: Self = Self {
        transfer_base_minutes: 10,
        transfer_aversion_minutes: 110,
        way_to_station_percent: 150,
        arrival_window_miss_percent: 200,
        night_awake_percent: 300,
    };
}

/// The parts of a journey the cost depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct JourneyCostInputs {
    /// Minutes from leaving the departure place to reaching the arrival place.
    pub door_to_door_minutes: u32,
    /// Minutes of those spent between a place and a station, or between stations at the end.
    pub way_to_station_minutes: u32,
    /// Number of changes between trains.
    pub transfer_count: u32,
    /// Minutes by which the arrival at the place falls before or after the arrival window.
    pub minutes_outside_arrival_window: u32,
    /// Minutes of the night between bedtime and wake-up time spent on the train with berths the
    /// traveller sleeps on. They are part of the door-to-door time but cost nothing.
    pub sleeping_minutes: u32,
    /// Minutes of the night up to the arrival that are not spent on that train.
    pub night_minutes_awake: u32,
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
    let way_to_station_extra =
        inputs.way_to_station_minutes * weights.way_to_station_percent.saturating_sub(100) / 100;
    let arrival_window_miss =
        inputs.minutes_outside_arrival_window * weights.arrival_window_miss_percent / 100;
    let night_awake = inputs.night_minutes_awake * weights.night_awake_percent / 100;
    inputs
        .door_to_door_minutes
        .saturating_sub(inputs.sleeping_minutes)
        + way_to_station_extra
        + inputs.transfer_count * transfer_penalty
        + arrival_window_miss
        + night_awake
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cost(door_to_door: u32, way_to_station: u32, transfers: u32, aversion: u8) -> u32 {
        cost_with_window_miss(door_to_door, way_to_station, transfers, aversion, 0)
    }

    fn cost_with_window_miss(
        door_to_door: u32,
        way_to_station: u32,
        transfers: u32,
        aversion: u8,
        minutes_outside_arrival_window: u32,
    ) -> u32 {
        journey_cost(
            JourneyCostInputs {
                door_to_door_minutes: door_to_door,
                way_to_station_minutes: way_to_station,
                transfer_count: transfers,
                minutes_outside_arrival_window,
                sleeping_minutes: 0,
                night_minutes_awake: 0,
            },
            TransferAversion::new(aversion).unwrap(),
            ScoringWeights::DEFAULT,
        )
    }

    #[test]
    fn arriving_outside_the_window_costs_twice_the_minutes() {
        assert_eq!(cost_with_window_miss(300, 0, 0, 50, 45), 390);
        // Arriving an hour late loses against a journey that takes 90 minutes longer.
        assert!(cost_with_window_miss(300, 0, 0, 50, 60) > cost(390, 0, 0, 50));
    }

    #[test]
    fn a_night_in_a_berth_beats_a_faster_night_awake() {
        let night = |door_to_door, sleeping_minutes| {
            journey_cost(
                JourneyCostInputs {
                    door_to_door_minutes: door_to_door,
                    way_to_station_minutes: 0,
                    transfer_count: 0,
                    minutes_outside_arrival_window: 0,
                    sleeping_minutes,
                    night_minutes_awake: 450 - sleeping_minutes,
                },
                TransferAversion::new(50).unwrap(),
                ScoringWeights::DEFAULT,
            )
        };
        // Sleeping the whole night costs only the waking hours on the train.
        assert_eq!(night(900, 450), 450);
        // Leaving the train at midnight saves three hours but loses 6.5 hours of sleep.
        assert!(night(720, 60) > night(900, 450));
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
