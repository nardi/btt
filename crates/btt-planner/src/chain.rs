//! Pairs the journeys of legs joined by same-day stops, so each journey gets the cost of the
//! best complete chain through it.

use crate::scoring::ScoringWeights;

/// What the pairing needs to know about one journey of a leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChainJourney {
    /// When the traveller leaves the departure place, in bundle minutes.
    pub departure: u32,
    /// When the traveller reaches the arrival place, in bundle minutes.
    pub arrival: u32,
    /// Cost of the journey on its own.
    pub cost: u32,
}

/// The stay the traveller wants at a same-day stop, in minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChainStay {
    /// Shortest stay.
    pub minimum: u32,
    /// Longest stay, or `None` for no upper bound.
    pub maximum: Option<u32>,
    /// Whether a longer stay within the range is better.
    pub prefers_longer: bool,
}

impl ChainStay {
    /// Returns the cost of staying a number of minutes, or `None` when the next journey leaves
    /// before the traveller arrives. A stay outside the range costs per minute outside, and a
    /// longer stay within the range can lower the cost below zero.
    fn cost(self, minutes: i64, weights: ScoringWeights) -> Option<i64> {
        if minutes < 0 {
            return None;
        }
        let minimum = i64::from(self.minimum);
        let maximum = self.maximum.map(i64::from);
        let outside =
            (minimum - minutes).max(0) + maximum.map_or(0, |maximum| (minutes - maximum).max(0));
        let mut cost = outside * i64::from(weights.stay_outside_range_percent) / 100;
        if self.prefers_longer {
            let useful_minutes = maximum.map_or(minutes, |maximum| minutes.min(maximum));
            let extra = (useful_minutes - minimum).max(0);
            cost -= extra * i64::from(weights.longer_stay_bonus_percent) / 100;
        }
        Some(cost)
    }
}

/// Returns, for each leg and journey, the cost of the best chain of journeys through it, or
/// `None` when no chain goes through it.
///
/// `stays[i]` lies between leg `i` and leg `i + 1`. `forced[i]` fixes leg `i` to one journey,
/// such as the traveller's pick, and no chain goes through the other journeys of that leg.
pub(crate) fn chain_through_costs(
    legs: &[Vec<ChainJourney>],
    stays: &[ChainStay],
    forced: &[Option<usize>],
    weights: ScoringWeights,
) -> Vec<Vec<Option<i64>>> {
    let allowed = |leg: usize, journey: usize| forced[leg].is_none_or(|fixed| fixed == journey);
    let pair_cost = |leg: usize, from: &ChainJourney, to: &ChainJourney| {
        stays[leg].cost(i64::from(to.departure) - i64::from(from.arrival), weights)
    };

    // Best cost of a chain from the first leg up to and including each journey.
    let mut forward: Vec<Vec<Option<i64>>> = Vec::with_capacity(legs.len());
    for (leg, journeys) in legs.iter().enumerate() {
        let costs = journeys
            .iter()
            .enumerate()
            .map(|(index, journey)| {
                if !allowed(leg, index) {
                    return None;
                }
                let own = i64::from(journey.cost);
                if leg == 0 {
                    return Some(own);
                }
                legs[leg - 1]
                    .iter()
                    .zip(&forward[leg - 1])
                    .filter_map(|(previous, previous_cost)| {
                        Some((*previous_cost)? + pair_cost(leg - 1, previous, journey)?)
                    })
                    .min()
                    .map(|best| best + own)
            })
            .collect();
        forward.push(costs);
    }

    // Best cost of the rest of the chain after each journey, without the journey itself.
    let mut backward: Vec<Vec<Option<i64>>> = vec![Vec::new(); legs.len()];
    for leg in (0..legs.len()).rev() {
        backward[leg] = legs[leg]
            .iter()
            .enumerate()
            .map(|(index, journey)| {
                if !allowed(leg, index) {
                    return None;
                }
                if leg + 1 == legs.len() {
                    return Some(0);
                }
                legs[leg + 1]
                    .iter()
                    .zip(&backward[leg + 1])
                    .filter_map(|(next, next_rest)| {
                        Some(pair_cost(leg, journey, next)? + i64::from(next.cost) + (*next_rest)?)
                    })
                    .min()
            })
            .collect();
    }

    forward
        .iter()
        .zip(&backward)
        .map(|(forward_costs, backward_costs)| {
            forward_costs
                .iter()
                .zip(backward_costs)
                .map(|(forward_cost, backward_cost)| Some((*forward_cost)? + (*backward_cost)?))
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journey(departure: u32, arrival: u32, cost: u32) -> ChainJourney {
        ChainJourney {
            departure,
            arrival,
            cost,
        }
    }

    const TWO_TO_FIVE_HOURS: ChainStay = ChainStay {
        minimum: 120,
        maximum: Some(300),
        prefers_longer: false,
    };

    #[test]
    fn a_short_stay_costs_more_than_a_later_train() {
        // The first leg arrives at 600. The next leaves at 620 or 740, both costing 100 on their
        // own. Twenty minutes of stay are 100 minutes short of the two hours.
        let legs = vec![
            vec![journey(300, 600, 300)],
            vec![journey(620, 900, 100), journey(740, 1020, 100)],
        ];
        let costs = chain_through_costs(
            &legs,
            &[TWO_TO_FIVE_HOURS],
            &[None, None],
            ScoringWeights::DEFAULT,
        );
        assert_eq!(costs[1], [Some(300 + 100 + 200), Some(400)]);
        assert_eq!(
            costs[0],
            [Some(400)],
            "the first leg takes the best pairing"
        );
    }

    #[test]
    fn a_departure_before_the_arrival_is_impossible() {
        let legs = vec![vec![journey(300, 600, 300)], vec![journey(550, 900, 100)]];
        let costs = chain_through_costs(
            &legs,
            &[TWO_TO_FIVE_HOURS],
            &[None, None],
            ScoringWeights::DEFAULT,
        );
        assert_eq!(costs, [vec![None], vec![None]]);
    }

    #[test]
    fn longer_is_better_rewards_extra_time_up_to_the_maximum() {
        let longer = ChainStay {
            prefers_longer: true,
            ..TWO_TO_FIVE_HOURS
        };
        let legs = vec![
            vec![journey(300, 600, 300)],
            vec![
                journey(720, 900, 100),
                journey(900, 1080, 100),
                journey(960, 1140, 100),
            ],
        ];
        let costs = chain_through_costs(&legs, &[longer], &[None, None], ScoringWeights::DEFAULT);
        // Staying the full five hours gives three hours beyond the minimum, a bonus of 45. An
        // hour past the maximum costs 120 and gains no more bonus.
        assert_eq!(costs[1], [Some(400), Some(400 - 45), Some(400 + 120 - 45)]);
    }

    #[test]
    fn a_forced_journey_fixes_the_chain() {
        let legs = vec![
            vec![journey(300, 600, 300), journey(360, 660, 280)],
            vec![journey(780, 900, 100), journey(840, 960, 100)],
        ];
        let costs = chain_through_costs(
            &legs,
            &[TWO_TO_FIVE_HOURS],
            &[Some(0), None],
            ScoringWeights::DEFAULT,
        );
        assert_eq!(
            costs[0][1], None,
            "the other journey of a forced leg is out"
        );
        assert_eq!(costs[1], [Some(400), Some(400)]);
    }

    #[test]
    fn a_short_stay_wins_when_the_later_train_arrives_much_too_late() {
        // The first leg arrives at 600 and the stay should last two hours. Leaving at 700 is
        // 20 minutes short, which costs 40. The train that respects the stay arrives 90
        // minutes after the next place's window, which its own cost holds as 180.
        let legs = vec![
            vec![journey(300, 600, 300)],
            vec![journey(700, 900, 100), journey(720, 990, 100 + 180)],
        ];
        let costs = chain_through_costs(
            &legs,
            &[TWO_TO_FIVE_HOURS],
            &[None, None],
            ScoringWeights::DEFAULT,
        );
        assert_eq!(costs[1], [Some(300 + 100 + 40), Some(300 + 280)]);

        // When respecting the stay only costs 5 minutes outside the window, it wins.
        let legs = vec![
            vec![journey(300, 600, 300)],
            vec![journey(700, 900, 100), journey(720, 905, 100 + 10)],
        ];
        let costs = chain_through_costs(
            &legs,
            &[TWO_TO_FIVE_HOURS],
            &[None, None],
            ScoringWeights::DEFAULT,
        );
        assert!(costs[1][1] < costs[1][0]);
    }
}
