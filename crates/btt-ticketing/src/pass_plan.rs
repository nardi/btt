//! Chooses the cheapest way to cover the legs of a trip: single tickets, or a pass with a
//! window of validity and its travel days.
//!
//! The searches of the planner offer, for every leg, the best journey on single tickets and
//! the best ones on pass days. The solve picks one option per leg, and a pass from the
//! catalogue with a window, so that the total of the costs and the prices is the lowest. It
//! is a multiple-choice knapsack over the travel days, which is small: a pass has at most
//! fifteen of them and a trip has tens of legs.

use btt_fares::{PassCatalogue, PassKind, PassType, PriceAgeGroup};
use btt_model::{
    AgeGroup, EuroCents, PassDays, PassScenario, PassScenarioLeg, PriceWeight, ScenarioPass,
    TravelClass, Traveller,
};
use chrono::{Days, NaiveDate};
use smallvec::SmallVec;

/// How many dates past a leg's date the state tracks coverage for. A journey uses at most
/// `PassDays::MAXIMUM` travel days from the date its first train leaves on.
const COVERAGE_DAYS: u32 = PassDays::MAXIMUM as u32;

/// One way of covering a leg in the choice of the whole trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegOption {
    /// A journey on single tickets.
    Single {
        /// The journey's cost in minutes of train travel.
        cost_minutes: u32,
        /// The estimated price of its tickets, in euro cents.
        price_cents: EuroCents,
    },
    /// A journey whose trains the pass covers, which uses travel days.
    Pass {
        /// The journey's cost in minutes of train travel.
        cost_minutes: u32,
        /// The price of what the pass does not cover, such as reservations and the night
        /// accommodation, in euro cents.
        price_cents: EuroCents,
        /// How many travel days the journey uses.
        days: PassDays,
        /// The first travel day it uses, the date its first train leaves on.
        first_date: NaiveDate,
    },
}

impl LegOption {
    /// Returns the journey's cost in minutes of train travel.
    fn cost_minutes(self) -> u32 {
        match self {
            Self::Single { cost_minutes, .. } | Self::Pass { cost_minutes, .. } => cost_minutes,
        }
    }

    /// Returns the journey's price in euro cents.
    fn price_cents(self) -> EuroCents {
        match self {
            Self::Single { price_cents, .. } | Self::Pass { price_cents, .. } => price_cents,
        }
    }
}

/// The options of one leg of the trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegToPlan {
    /// The leg's departure date, which is the first date of its pass journeys.
    pub date: NaiveDate,
    /// The journeys the leg can take, single tickets and pass days.
    pub options: SmallVec<[LegOption; 4]>,
}

/// A trip to solve, with everything the passes need to know.
#[derive(Debug)]
pub struct TripToPlan<'a> {
    /// The options of each leg, in travel order. The dates never go backwards.
    pub legs: &'a [LegToPlan],
    /// The passes the traveller can buy.
    pub passes: &'a PassCatalogue,
    /// The age group that prices the passes.
    pub age_group: PriceAgeGroup,
    /// The class the traveller travels in.
    pub class: TravelClass,
    /// How many minutes of travel time one euro is worth, in hundredths of a minute. Zero
    /// buys no pass, because a pass saves only money.
    pub price_weight_hundredths: u32,
}

/// One way of covering the whole trip: the chosen option of each leg, and the pass when the
/// trip takes one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripScenario {
    /// The pass in the catalogue, or `None` when every leg takes single tickets.
    pub pass: Option<usize>,
    /// The first day of the pass's validity, or `None` without a pass.
    pub pass_start: Option<NaiveDate>,
    /// The chosen option of each leg, as an index into its options.
    pub choices: Vec<usize>,
    /// The total cost of the journeys in minutes of train travel.
    pub minutes: u64,
    /// The total price in euro cents, the pass included.
    pub price_cents: u64,
}

impl TripScenario {
    /// Returns the total the planner decides by: the costs plus the weight times the prices,
    /// in hundredths of a minute.
    fn weighted_cost(&self, price_weight_hundredths: u32) -> u64 {
        if price_weight_hundredths == 0 {
            return self.minutes * 100;
        }
        self.minutes * 100 + u64::from(price_weight_hundredths) * self.price_cents / 100
    }
}

/// One state of the choices over the legs so far: how many travel days the chosen pass
/// journeys use together, and which of the dates around the current leg they already cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChoiceState {
    /// Travel days used, counting a day that several journeys share once.
    days_used: u32,
    /// Which dates the chosen journeys cover, as one bit per day from the state's date. A
    /// journey covers the date its first train leaves on and the days after it that its
    /// trains leave on.
    coverage: u32,
}

/// The best choice that reaches a state, with where it came from.
#[derive(Debug, Clone, Copy)]
struct Choice {
    /// The weighted cost so far, in hundredths of a minute.
    cost: u64,
    /// The price so far, in euro cents.
    price: u64,
    /// The minutes so far.
    minutes: u64,
    /// The option of the leg this state comes from, as an index into its options.
    option: usize,
    /// The state before the leg's choice, in the table of the leg before.
    previous: ChoiceState,
}

/// Chooses the cheapest ways to cover the trip: single tickets, and the best one or two
/// passes with their windows and days. The scenarios come sorted, the cheapest first, and the
/// solve is exact for the options it gets.
///
/// A pass journey uses the day its first train leaves on and the days after it that its
/// trains leave on, and it has to fit in the pass's validity. Legs that share a travel day,
/// such as the legs of a same-day chain, share a day of the pass.
pub fn solve(request: &TripToPlan<'_>) -> Vec<TripScenario> {
    let mut scenarios = vec![single_tickets_scenario(request.legs)];
    // Without a weight a pass saves nothing that counts, so the solve does not offer one.
    if request.price_weight_hundredths > 0 {
        let mut pass_scenarios: Vec<TripScenario> = Vec::new();
        for (index, pass) in request.passes.passes.iter().enumerate() {
            let Some(price) = pass.price(request.age_group, request.class) else {
                continue;
            };
            let Some(mut scenario) = best_window(request, index, pass, price) else {
                continue;
            };
            scenario.price_cents += u64::from(price.0);
            pass_scenarios.push(scenario);
        }
        pass_scenarios
            .sort_by_key(|scenario| scenario.weighted_cost(request.price_weight_hundredths));
        scenarios.extend(pass_scenarios.into_iter().take(2));
    }
    scenarios.sort_by_key(|scenario| scenario.weighted_cost(request.price_weight_hundredths));
    scenarios
}

/// Returns the scenario in which every leg takes its single tickets.
fn single_tickets_scenario(legs: &[LegToPlan]) -> TripScenario {
    let mut choices = Vec::with_capacity(legs.len());
    let mut minutes = 0_u64;
    let mut price = 0_u64;
    for leg in legs {
        let option = leg
            .options
            .iter()
            .position(|option| matches!(option, LegOption::Single { .. }))
            .unwrap_or(0);
        let chosen = leg.options[option];
        minutes += u64::from(chosen.cost_minutes());
        price += u64::from(chosen.price_cents().0);
        choices.push(option);
    }
    TripScenario {
        pass: None,
        pass_start: None,
        choices,
        minutes,
        price_cents: price,
    }
}

/// Returns the best scenario with one pass, over every window that starts at a leg's date.
/// The pass's own price counts in the decision.
fn best_window(
    request: &TripToPlan<'_>,
    pass_index: usize,
    pass: &PassType,
    price: EuroCents,
) -> Option<TripScenario> {
    let mut starts: Vec<NaiveDate> = request.legs.iter().map(|leg| leg.date).collect();
    starts.dedup();
    let pass_weighted = u64::from(request.price_weight_hundredths) * u64::from(price.0) / 100;
    starts
        .into_iter()
        .filter_map(|start| solve_window(request, pass_index, pass, start))
        .min_by_key(|scenario| {
            scenario.weighted_cost(request.price_weight_hundredths) + pass_weighted
        })
}

/// Solves one leg at a time within one window of validity, counting the travel days. The
/// state of the choices holds the days used and the dates covered, because a journey of one
/// leg can cover the date of a journey of a later leg.
fn solve_window(
    request: &TripToPlan<'_>,
    pass_index: usize,
    pass: &PassType,
    start: NaiveDate,
) -> Option<TripScenario> {
    let end = pass.last_valid_date(start);
    let day_budget = match pass.kind {
        PassKind::Flexi { travel_days } => u32::from(travel_days),
        PassKind::Continuous => u32::MAX,
    };
    // One table of states per leg, the states after the leg's choice.
    let mut tables: Vec<Vec<(ChoiceState, Choice)>> = Vec::with_capacity(request.legs.len());
    let mut previous_date = start;
    for (leg_index, leg) in request.legs.iter().enumerate() {
        let shift = if leg_index == 0 {
            0
        } else {
            u32::try_from((leg.date - previous_date).num_days().max(0)).unwrap_or(COVERAGE_DAYS + 1)
        };
        // The states of the leg before, their coverage moved to this leg's date.
        let sources: Vec<ChoiceState> = if leg_index == 0 {
            vec![ChoiceState {
                days_used: 0,
                coverage: 0,
            }]
        } else {
            tables[leg_index - 1]
                .iter()
                .map(|(state, _)| shift_coverage(*state, shift))
                .collect()
        };
        let mut next: Vec<(ChoiceState, Choice)> = Vec::new();
        for (source_index, source) in sources.iter().enumerate() {
            let previous = if leg_index == 0 {
                None
            } else {
                Some(tables[leg_index - 1][source_index].1)
            };
            // The state as the table of the leg before holds it, without the move to this
            // leg's date, so the choices can be followed back.
            let previous_state = if leg_index == 0 {
                *source
            } else {
                tables[leg_index - 1][source_index].0
            };
            for (option_index, option) in leg.options.iter().enumerate() {
                let Some(state) = state_after(*source, option, leg.date, start, end, day_budget)
                else {
                    continue;
                };
                let price =
                    previous.map_or(0, |choice| choice.price) + u64::from(option.price_cents().0);
                let minutes =
                    previous.map_or(0, |choice| choice.minutes) + u64::from(option.cost_minutes());
                // The weight times the price of the whole choice so far, so the rounding is
                // the same as the one the totals use.
                let entry = Choice {
                    cost: minutes * 100 + u64::from(request.price_weight_hundredths) * price / 100,
                    price,
                    minutes,
                    option: option_index,
                    previous: previous_state,
                };
                match next.iter().position(|(known, _)| *known == state) {
                    Some(found) if next[found].1.cost <= entry.cost => {}
                    Some(found) => next[found].1 = entry,
                    None => next.push((state, entry)),
                }
            }
        }
        if next.is_empty() {
            return None;
        }
        tables.push(next);
        previous_date = leg.date;
    }
    let (_, choice) = tables
        .last()
        .and_then(|table| table.iter().min_by_key(|(_, choice)| choice.cost))?;
    // Follow the choices back to the first leg, keeping the totals of the last leg's choice.
    let best_choice = *choice;
    let mut choices = vec![0_usize; request.legs.len()];
    let mut choice = *choice;
    for leg_index in (0..request.legs.len()).rev() {
        choices[leg_index] = choice.option;
        if leg_index == 0 {
            break;
        }
        let state = choice.previous;
        let (_, previous_choice) = tables[leg_index - 1]
            .iter()
            .find(|(known, _)| *known == state)?;
        choice = *previous_choice;
    }
    Some(TripScenario {
        pass: Some(pass_index),
        pass_start: Some(start),
        choices,
        minutes: best_choice.minutes,
        price_cents: best_choice.price,
    })
}

/// Returns a state with its coverage moved to a date `shift` days later. Coverage that lies
/// before the new date is gone, and coverage far past it no longer matters.
fn shift_coverage(state: ChoiceState, shift: u32) -> ChoiceState {
    ChoiceState {
        days_used: state.days_used,
        coverage: if shift >= 32 {
            0
        } else {
            state.coverage >> shift
        },
    }
}

/// Returns the state after a leg takes an option, or `None` when a pass journey does not fit
/// the window or the budget. A pass journey covers the dates from its first date on, one bit
/// per day from the leg's date.
fn state_after(
    source: ChoiceState,
    option: &LegOption,
    leg_date: NaiveDate,
    start: NaiveDate,
    end: NaiveDate,
    day_budget: u32,
) -> Option<ChoiceState> {
    let LegOption::Pass {
        days, first_date, ..
    } = option
    else {
        return Some(source);
    };
    if first_date < &start || first_date < &leg_date {
        return None;
    }
    let last_used = first_date
        .checked_add_days(Days::new(u64::from(days.days()) - 1))
        .unwrap_or(*first_date);
    if last_used > end {
        return None;
    }
    let offset = u32::try_from((*first_date - leg_date).num_days()).ok()?;
    let mut state = source;
    for day in 0..u32::from(days.days()) {
        let bit = 1_u32.checked_shl(offset + day)?;
        if state.coverage & bit == 0 {
            state.days_used += 1;
            state.coverage |= bit;
        }
    }
    (state.days_used <= day_budget).then_some(state)
}

/// Returns the scenarios of a trip: single tickets on every leg, and the best one or two
/// passes. The legs come in travel order, each with the journey it takes on single tickets and
/// its pass options. The scenarios come sorted, the lowest weighted total first.
///
/// The traveller's age on the first leg's date and class price the passes. A child or infant
/// travels free on a pass with an adult, which the scenarios do not model, so they get only
/// single tickets, and so does a trip without a weight.
pub fn pass_scenarios(
    legs: &[PassScenarioLeg],
    passes: &PassCatalogue,
    traveller: Option<&Traveller>,
    price_weight: PriceWeight,
) -> Vec<PassScenario> {
    let to_plan: Vec<LegToPlan> = legs
        .iter()
        .map(|leg| LegToPlan {
            date: leg.date,
            // The single tickets come first, so option `index` is pass option `index - 1`.
            options: std::iter::once(LegOption::Single {
                cost_minutes: leg.single_cost_minutes,
                price_cents: leg.single_price.unwrap_or_default(),
            })
            .chain(leg.pass_options.iter().map(|option| LegOption::Pass {
                cost_minutes: option.cost_minutes,
                price_cents: option.price_cents,
                days: option.days,
                first_date: option.first_date,
            }))
            .collect(),
        })
        .collect();
    let first_date = legs.first().map_or(NaiveDate::MIN, |leg| leg.date);
    let age_group = traveller.map_or(AgeGroup::Adult, |traveller| {
        traveller.age_group_on(first_date)
    });
    let class = traveller.map_or(TravelClass::Second, |traveller| traveller.travel_class);
    let price_weight_hundredths = match PriceAgeGroup::of(age_group) {
        Some(_) => u32::from(price_weight.hundredths_of_minute_per_euro()),
        None => 0,
    };
    let request = TripToPlan {
        legs: &to_plan,
        passes,
        age_group: PriceAgeGroup::of(age_group).unwrap_or(PriceAgeGroup::Adult),
        class,
        price_weight_hundredths,
    };
    solve(&request)
        .into_iter()
        .map(|scenario| {
            let pass = scenario
                .pass
                .zip(scenario.pass_start)
                .and_then(|(index, start)| {
                    let pass = passes.passes.get(index)?;
                    Some(ScenarioPass {
                        name: format!("Interrail Global Pass, {}", pass.title()).into(),
                        first_valid_date: start,
                        last_valid_date: pass.last_valid_date(start),
                        price: pass.price(request.age_group, class)?,
                    })
                });
            PassScenario {
                pass,
                pass_option_of_leg: scenario
                    .choices
                    .iter()
                    .map(|&choice| {
                        choice
                            .checked_sub(1)
                            .and_then(|option| u8::try_from(option).ok())
                    })
                    .collect(),
                minutes: u32::try_from(scenario.minutes).unwrap_or(u32::MAX),
                price: EuroCents(u32::try_from(scenario.price_cents).unwrap_or(u32::MAX)),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use btt_model::PassOptionSummary;
    use proptest::prelude::*;

    /// A small catalogue: one flexi pass of 4 days in a month for 200 euro, one continuous
    /// pass of 15 days for 500 euro, and one pass with no adult price.
    fn catalogue() -> PassCatalogue {
        PassCatalogue::from_toml(
            r#"
            as_of = "2026-01-01"
            source = "test"

            [[pass]]
            id = "4-in-1"
            kind = "flexi"
            travel_days = 4
            validity_months = 1
            [pass.prices.adult]
            second = 200.0
            first = 260.0

            [[pass]]
            id = "15-continuous"
            kind = "continuous"
            validity_days = 15
            [pass.prices.adult]
            second = 500.0
            first = 650.0

            [[pass]]
            id = "youth-only"
            kind = "flexi"
            travel_days = 7
            validity_months = 1
            [pass.prices.youth]
            second = 300.0
            first = 380.0
            "#,
        )
        .unwrap()
    }

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 7, 1).unwrap() + Days::new(u64::from(day))
    }

    /// The leg of the plan's deciding case: a slow regional train on single tickets and a
    /// fast ICE that a pass covers for nothing.
    fn ice_leg(day: u32) -> LegToPlan {
        LegToPlan {
            date: date(day),
            options: smallvec::smallvec![
                LegOption::Single {
                    cost_minutes: 300,
                    price_cents: EuroCents(3_500),
                },
                LegOption::Pass {
                    cost_minutes: 120,
                    price_cents: EuroCents(0),
                    days: PassDays::new(1).unwrap(),
                    first_date: date(day),
                },
            ],
        }
    }

    fn request<'catalogue>(
        legs: &'catalogue [LegToPlan],
        weight: u32,
        catalogue: &'catalogue PassCatalogue,
    ) -> TripToPlan<'catalogue> {
        TripToPlan {
            legs,
            passes: catalogue,
            age_group: PriceAgeGroup::of(AgeGroup::Adult).unwrap(),
            class: TravelClass::Second,
            price_weight_hundredths: weight,
        }
    }

    #[test]
    fn the_pass_wins_when_its_days_are_worth_more_than_its_price() {
        // The plan's case: four legs of a slow cheap train and a fast expensive one. On
        // single tickets the regional train wins, and the pass turns every leg into the ICE.
        let legs: Vec<LegToPlan> = [0, 1, 2, 3].iter().map(|day| ice_leg(*day)).collect();
        let catalogue = catalogue();
        let scenarios = solve(&request(&legs, 300, &catalogue));
        assert_eq!(scenarios.len(), 3, "single tickets and the two passes");
        let best = &scenarios[0];
        assert_eq!(best.pass, Some(0), "the 200 euro pass covers all four legs");
        assert_eq!(best.pass_start, Some(date(0)));
        assert!(best.choices.iter().all(|&choice| choice == 1));
        // Four ICE legs of 120 minutes and the pass price.
        assert_eq!(best.minutes, 480);
        assert_eq!(best.price_cents, 20_000);
        // Single tickets come next, with the regional train on every leg, and the continuous
        // pass of 500 euro last.
        let singles = &scenarios[1];
        assert_eq!(singles.pass, None);
        assert!(singles.choices.iter().all(|&choice| choice == 0));
        assert_eq!(singles.minutes, 1_200);
        assert_eq!(singles.price_cents, 14_000);
        assert_eq!(scenarios[2].pass, Some(1));
    }

    #[test]
    fn a_day_the_pass_shares_between_legs_counts_once() {
        // Legs of a chain on the same date share the pass day, so the flexi pass of four days
        // covers all six, and the last leg, which has no pass option, takes single tickets.
        let legs: Vec<LegToPlan> = [0, 0, 1, 1, 2, 2]
            .iter()
            .map(|day| ice_leg(*day))
            .chain(std::iter::once(LegToPlan {
                date: date(2),
                options: smallvec::smallvec![LegOption::Single {
                    cost_minutes: 400,
                    price_cents: EuroCents(9_000),
                }],
            }))
            .collect();
        let catalogue = catalogue();
        let scenarios = solve(&request(&legs, 300, &catalogue));
        let best = &scenarios[0];
        assert_eq!(best.pass, Some(0));
        assert!(best.choices[..6].iter().all(|&choice| choice == 1));
        assert_eq!(best.choices[6], 0, "the leg without a pass option");
    }

    /// The leg of `ice_leg` as the app sends it, with the journey on single tickets apart from
    /// the pass options.
    fn scenario_leg(day: u32) -> PassScenarioLeg {
        PassScenarioLeg {
            date: date(day),
            single_cost_minutes: 300,
            single_price: Some(EuroCents(3_500)),
            pass_options: vec![PassOptionSummary {
                cost_minutes: 120,
                price_cents: EuroCents(0),
                days: PassDays::new(1).unwrap(),
                first_date: date(day),
            }],
        }
    }

    #[test]
    fn a_scenario_names_its_pass_and_the_pass_option_of_each_leg() {
        let legs: Vec<PassScenarioLeg> = (0..4).map(scenario_leg).collect();
        let scenarios = pass_scenarios(&legs, &catalogue(), None, PriceWeight::FRUGAL);
        let best = &scenarios[0];
        let pass = best.pass.as_ref().unwrap();
        assert!(pass.name.starts_with("Interrail Global Pass, "));
        assert_eq!(pass.first_valid_date, date(0));
        assert_eq!(pass.last_valid_date, date(30));
        assert_eq!(pass.price, EuroCents(20_000));
        // Every leg takes its first pass option, and the price holds the pass.
        assert_eq!(best.pass_option_of_leg, vec![Some(0); 4]);
        assert_eq!(best.minutes, 480);
        assert_eq!(best.price, EuroCents(20_000));
        let singles = &scenarios[1];
        assert_eq!(singles.pass, None);
        assert_eq!(singles.pass_option_of_leg, vec![None; 4]);
    }

    #[test]
    fn a_child_gets_single_tickets_only() {
        let legs: Vec<PassScenarioLeg> = (0..4).map(scenario_leg).collect();
        let child = Traveller::new("Kim".into(), date(0) - Days::new(8 * 366), "NL".into());
        let scenarios = pass_scenarios(&legs, &catalogue(), Some(&child), PriceWeight::FRUGAL);
        assert_eq!(scenarios.len(), 1);
        assert_eq!(scenarios[0].pass, None);
    }

    #[test]
    fn without_a_weight_no_pass_wins() {
        let legs: Vec<LegToPlan> = [0, 1, 2, 3].iter().map(|day| ice_leg(*day)).collect();
        let catalogue = catalogue();
        let scenarios = solve(&request(&legs, 0, &catalogue));
        assert_eq!(scenarios.len(), 1);
        assert_eq!(scenarios[0].pass, None);
    }

    /// Returns the weighted cost of a choice of options, with the day rules, or `None` when
    /// the choice breaks the window or the budget.
    fn brute_force_choice(
        request: &TripToPlan<'_>,
        pass: Option<usize>,
        start: Option<NaiveDate>,
        choices: &[usize],
    ) -> Option<u64> {
        let (pass, start, budget) = match pass {
            None => (None, None, u32::MAX),
            Some(index) => {
                let pass = &request.passes.passes[index];
                let start = start?;
                let budget = match pass.kind {
                    PassKind::Flexi { travel_days } => u32::from(travel_days),
                    PassKind::Continuous => u32::MAX,
                };
                (Some(pass), Some(start), budget)
            }
        };
        let end = match (pass, start) {
            (Some(pass), Some(start)) => pass.last_valid_date(start),
            _ => NaiveDate::MAX,
        };
        let mut price = 0_u64;
        let mut minutes = 0_u64;
        let mut used_days: std::collections::BTreeSet<NaiveDate> =
            std::collections::BTreeSet::new();
        for (leg, &choice) in request.legs.iter().zip(choices) {
            let option = leg.options[choice];
            price += u64::from(option.price_cents().0);
            minutes += u64::from(option.cost_minutes());
            if let LegOption::Pass {
                days, first_date, ..
            } = option
            {
                pass?;
                let last_used = first_date + Days::new(u64::from(days.days()) - 1);
                if first_date < start.unwrap() || last_used > end {
                    return None;
                }
                for day in 0..u32::from(days.days()) {
                    used_days.insert(first_date + Days::new(u64::from(day)));
                }
            }
        }
        if used_days.len() > budget as usize {
            return None;
        }
        if let Some(pass) = pass {
            let pass_price = pass
                .price(request.age_group, request.class)
                .unwrap_or(EuroCents(0));
            price += u64::from(pass_price.0);
        }
        Some(minutes * 100 + u64::from(request.price_weight_hundredths) * price / 100)
    }

    /// Returns the best weighted cost of every combination of options, over every pass and
    /// window that starts at a leg's date.
    fn brute_force_best(request: &TripToPlan<'_>) -> Option<u64> {
        let mut starts: Vec<NaiveDate> = request.legs.iter().map(|leg| leg.date).collect();
        starts.dedup();
        let mut best: Option<u64> = None;
        let keep = |best: &mut Option<u64>, cost: Option<u64>| {
            if let Some(cost) = cost {
                *best = Some(best.map_or(cost, |known: u64| known.min(cost)));
            }
        };
        let count = request.legs.len();
        let mut choices = vec![0_usize; count];
        // Walk every combination of options like an odometer.
        loop {
            keep(&mut best, brute_force_choice(request, None, None, &choices));
            // Without a weight the solve offers no pass, so the search leaves them out too.
            if request.price_weight_hundredths > 0 {
                for (index, pass) in request.passes.passes.iter().enumerate() {
                    if pass.price(request.age_group, request.class).is_none() {
                        continue;
                    }
                    for &start in &starts {
                        keep(
                            &mut best,
                            brute_force_choice(request, Some(index), Some(start), &choices),
                        );
                    }
                }
            }
            // The next combination, or the end.
            let mut position = count;
            loop {
                if position == 0 {
                    return best;
                }
                position -= 1;
                choices[position] += 1;
                if choices[position] < request.legs[position].options.len() {
                    break;
                }
                choices[position] = 0;
            }
        }
    }

    /// What a random leg is made of: its single option, its pass option with the days, and
    /// the days the leg follows the one before.
    type RandomLegPart = (LegOption, Option<(u32, EuroCents, PassDays, u8)>, u8);

    /// Builds a random leg. The dates never go backwards, the way a trip's legs do.
    fn random_leg() -> impl Strategy<Value = RandomLegPart> {
        (
            30..600_u32,
            500..15_000_u32,
            proptest::option::of((30..600_u32, 0..3_000_u32, 1..=3_u8, 0..=1_u8)),
            0..=4_u8,
        )
            .prop_map(|(cost, price, pass, gap)| {
                let single = LegOption::Single {
                    cost_minutes: cost,
                    price_cents: EuroCents(price),
                };
                let pass_option = pass.map(|(cost, price, days, extra_day)| {
                    (
                        cost,
                        EuroCents(price),
                        PassDays::new(days).unwrap(),
                        extra_day,
                    )
                });
                (single, pass_option, gap)
            })
    }

    proptest! {
        #[test]
        fn the_solve_matches_the_search_over_every_choice(
            parts in proptest::collection::vec(random_leg(), 3..=6),
            weight in 0_u32..800,
        ) {
            // The days add up, so the dates never go backwards.
            let legs: Vec<LegToPlan> = {
                let mut legs = Vec::with_capacity(parts.len());
                let mut day = 0_u32;
                for (single, pass_option, gap) in parts {
                    day += u32::from(gap);
                    let leg_date = date(day);
                    let mut options = smallvec::smallvec![single];
                    if let Some((cost, price, days, extra_day)) = pass_option {
                        options.push(LegOption::Pass {
                            cost_minutes: cost,
                            price_cents: price,
                            days,
                            first_date: leg_date + Days::new(u64::from(extra_day)),
                        });
                    }
                    legs.push(LegToPlan {
                        date: leg_date,
                        options,
                    });
                }
                legs
            };
            let catalogue = catalogue();
            let request = TripToPlan {
                legs: &legs,
                passes: &catalogue,
                age_group: PriceAgeGroup::of(AgeGroup::Adult).unwrap(),
                class: TravelClass::Second,
                price_weight_hundredths: weight,
            };
            let scenarios = solve(&request);
            let expected = brute_force_best(&request);
            if let Some(expected) = expected {
                prop_assert_eq!(scenarios[0].weighted_cost(weight), expected);
            }
        }
    }
}
