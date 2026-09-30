//! Turns a journey into one cost in minutes, so the planner can pick among Pareto-optimal
//! journeys.

use btt_model::{JourneyCostParts, TransferAversion};

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
    /// How much each minute of arriving before the arrival window costs, in percent of a minute
    /// on a train. A traveller who arrives early can wait, so it costs no more than waiting.
    pub arrival_before_window_percent: u32,
    /// How much each minute of arriving after the arrival window costs, in percent of a minute
    /// on a train.
    pub arrival_after_window_percent: u32,
    /// How much each minute of the night spent off a train with berths costs, in percent of a
    /// minute on a train. Only overnight legs have a night.
    pub night_awake_percent: u32,
    /// How much each minute of a same-day stay outside its range costs, in percent of a minute
    /// on a train.
    pub stay_outside_range_percent: u32,
    /// How much each minute of a same-day stay beyond its minimum takes off the cost when the
    /// traveller prefers a longer stay, in percent of a minute on a train.
    pub longer_stay_bonus_percent: u32,
}

impl ScoringWeights {
    /// Weights for when the user has not tuned anything. One transfer costs from 10 minutes at
    /// no aversion to two hours at full aversion, and a minute to or from a station counts one
    /// and a half times. A minute of arriving before the window counts once, like the waiting it
    /// stands for, and a minute after it counts twice. A minute of the night without a berth
    /// counts three times, so missing a whole night of 7.5 hours costs almost a day of travel.
    /// A minute of stay outside a same-day stay range counts twice, and with "longer is better"
    /// a minute beyond the minimum takes a quarter minute off.
    pub const DEFAULT: Self = Self {
        transfer_base_minutes: 10,
        transfer_aversion_minutes: 110,
        way_to_station_percent: 150,
        arrival_before_window_percent: 100,
        arrival_after_window_percent: 200,
        night_awake_percent: 300,
        stay_outside_range_percent: 200,
        longer_stay_bonus_percent: 25,
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
    /// Minutes by which the arrival at the place falls before the arrival window.
    pub minutes_before_arrival_window: u32,
    /// Minutes by which the arrival at the place falls after the arrival window.
    pub minutes_after_arrival_window: u32,
    /// Minutes of the night between bedtime and wake-up time spent on trains with berths. They
    /// are part of the door-to-door time but cost nothing.
    pub sleeping_minutes: u32,
    /// Minutes of the night up to the arrival that are not spent on a train with berths.
    pub night_minutes_awake: u32,
}

/// Returns the cost of a journey in minutes. Lower is better.
pub fn journey_cost(
    inputs: JourneyCostInputs,
    transfer_aversion: TransferAversion,
    weights: ScoringWeights,
) -> u32 {
    let total = journey_cost_parts(inputs, transfer_aversion, weights).total_cost;
    u32::try_from(total).unwrap_or(0)
}

/// Returns what the cost of a journey on its own is made of, with the total that
/// `journey_cost` returns.
pub fn journey_cost_parts(
    inputs: JourneyCostInputs,
    transfer_aversion: TransferAversion,
    weights: ScoringWeights,
) -> JourneyCostParts {
    let transfer_cost_each = weights.transfer_base_minutes
        + weights.transfer_aversion_minutes * u32::from(transfer_aversion.percent()) / 100;
    // The door-to-door time already counts the way to the stations once, so only the extra
    // part of its weight is added.
    let way_to_station_extra_cost =
        inputs.way_to_station_minutes * weights.way_to_station_percent.saturating_sub(100) / 100;
    let arrival_window_cost = inputs.minutes_before_arrival_window
        * weights.arrival_before_window_percent
        / 100
        + inputs.minutes_after_arrival_window * weights.arrival_after_window_percent / 100;
    let night_awake_cost = inputs.night_minutes_awake * weights.night_awake_percent / 100;
    let sleeping_minutes = inputs.sleeping_minutes.min(inputs.door_to_door_minutes);
    let total = inputs.door_to_door_minutes - sleeping_minutes
        + way_to_station_extra_cost
        + inputs.transfer_count * transfer_cost_each
        + arrival_window_cost
        + night_awake_cost;
    JourneyCostParts {
        door_to_door_minutes: inputs.door_to_door_minutes,
        way_to_station_minutes: inputs.way_to_station_minutes,
        way_to_station_extra_cost,
        transfer_count: inputs.transfer_count,
        transfer_cost_each,
        minutes_outside_arrival_window: inputs.minutes_before_arrival_window
            + inputs.minutes_after_arrival_window,
        arrival_window_cost,
        sleeping_minutes,
        night_minutes_awake: inputs.night_minutes_awake,
        night_awake_cost,
        rest_of_chain_cost: 0,
        total_cost: i32::try_from(total).unwrap_or(i32::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inputs of a journey without a night and inside its arrival window.
    fn inputs(door_to_door: u32, way_to_station: u32, transfers: u32) -> JourneyCostInputs {
        JourneyCostInputs {
            door_to_door_minutes: door_to_door,
            way_to_station_minutes: way_to_station,
            transfer_count: transfers,
            minutes_before_arrival_window: 0,
            minutes_after_arrival_window: 0,
            sleeping_minutes: 0,
            night_minutes_awake: 0,
        }
    }

    fn cost_of(inputs: JourneyCostInputs, aversion: u8) -> u32 {
        journey_cost(
            inputs,
            TransferAversion::new(aversion).unwrap(),
            ScoringWeights::DEFAULT,
        )
    }

    fn cost(door_to_door: u32, way_to_station: u32, transfers: u32, aversion: u8) -> u32 {
        cost_of(inputs(door_to_door, way_to_station, transfers), aversion)
    }

    #[test]
    fn arriving_late_costs_twice_the_minutes_and_early_once() {
        let late = JourneyCostInputs {
            minutes_after_arrival_window: 45,
            ..inputs(300, 0, 0)
        };
        assert_eq!(cost_of(late, 50), 390);
        let early = JourneyCostInputs {
            minutes_before_arrival_window: 45,
            ..inputs(300, 0, 0)
        };
        assert_eq!(cost_of(early, 50), 345);
        // Arriving an hour late loses against a journey that takes 90 minutes longer.
        let an_hour_late = JourneyCostInputs {
            minutes_after_arrival_window: 60,
            ..inputs(300, 0, 0)
        };
        assert!(cost_of(an_hour_late, 50) > cost(390, 0, 0, 50));
    }

    #[test]
    fn arriving_early_costs_what_waiting_for_the_window_would() {
        // Arriving an hour early and waiting costs the same as leaving an hour later and
        // arriving when the window opens.
        let early = JourneyCostInputs {
            minutes_before_arrival_window: 60,
            ..inputs(300, 0, 0)
        };
        assert_eq!(cost_of(early, 50), cost(360, 0, 0, 50));
    }

    #[test]
    fn a_night_in_a_berth_beats_a_faster_night_awake() {
        let night = |door_to_door, sleeping_minutes| {
            cost_of(
                JourneyCostInputs {
                    sleeping_minutes,
                    night_minutes_awake: 450 - sleeping_minutes,
                    ..inputs(door_to_door, 0, 0)
                },
                50,
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

    #[test]
    fn the_cost_parts_add_up_to_the_cost() {
        let inputs = JourneyCostInputs {
            door_to_door_minutes: 900,
            way_to_station_minutes: 30,
            transfer_count: 2,
            minutes_before_arrival_window: 10,
            minutes_after_arrival_window: 25,
            sleeping_minutes: 400,
            night_minutes_awake: 50,
        };
        let aversion = TransferAversion::new(50).unwrap();
        let parts = journey_cost_parts(inputs, aversion, ScoringWeights::DEFAULT);
        let sum = i64::from(parts.door_to_door_minutes) - i64::from(parts.sleeping_minutes)
            + i64::from(parts.way_to_station_extra_cost)
            + i64::from(parts.transfer_count * parts.transfer_cost_each)
            + i64::from(parts.arrival_window_cost)
            + i64::from(parts.night_awake_cost);
        assert_eq!(sum, i64::from(parts.total_cost));
        assert_eq!(
            parts.total_cost,
            i32::try_from(journey_cost(inputs, aversion, ScoringWeights::DEFAULT)).unwrap()
        );
        assert_eq!(parts.transfer_cost_each, 65);
        assert_eq!(parts.minutes_outside_arrival_window, 35);
        assert_eq!(parts.arrival_window_cost, 10 + 50);
    }
}
