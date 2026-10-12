//! Finds the cheapest tickets for the rides of a trip.
//!
//! The problem splits by day. For one day, the cheapest single tickets are a shortest path over
//! the rides of the day, where a ticket covers a run of consecutive rides. For a pass, a day is
//! worth what its single tickets cost minus what the pass days cost in reservations. A flexi
//! pass takes the days of the highest worth, and a continuous pass takes every day that pays.
//! The start of the pass is tried at every day of the trip.

use std::ops::Range;

use btt_fares::{
    FareData, NightFares, NightOperator, PassKind, PassType, PriceAgeGroup, PriceBook,
    ReservationRule, RideFacts, RuleScope,
};
use btt_model::{
    AgeGroup, EuroCents, EuroRange, Price, PriceKind, PriceSource, PriceTotal, StrategyKind,
    TicketItem, TicketPreferences, TicketProduct, TicketStrategy, TravelClass,
};
use chrono::NaiveDate;
use smol_str::SmolStr;

use crate::links::{pass_link, shop_link};
use crate::rides::RideInput;

/// An operator that sells tickets for its own trains only.
struct OwnTicketOperator {
    /// The operator as `data/operators.toml` names it.
    id: &'static str,
    /// The name of its shop on a booking link.
    shop: &'static str,
    /// The start page of its shop.
    shop_url: &'static str,
}

/// The largest distance between the station a ride arrives at and the station the next ride
/// leaves from, for one ticket to cover both. Two platforms of one station can lie a few
/// hundred metres apart.
const MAXIMUM_CHANGE_METERS: f64 = 1000.0;

/// The longest wait between two rides, in minutes, for one ticket to cover both. A longer wait
/// is a stay, and the booking sites sell the parts before and after it as journeys of their own.
const MAXIMUM_CHANGE_MINUTES: u32 = 120;

/// The operators that sell tickets for their own trains only. A ticket never joins one of their
/// rides with a ride of another operator.
const OWN_TICKET_OPERATORS: &[OwnTicketOperator] = &[
    OwnTicketOperator {
        id: "italo",
        shop: "italotreno.com",
        shop_url: "https://www.italotreno.com/en",
    },
    OwnTicketOperator {
        id: "flixtrain",
        shop: "flixtrain.com",
        shop_url: "https://www.flixtrain.com/",
    },
    OwnTicketOperator {
        id: "regiojet",
        shop: "regiojet.com",
        shop_url: "https://regiojet.com/",
    },
    OwnTicketOperator {
        id: "leo-express",
        shop: "leoexpress.com",
        shop_url: "https://www.leoexpress.com/en",
    },
    OwnTicketOperator {
        id: "westbahn",
        shop: "westbahn.at",
        shop_url: "https://westbahn.at/en/",
    },
    OwnTicketOperator {
        id: "european-sleeper",
        shop: "europeansleeper.eu",
        shop_url: "https://www.europeansleeper.eu/",
    },
    OwnTicketOperator {
        id: "snalltaget",
        shop: "snalltaget.se",
        shop_url: "https://www.snalltaget.se/en",
    },
    OwnTicketOperator {
        id: "eurostar",
        shop: "eurostar.com",
        shop_url: "https://www.eurostar.com/",
    },
    OwnTicketOperator {
        id: "ouigo",
        shop: "ouigo.com",
        shop_url: "https://www.ouigo.com/",
    },
    OwnTicketOperator {
        id: "ouigo-espana",
        shop: "ouigo.com",
        shop_url: "https://www.ouigo.com/es/",
    },
    OwnTicketOperator {
        id: "iryo",
        shop: "iryo.eu",
        shop_url: "https://iryo.eu/en/",
    },
];

/// Returns the operator of a run that sells its own tickets, or `None` when the run has none.
/// Such an operator runs every ride of the run, because a ticket never joins its rides with
/// others.
fn own_ticket_operator(rides: &[RideInput]) -> Option<&'static OwnTicketOperator> {
    OWN_TICKET_OPERATORS.iter().find(|operator| {
        rides
            .iter()
            .any(|ride| ride.train_label.operator == operator.id)
    })
}

/// Who the tickets are for and how they are chosen.
pub struct Settings<'a> {
    /// Class of travel.
    pub class: TravelClass,
    /// Age group on the first day of travel.
    pub age_group: AgeGroup,
    /// Country of residence as a two-letter code, if known.
    pub residence_country: Option<&'a str>,
    /// How tickets are chosen.
    pub preferences: &'a TicketPreferences,
    /// The day the tickets are assumed to be bought.
    pub purchase_date: NaiveDate,
    /// Today's date. Booking sites sell no tickets for trains that left before it.
    pub today: NaiveDate,
    /// The share of the adult single ticket price that the traveller pays, in percent.
    pub single_ticket_percent: u32,
}

/// The cheapest single tickets for some rides of one day.
struct Cover {
    /// Runs of consecutive rides, as inclusive ride indices.
    runs: Vec<(usize, usize)>,
    /// What the runs cost at the middle of their ranges.
    middle: u64,
}

/// A pass with the days it is used on, ready to be turned into a strategy.
struct PassChoice<'a> {
    /// The pass.
    pass: &'a PassType,
    /// The price of the pass.
    price: EuroCents,
    /// The first day of validity.
    start: NaiveDate,
    /// The last day of validity.
    end: NaiveDate,
    /// The days of the trip, as indices into the day list, that the pass pays on.
    chosen_days: Vec<usize>,
    /// What everything costs with this pass, at the middle of the ranges, without the night
    /// train accommodation that every strategy pays.
    middle_total: u64,
}

/// One problem: the rides of a trip, and what it takes to price them.
pub struct Problem<'a> {
    /// The rides in travel order.
    rides: &'a [RideInput],
    /// The data files.
    data: &'a FareData,
    /// Who travels and how.
    settings: &'a Settings<'a>,
    /// The rides of each day, as ranges of ride indices.
    days: Vec<Range<usize>>,
    /// The price of the run from ride `i` to ride `i + offset`, at `run_prices[i][offset]`.
    run_prices: Vec<Vec<Price>>,
}

/// Adds the sources' note about a share of the price to a price.
fn scaled(price: &Price, percent: u32) -> Price {
    if percent == 100 {
        return price.clone();
    }
    let scale = |cents: EuroCents| EuroCents(cents.0 * percent / 100);
    let mut price = price.clone();
    price.low = scale(price.low);
    price.high = scale(price.high);
    if let Some(original) = &mut price.original {
        original.hundredths = original.hundredths * percent / 100;
    }
    price.source.note = format!(
        "{} The traveller pays {percent}% of the adult price.",
        price.source.note
    )
    .into();
    price
}

/// Turns the fee range of a rule into a price.
fn fee_price(rule: &ReservationRule, class: TravelClass) -> Price {
    let fee = rule.fee(class);
    let low = EuroCents::from_euro(fee.low);
    let high = EuroCents::from_euro(fee.high);
    let (name, scope_text) = if rule.scope == RuleScope::Night {
        ("Interrail night train fees", "accommodation")
    } else {
        ("Interrail reservation fees", "reservation")
    };
    let mut note = format!("Passholder {scope_text} fee of the rule {}.", rule.id);
    if let Some(text) = &rule.notes {
        note.push(' ');
        note.push_str(text);
    }
    if rule.unverified {
        note.push_str(" This fee is a rough value that has not been checked against its page.");
    }
    Price {
        low,
        high,
        kind: if low == high && !rule.unverified {
            PriceKind::Formula
        } else {
            PriceKind::Range
        },
        source: PriceSource {
            name: name.into(),
            note: note.into(),
            as_of: None,
            page: None,
        },
        original: None,
    }
}

/// Sums prices into a total.
pub fn total_of<'a>(prices: impl IntoIterator<Item = &'a Price>) -> PriceTotal {
    prices.into_iter().fold(
        PriceTotal {
            low: EuroCents::ZERO,
            high: EuroCents::ZERO,
            kind: PriceKind::Formula,
        },
        |total, price| PriceTotal {
            low: total.low.saturating_add(price.low),
            high: total.high.saturating_add(price.high),
            kind: total.kind.max(price.kind),
        },
    )
}

/// Returns the text for a class in titles.
fn class_text(class: TravelClass) -> &'static str {
    match class {
        TravelClass::First => "1st class",
        TravelClass::Second => "2nd class",
    }
}

/// Returns the text for an age group in titles.
fn age_text(group: PriceAgeGroup) -> &'static str {
    match group {
        PriceAgeGroup::Youth => "youth",
        PriceAgeGroup::Adult => "adult",
        PriceAgeGroup::Senior => "senior",
    }
}

impl<'a> Problem<'a> {
    /// Prepares a problem, which prices every run of consecutive rides within a day.
    pub fn new(
        rides: &'a [RideInput],
        data: &'a FareData,
        book: &'a PriceBook,
        settings: &'a Settings<'a>,
    ) -> Self {
        let mut days: Vec<Range<usize>> = Vec::new();
        for (index, ride) in rides.iter().enumerate() {
            match days.last_mut() {
                Some(day) if rides[day.start].departure_date == ride.departure_date => {
                    day.end = index + 1;
                }
                _ => days.push(index..index + 1),
            }
        }
        let mut run_prices = Vec::with_capacity(rides.len());
        for day in &days {
            for first in day.clone() {
                let prices = (first..day.end)
                    .map(|last| {
                        let facts: Vec<RideFacts<'_>> = rides[first..=last]
                            .iter()
                            .map(|ride| RideFacts {
                                from_country: ride.from_country.as_deref(),
                                to_country: ride.to_country.as_deref(),
                                operator: &ride.train_label.operator,
                                category: &ride.train_label.category,
                                distance_kilometres: ride.distance_kilometres,
                                offers_berths: ride.offers_berths,
                            })
                            .collect();
                        scaled(&book.price_run(&facts), settings.single_ticket_percent)
                    })
                    .collect();
                run_prices.push(prices);
            }
        }
        Self {
            rides,
            data,
            settings,
            days,
            run_prices,
        }
    }

    /// Returns the price of the single ticket that covers rides `first` to `last`.
    fn run_price(&self, first: usize, last: usize) -> &Price {
        &self.run_prices[first][last - first]
    }

    /// Returns the estimated price of the rides: the cheapest single tickets that cover them,
    /// split into runs within each day the way `cover` splits them, plus the night
    /// accommodation of the rides that have a rule. The median is the number the planner
    /// decides with, and the ends go along for display. This is what the single-ticket
    /// strategy of `strategies` costs, so a planned price and a ticket plan agree. Empty rides
    /// have no price.
    pub fn rides_price(&self) -> Option<EuroRange> {
        if self.rides.is_empty() {
            return None;
        }
        let mut low = 0_u64;
        let mut middle = 0_u64;
        let mut high = 0_u64;
        for day in &self.days {
            let cover = self.cover(day, |_| true);
            for &(first, last) in &cover.runs {
                let price = self.run_price(first, last);
                low += u64::from(price.low.0);
                middle += u64::from(price.middle().0);
                high += u64::from(price.high.0);
            }
        }
        for index in 0..self.rides.len() {
            if let Some(price) = self.accommodation_price(index, false) {
                low += u64::from(price.low.0);
                middle += u64::from(price.middle().0);
                high += u64::from(price.high.0);
            }
        }
        let to_cents = |cents: u64| EuroCents(u32::try_from(cents).unwrap_or(u32::MAX));
        EuroRange::new(to_cents(low), to_cents(middle), to_cents(high)).ok()
    }

    /// Returns the estimated price of the rides on the travel days of a pass: the fares of the
    /// rides the pass does not cover, the seat reservations of the ones it does, and the night
    /// accommodation of a pass holder, which the pass charges on top of the covered ride. The
    /// median is the number the planner decides with. Empty rides have no price.
    pub fn rides_price_on_pass(&self) -> Option<EuroRange> {
        if self.rides.is_empty() {
            return None;
        }
        let covered = self.pass_covers_ride();
        let mut parts: Vec<Price> = Vec::new();
        for day in &self.days {
            let cover = self.cover(day, |index| !covered[index]);
            for &(first, last) in &cover.runs {
                parts.push(self.run_price(first, last).clone());
            }
        }
        for (index, &is_covered) in covered.iter().enumerate() {
            if is_covered {
                if let Some(rule) = self.seat_rule(index) {
                    parts.push(fee_price(rule, self.settings.class));
                }
                if let Some(price) = self.accommodation_price(index, true) {
                    parts.push(price);
                }
            } else if let Some(price) = self.accommodation_price(index, false) {
                parts.push(price);
            }
        }
        let mut low = 0_u64;
        let mut middle = 0_u64;
        let mut high = 0_u64;
        for price in &parts {
            low += u64::from(price.low.0);
            middle += u64::from(price.middle().0);
            high += u64::from(price.high.0);
        }
        // Every part holds low <= middle <= high, so the sums do too, and saturating each at
        // the largest amount keeps that order.
        let to_cents = |cents: u64| EuroCents(u32::try_from(cents).unwrap_or(u32::MAX));
        EuroRange::new(to_cents(low), to_cents(middle), to_cents(high)).ok()
    }

    /// Returns whether one ticket can cover rides `first` to `last`. It can't when a ride
    /// of an operator that sells only its own trains shares the run with another operator, or
    /// when the rides do not form one journey: each ride has to leave from where the one before
    /// it arrived, within the time of a change. A day with a walk between two stations or a
    /// stay of hours takes a ticket for each part, as the booking sites sell them.
    fn sold_together(&self, first: usize, last: usize) -> bool {
        let rides = &self.rides[first..=last];
        let connected = rides.windows(2).all(|pair| {
            let (arriving, leaving) = (&pair[0], &pair[1]);
            arriving
                .to_coordinates
                .haversine_distance_meters(leaving.from_coordinates)
                <= MAXIMUM_CHANGE_METERS
                && leaving
                    .departure_time
                    .0
                    .saturating_sub(arriving.arrival_time.0)
                    <= MAXIMUM_CHANGE_MINUTES
        });
        if !connected {
            return false;
        }
        let operator = &rides[0].train_label.operator;
        let own_tickets_only = |operator: &str| {
            OWN_TICKET_OPERATORS
                .iter()
                .any(|listed| listed.id == operator)
        };
        rides
            .iter()
            .all(|ride| ride.train_label.operator == *operator)
            || !rides
                .iter()
                .any(|ride| own_tickets_only(&ride.train_label.operator))
    }

    /// Finds the cheapest single tickets for the rides of a day that `needs_ticket` accepts.
    fn cover(&self, day: &Range<usize>, needs_ticket: impl Fn(usize) -> bool) -> Cover {
        let mut cover = Cover {
            runs: Vec::new(),
            middle: 0,
        };
        let mut index = day.start;
        while index < day.end {
            if !needs_ticket(index) {
                index += 1;
                continue;
            }
            let segment_start = index;
            while index < day.end && needs_ticket(index) {
                index += 1;
            }
            let length = index - segment_start;
            // `best[k]` is the cheapest cover of the first `k` rides of the segment, and
            // `previous[k]` is where its last run starts.
            let mut best = vec![u64::MAX; length + 1];
            let mut previous = vec![0; length + 1];
            best[0] = 0;
            for end in 1..=length {
                for start in 0..end {
                    if !self.sold_together(segment_start + start, segment_start + end - 1) {
                        continue;
                    }
                    let price = u64::from(
                        self.run_price(segment_start + start, segment_start + end - 1)
                            .middle()
                            .0,
                    );
                    if best[start].saturating_add(price) < best[end] {
                        best[end] = best[start] + price;
                        previous[end] = start;
                    }
                }
            }
            cover.middle += best[length];
            let mut end = length;
            let mut runs = Vec::new();
            while end > 0 {
                let start = previous[end];
                runs.push((segment_start + start, segment_start + end - 1));
                end = start;
            }
            runs.reverse();
            cover.runs.extend(runs);
        }
        cover
    }

    /// Returns the rule for the night accommodation of a ride, if it has berths and a rule.
    fn night_rule(&self, index: usize) -> Option<&ReservationRule> {
        let ride = &self.rides[index];
        if !ride.offers_berths {
            return None;
        }
        self.data
            .reservations
            .for_ride(
                ride.from_country.as_deref(),
                ride.to_country.as_deref(),
                &ride.train_label.category,
                true,
            )
            .filter(|rule| rule.scope == RuleScope::Night)
    }

    /// Returns the price of the accommodation of a night ride, or `None` for a ride that needs
    /// none. A pass holder pays the accommodation with its reservation and no ticket. Without a
    /// pass, the ticket holds a seat, so the accommodation is the extra over the seat. A night
    /// train that the offers of the shop do not cover has the fee of its rule for both.
    pub(crate) fn accommodation_price(&self, index: usize, with_pass: bool) -> Option<Price> {
        let rule = self.night_rule(index)?;
        let ride = &self.rides[index];
        let fitted = NightOperator::of(&ride.train_label.operator, &ride.train_label.category)
            .and_then(|operator| {
                self.data.night_fares.accommodation_price(
                    operator,
                    &ride.train_label.category,
                    ride.distance_kilometres,
                    with_pass,
                )
            });
        Some(fitted.unwrap_or_else(|| fee_price(rule, self.settings.class)))
    }

    /// Returns the rule for the seat reservation of a ride on a pass, if one is charged.
    fn seat_rule(&self, index: usize) -> Option<&ReservationRule> {
        if self.night_rule(index).is_some() {
            return None;
        }
        let ride = &self.rides[index];
        self.data
            .reservations
            .for_ride(
                ride.from_country.as_deref(),
                ride.to_country.as_deref(),
                &ride.train_label.category,
                false,
            )
            .filter(|rule| {
                rule.mode
                    .is_charged(self.settings.preferences.reserve_optional_seats)
            })
    }

    /// Returns whether the pass covers each ride, apart from the dates. A pass covers no
    /// category it is known not to take. In the country of residence it covers only the
    /// journey out and the journey home, which are the rides before the first and after the
    /// last ride that touches another country.
    pub(crate) fn pass_covers_ride(&self) -> Vec<bool> {
        let rides = self.rides;
        let accepted: Vec<bool> = rides
            .iter()
            .map(|ride| {
                self.data
                    .reservations
                    .pass_accepted(&ride.train_label.category, &ride.train_label.operator)
            })
            .collect();
        let Some(residence) = self.settings.residence_country else {
            return accepted;
        };
        let at_home = |ride: &RideInput| {
            ride.from_country.as_deref() == Some(residence)
                && ride.to_country.as_deref() == Some(residence)
        };
        let first_abroad = rides.iter().position(|ride| !at_home(ride));
        let last_abroad = rides.iter().rposition(|ride| !at_home(ride));
        rides
            .iter()
            .enumerate()
            .map(|(index, ride)| {
                let allowed = !at_home(ride)
                    || first_abroad.is_some_and(|first| index < first)
                    || last_abroad.is_some_and(|last| index > last);
                accepted[index] && allowed
            })
            .collect()
    }

    /// Chooses the days of the best use of a pass that starts on `start`.
    fn choose_pass_days(
        &self,
        pass: &'a PassType,
        price: EuroCents,
        start: NaiveDate,
        covered_by_pass: &[bool],
        singles_by_day: &[Cover],
    ) -> PassChoice<'a> {
        let end = pass.last_valid_date(start);
        let on_pass = |index: usize| {
            covered_by_pass[index]
                && self.rides[index].departure_date >= start
                && self.rides[index].arrival_date <= end
        };
        // The worth of a day is what its single tickets cost, less what it costs on the pass.
        let mut worth: Vec<(i64, usize, u64)> = Vec::new();
        for (day_index, day) in self.days.iter().enumerate() {
            if !day.clone().any(on_pass) {
                continue;
            }
            let remaining = self.cover(day, |index| !on_pass(index)).middle;
            let reservations: u64 = day
                .clone()
                .filter(|&index| on_pass(index))
                .filter_map(|index| self.seat_rule(index))
                .map(|rule| fee_price(rule, self.settings.class).middle().0)
                .map(u64::from)
                .sum();
            // A night train costs the accommodation on a pass, and the extra over its seat on a
            // single ticket, which holds the seat.
            let night = |with_pass: &dyn Fn(usize) -> bool| -> u64 {
                day.clone()
                    .filter_map(|index| {
                        self.accommodation_price(index, with_pass(index))
                            .map(|price| u64::from(price.middle().0))
                    })
                    .sum()
            };
            let cost_on_pass = remaining + reservations + night(&on_pass);
            let singles = singles_by_day[day_index].middle + night(&|_| false);
            worth.push((
                i64::try_from(singles).unwrap_or(i64::MAX)
                    - i64::try_from(cost_on_pass).unwrap_or(i64::MAX),
                day_index,
                cost_on_pass,
            ));
        }
        worth.retain(|&(value, _, _)| value > 0);
        worth.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        if let PassKind::Flexi { travel_days } = pass.kind {
            worth.truncate(usize::from(travel_days));
        }
        worth.sort_by_key(|&(_, day_index, _)| day_index);
        let mut middle_total = u64::from(price.0);
        let mut chosen_days = Vec::new();
        for (day_index, singles) in singles_by_day.iter().enumerate() {
            if let Some(&(_, _, cost_on_pass)) = worth.iter().find(|entry| entry.1 == day_index) {
                chosen_days.push(day_index);
                middle_total += cost_on_pass;
            } else {
                middle_total += singles.middle;
            }
        }
        PassChoice {
            pass,
            price,
            start,
            end,
            chosen_days,
            middle_total,
        }
    }

    /// Returns the cheapest use of each pass that the traveller can buy, one for each pass.
    fn pass_choices(&self, singles_by_day: &[Cover]) -> Vec<PassChoice<'a>> {
        let Some(age_group) = PriceAgeGroup::of(self.settings.age_group) else {
            return Vec::new();
        };
        let covered = self.pass_covers_ride();
        let mut starts: Vec<NaiveDate> = self
            .days
            .iter()
            .map(|day| self.rides[day.start].departure_date)
            .collect();
        starts.dedup();
        let mut choices = Vec::new();
        for pass in &self.data.passes.passes {
            let Some(price) = pass.price(age_group, self.settings.class) else {
                continue;
            };
            let best = starts
                .iter()
                .map(|&start| self.choose_pass_days(pass, price, start, &covered, singles_by_day))
                .min_by_key(|choice| choice.middle_total);
            choices.extend(best);
        }
        choices
    }

    /// Solves the problem and returns the strategies from the cheapest at the middle of the
    /// ranges.
    pub fn strategies(&self) -> Vec<TicketStrategy> {
        let singles_by_day: Vec<Cover> = self
            .days
            .iter()
            .map(|day| self.cover(day, |_| true))
            .collect();
        let mut strategies = vec![self.single_ticket_strategy(&singles_by_day)];
        if self.settings.preferences.consider_passes {
            let choices = self.pass_choices(&singles_by_day);
            let best = choices.iter().min_by_key(|choice| choice.middle_total);
            // The cheapest pass of each kind, so the traveller sees a flexi and a continuous one.
            let best_of_other_kind = best.and_then(|best| {
                choices
                    .iter()
                    .filter(|choice| {
                        matches!(choice.pass.kind, PassKind::Continuous)
                            != matches!(best.pass.kind, PassKind::Continuous)
                    })
                    .min_by_key(|choice| choice.middle_total)
            });
            for choice in best.into_iter().chain(best_of_other_kind) {
                strategies.push(self.pass_strategy(choice, &singles_by_day));
            }
        }
        strategies.sort_by_key(|strategy| strategy.total.low.0.midpoint(strategy.total.high.0));
        strategies
    }

    /// Adds the night accommodation of every ride that has a rule to a strategy. `on_pass` says
    /// which rides the strategy covers with a pass.
    fn add_accommodation(&self, builder: &mut StrategyBuilder, on_pass: impl Fn(usize) -> bool) {
        for index in 0..self.rides.len() {
            if let Some(price) = self.accommodation_price(index, on_pass(index)) {
                let ride = &self.rides[index];
                let (link, live_request) = self.accommodation_link(ride, on_pass(index));
                let id = builder.add(
                    TicketProduct::NightAccommodation,
                    format!(
                        "Night train accommodation, {} {}",
                        ride.train_label.category, ride.train_label.number
                    ),
                    format!(
                        "{} to {} on {}",
                        ride.from_name,
                        ride.to_name,
                        ride.departure_date.format("%-d %b %Y")
                    ),
                    &[index],
                    price,
                    Some(link),
                );
                builder.items[usize::from(id)]
                    .as_mut()
                    .expect("the accommodation was just added")
                    .live_quote_request = live_request;
            }
        }
    }

    /// Returns the link to buy the accommodation of a night ride, and the request for its live
    /// price when the booking site of the operator sells the train. The ÖBB shop lists a train
    /// between the stations of the ride at its departure, and European Sleeper takes the codes
    /// of its stations.
    fn accommodation_link(
        &self,
        ride: &RideInput,
        with_pass: bool,
    ) -> (btt_model::BookingLink, Option<btt_model::LiveQuoteRequest>) {
        let shop = || shop_link(ride.from_country.as_deref());
        let category = ride.train_label.category.as_str();
        let Some(operator) = NightOperator::of(&ride.train_label.operator, category) else {
            return (shop(), None);
        };
        let number = &ride.train_label.number;
        let (link, site_search, train, seat_estimate) = match operator {
            NightOperator::Oebb => {
                let Some(link) = crate::links::obb_link(&self.data.obb_stations, ride, ride) else {
                    return (shop(), None);
                };
                // The shop names the Nightjet `NJ` and the EuroNight `EN`.
                let shop_category = if NightFares::is_euronight(category) {
                    "EN"
                } else {
                    "NJ"
                };
                let seat = self
                    .data
                    .night_fares
                    .seat_price(operator, category, ride.distance_kilometres)
                    .map(EuroCents::from_euro);
                (link, None, format!("{shop_category} {number}"), seat)
            }
            // Snälltåget has a booking page and no live lookup of accommodation.
            NightOperator::Snalltaget => {
                let link =
                    crate::links::snalltaget_link(&self.data.snalltaget_stations, ride, ride)
                        .map_or_else(shop, |(link, _)| link);
                return (link, None);
            }
            NightOperator::EuropeanSleeper => {
                let Some((link, search)) = crate::links::european_sleeper_link(
                    &self.data.european_sleeper_stations,
                    ride,
                    ride,
                ) else {
                    return (shop(), None);
                };
                (link, Some(search), format!("ES {number}"), None)
            }
        };
        let departed = ride.departure_date < self.settings.today;
        let request = (self.settings.preferences.live_prices
            && !departed
            && self.settings.class == TravelClass::Second)
            .then(|| btt_model::LiveQuoteRequest {
                booking_link: link.url.clone(),
                first_departure_time: ride.departure_time,
                ride_count: 1,
                travel_class: self.settings.class,
                site_search,
                run: None,
                refresh: false,
                accommodation: Some(btt_model::AccommodationRequest {
                    train: train.into(),
                    with_pass,
                    seat_estimate,
                }),
            });
        (link, request)
    }

    /// Says when sales open for a ride and whether they are open on the purchase date, or
    /// returns nothing when the booking window of the country is not known.
    fn sales_text(&self, ride: &RideInput) -> String {
        let Some(window) = self
            .data
            .booking_windows
            .iter()
            .find(|window| ride.from_country.as_deref() == Some(window.country.as_str()))
        else {
            return String::new();
        };
        let opens = ride
            .departure_date
            .checked_sub_days(chrono::Days::new(u64::from(window.days)))
            .unwrap_or(ride.departure_date);
        if self.settings.purchase_date < opens {
            format!(
                " Sales open on {}, which is after the assumed purchase date.",
                opens.format("%-d %b %Y")
            )
        } else {
            format!(" On sale since {}.", opens.format("%-d %b %Y"))
        }
    }

    /// Adds single tickets for the runs of a cover to a strategy.
    fn add_single_tickets(&self, builder: &mut StrategyBuilder, runs: &[(usize, usize)]) {
        for &(first, last) in runs {
            let rides = &self.rides[first..=last];
            let names: Vec<String> = rides
                .iter()
                .map(|ride| format!("{} {}", ride.train_label.category, ride.train_label.number))
                .collect();
            let indices: Vec<usize> = (first..=last).collect();
            let sales = self.sales_text(&rides[0]);
            let (link, live_request) = self.single_ticket_link(rides);
            let id = builder.add(
                TicketProduct::SingleTicket,
                format!("Single ticket, {}", names.join(" + ")),
                format!(
                    "{} to {} on {}.{sales}",
                    rides[0].from_name,
                    rides[rides.len() - 1].to_name,
                    rides[0].departure_date.format("%-d %b %Y")
                ),
                &indices,
                self.run_price(first, last).clone(),
                Some(link),
            );
            builder.items[usize::from(id)]
                .as_mut()
                .expect("the ticket was just added")
                .live_quote_request = live_request;
        }
    }

    /// Returns the link to buy a ticket for a run of rides, and the request for its live price
    /// when the booking site can tell it.
    ///
    /// Italo, Snälltåget, RegioJet and Eurostar trains are sold on the sites of their operators,
    /// and iryo trains on SBB. Other operators that sell only their own trains get the start page
    /// of their shop and no live price. Any other run goes to the site of the country it leaves
    /// from, which for a run that crosses a border is the most likely seller:
    ///
    /// - DB for Germany, SBB for Switzerland, ÖBB for Austria and Trenitalia for Italy, which
    ///   all sell trips abroad too.
    /// - NS for the Netherlands, Koleo for Poland, SBB for France, DSB for Denmark, SJ for
    ///   Sweden, MÁV for Hungary, Renfe for Spain and DB for Czechia, which only get runs that
    ///   stay in their country. Koleo sells PKP Intercity and most Polish regional operators.
    ///
    /// A run that crosses a border and that the site of its departure country cannot take goes
    /// to DB, which prices most trips in Europe. Anything else gets the shop of the country of
    /// the first ride and no live price.
    fn single_ticket_link(
        &self,
        rides: &[RideInput],
    ) -> (btt_model::BookingLink, Option<btt_model::LiveQuoteRequest>) {
        let first = &rides[0];
        let Some((link, site_search)) = self
            .operator_site_link(rides)
            .or_else(|| self.country_site_link(rides))
        else {
            let shop = own_ticket_operator(rides).map_or_else(
                || shop_link(first.from_country.as_deref()),
                |operator| btt_model::BookingLink {
                    label: format!("Open {}", operator.shop).into(),
                    url: operator.shop_url.into(),
                    kind: btt_model::BookingLinkKind::Search,
                },
            );
            return (shop, None);
        };
        // Booking sites sell no tickets for trains that have left.
        let departed = first.departure_date < self.settings.today;
        let request = (self.settings.preferences.live_prices && !departed).then(|| {
            btt_model::LiveQuoteRequest {
                booking_link: link.url.clone(),
                first_departure_time: first.departure_time,
                ride_count: u8::try_from(rides.len()).unwrap_or(u8::MAX),
                travel_class: self.settings.class,
                site_search,
                run: self.run_description(rides),
                refresh: false,
                accommodation: None,
            }
        });
        (link, request)
    }

    /// Describes a run for the price model, or returns `None` when the traveller is not an
    /// adult in second class, whose fare is the one the model predicts.
    fn run_description(&self, rides: &[RideInput]) -> Option<btt_model::RunDescription> {
        let adult_in_second_class = self.settings.class == TravelClass::Second
            && self.settings.age_group == AgeGroup::Adult;
        let (first, last) = (rides.first()?, rides.last()?);
        adult_in_second_class.then(|| btt_model::RunDescription {
            from_station: first.from_name.clone(),
            to_station: last.to_name.clone(),
            local_departure: first.departure_date.and_time(first.departure_clock),
            rides: rides
                .iter()
                .map(|ride| btt_model::RideDescription {
                    operator: ride.train_label.operator.clone(),
                    category: ride.train_label.category.clone(),
                    from_country: ride.from_country.clone(),
                    to_country: ride.to_country.clone(),
                    // Rides are far shorter than the 4 million km that `u32` metres hold.
                    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    distance_metres: (ride.distance_kilometres * 1000.0).round() as u32,
                    offers_berths: ride.offers_berths,
                })
                .collect(),
        })
    }

    /// Returns whether every ride of a run is a train of an operator.
    fn run_of_operator(rides: &[RideInput], operator: &str) -> bool {
        rides.iter().all(|ride| {
            ride.train_label.operator == operator
                // Plans stored without an operator still name Italo trains by their category.
                || (operator == "italo" && ride.train_label.category.eq_ignore_ascii_case("Italo"))
        })
    }

    /// Returns the link to the shop of the operator of a run whose operator sells its own
    /// tickets, with the search to run there, or `None` when no site with live prices sells
    /// its trains. Italo, Snälltåget, RegioJet and Eurostar sell their own, and SBB sells
    /// iryo.
    fn operator_site_link(
        &self,
        rides: &[RideInput],
    ) -> Option<(btt_model::BookingLink, Option<btt_model::SiteSearch>)> {
        let (first, last) = (&rides[0], &rides[rides.len() - 1]);
        let data = self.data;
        if Self::run_of_operator(rides, "italo") {
            let link = crate::links::italo_link(
                &data.italo_stations,
                first,
                last,
                self.settings.age_group,
            )?;
            let search = crate::links::italo_site_search(&data.italo_stations, first, last);
            return Some((link, search));
        }
        let operator = own_ticket_operator(rides)?;
        let with_search = |link: Option<(btt_model::BookingLink, btt_model::SiteSearch)>| {
            link.map(|(link, search)| (link, Some(search)))
        };
        match operator.id {
            "european-sleeper" => with_search(crate::links::european_sleeper_link(
                &data.european_sleeper_stations,
                first,
                last,
            )),
            "snalltaget" => with_search(crate::links::snalltaget_link(
                &data.snalltaget_stations,
                first,
                last,
            )),
            "regiojet" => with_search(crate::links::regiojet_link(
                &data.regiojet_stations,
                first,
                last,
            )),
            "eurostar" => with_search(crate::links::eurostar_link(
                &data.eurostar_stations,
                first,
                last,
            )),
            "iryo" => crate::links::sbb_link(
                &data.sbb_abroad_stations,
                &data.sbb_abroad_stations,
                first,
                last,
            )
            .map(|link| (link, None)),
            _ => None,
        }
    }

    /// Returns the link to the site of the country a run leaves from, or to DB for a run across
    /// a border that the site of its country cannot take, with the search to run there when
    /// the link does not name it.
    fn country_site_link(
        &self,
        rides: &[RideInput],
    ) -> Option<(btt_model::BookingLink, Option<btt_model::SiteSearch>)> {
        // The sites of countries do not sell the trains of operators with their own shops.
        if own_ticket_operator(rides).is_some() {
            return None;
        }
        let (first, last) = (&rides[0], &rides[rides.len() - 1]);
        let data = self.data;
        let departure_country = first.from_country.as_deref();
        let domestic = rides.iter().all(|ride| {
            ride.from_country.as_deref() == departure_country
                && ride.to_country.as_deref() == departure_country
        });
        let db_link = || {
            crate::links::db_link(
                &data.db_stations,
                first,
                last,
                self.settings.class,
                self.settings.age_group,
            )
        };
        let with_search = |link: Option<(btt_model::BookingLink, btt_model::SiteSearch)>| {
            link.map(|(link, search)| (link, Some(search)))
        };
        let departure_site_link = match (departure_country, domestic) {
            (Some("DE"), _) | (Some("CZ"), true) => db_link(),
            (Some("CH"), _) => {
                let stations = |country: Option<&str>| {
                    if country == Some("CH") {
                        &data.sbb_stations
                    } else {
                        &data.sbb_abroad_stations
                    }
                };
                crate::links::sbb_link(
                    &data.sbb_stations,
                    stations(last.to_country.as_deref()),
                    first,
                    last,
                )
            }
            (Some("AT"), _) => crate::links::obb_link(&data.obb_stations, first, last),
            (Some("IT"), _) => crate::links::trenitalia_link(
                &data.trenitalia_stations,
                first,
                last,
                self.settings.age_group,
            ),
            // The SBB site sells French trips, while SNCF Connect has no link to a search and
            // blocks scripted clients.
            (Some("FR"), true) => crate::links::sbb_link(
                &data.sbb_abroad_stations,
                &data.sbb_abroad_stations,
                first,
                last,
            ),
            (Some("NL"), true) => crate::links::ns_link(&data.ns_stations, first, last),
            (Some("PL"), true) => {
                return with_search(crate::links::koleo_link(&data.koleo_stations, first, last));
            }
            (Some("DK"), true) => {
                return with_search(crate::links::dsb_link(&data.dsb_stations, first, last));
            }
            (Some("SE"), true) => {
                return with_search(crate::links::sj_link(&data.sj_stations, first, last));
            }
            (Some("HU"), true) => {
                return with_search(crate::links::mav_link(&data.mav_stations, first, last));
            }
            (Some("ES"), true) => {
                return with_search(crate::links::renfe_link(&data.renfe_stations, first, last));
            }
            _ => None,
        };
        departure_site_link
            .or_else(|| if domestic { None } else { db_link() })
            .map(|link| (link, None))
    }

    /// Builds the strategy of single tickets for every ride.
    fn single_ticket_strategy(&self, singles_by_day: &[Cover]) -> TicketStrategy {
        let mut builder = StrategyBuilder::new(self.rides.len());
        for cover in singles_by_day {
            self.add_single_tickets(&mut builder, &cover.runs);
        }
        self.add_accommodation(&mut builder, |_| false);
        let mut notes = Vec::new();
        if self.settings.class == TravelClass::First {
            notes.push("Single ticket prices are estimates for second class.".into());
        }
        builder.finish(StrategyKind::SingleTickets, "Single tickets".into(), notes)
    }

    // The strategy reads best as one pass over the days, so it stays in one function.
    #[allow(clippy::too_many_lines)]
    /// Builds the strategy of a pass with single tickets for the rides it does not cover.
    fn pass_strategy(&self, choice: &PassChoice<'_>, singles_by_day: &[Cover]) -> TicketStrategy {
        let covered = self.pass_covers_ride();
        let on_pass = |index: usize| {
            covered[index]
                && self.rides[index].departure_date >= choice.start
                && self.rides[index].arrival_date <= choice.end
        };
        let mut builder = StrategyBuilder::new(self.rides.len());
        let pass_item = builder.reserve_id();
        let mut pass_rides = Vec::new();
        let mut travel_days = Vec::new();
        let mut notes = Vec::new();
        for (day_index, day) in self.days.iter().enumerate() {
            if choice.chosen_days.contains(&day_index) {
                travel_days.push(self.rides[day.start].departure_date);
                let remaining = self.cover(day, |index| !on_pass(index));
                self.add_single_tickets(&mut builder, &remaining.runs);
                for index in day.clone().filter(|&index| on_pass(index)) {
                    pass_rides.push(index);
                    builder.cover_with(index, pass_item);
                    if let Some(rule) = self.seat_rule(index) {
                        let ride = &self.rides[index];
                        builder.add(
                            TicketProduct::Reservation,
                            format!(
                                "Seat reservation, {} {}",
                                ride.train_label.category, ride.train_label.number
                            ),
                            format!(
                                "{} to {} on {}",
                                ride.from_name,
                                ride.to_name,
                                ride.departure_date.format("%-d %b %Y")
                            ),
                            &[index],
                            fee_price(rule, self.settings.class),
                            Some(shop_link(ride.from_country.as_deref())),
                        );
                    }
                    // A night train on a pass day also needs its accommodation, which every
                    // strategy adds below.
                }
            } else {
                self.add_single_tickets(&mut builder, &singles_by_day[day_index].runs);
            }
        }
        self.add_accommodation(&mut builder, |index| pass_rides.contains(&index));
        let age_group = PriceAgeGroup::of(self.settings.age_group).unwrap_or(PriceAgeGroup::Adult);
        let title = format!(
            "Interrail Global Pass, {}, {}, {}",
            choice.pass.title(),
            class_text(self.settings.class),
            age_text(age_group)
        );
        let detail = if matches!(choice.pass.kind, PassKind::Continuous) {
            format!(
                "Every day from {} to {}.",
                choice.start.format("%-d %b %Y"),
                choice.end.format("%-d %b %Y")
            )
        } else {
            let days: Vec<String> = travel_days
                .iter()
                .map(|day| day.format("%-d %b").to_string())
                .collect();
            format!(
                "Valid from {} to {}. Travel days: {}.",
                choice.start.format("%-d %b %Y"),
                choice.end.format("%-d %b %Y"),
                if days.is_empty() {
                    "none, no day pays".to_owned()
                } else {
                    days.join(", ")
                }
            )
        };
        let price = Price {
            low: choice.price,
            high: choice.price,
            kind: PriceKind::Formula,
            source: PriceSource {
                name: "Interrail price list".into(),
                note: "The prices of the pass list below. The Interrail shop has the final word, \
                       because its prices can differ by promotion."
                    .into(),
                as_of: Some(self.data.passes.as_of),
                page: Some(self.data.passes.source.clone()),
            },
            original: None,
        };
        let continuous_days = if matches!(choice.pass.kind, PassKind::Continuous) {
            let mut day = choice.start;
            let mut days = Vec::new();
            while day <= choice.end {
                days.push(day);
                match day.succ_opt() {
                    Some(next) => day = next,
                    None => break,
                }
            }
            days
        } else {
            travel_days
        };
        builder.fill(
            pass_item,
            TicketProduct::InterrailPass {
                pass_id: choice.pass.id.clone(),
                first_valid_date: choice.start,
                travel_days: continuous_days,
            },
            title.clone(),
            detail,
            &pass_rides,
            price,
            Some(pass_link()),
        );
        if choice.chosen_days.is_empty() {
            notes.push("No day of the trip pays on this pass.".into());
        }
        builder.finish(StrategyKind::Pass, title.into(), notes)
    }
}

/// Collects the tickets of one strategy and which ticket covers which ride.
struct StrategyBuilder {
    /// The tickets, in the order they were added. A reserved id holds a placeholder until it is
    /// filled.
    items: Vec<Option<TicketItem>>,
    /// For each ride, the ids of the tickets that cover it.
    covers: Vec<Vec<u16>>,
}

impl StrategyBuilder {
    /// Starts a strategy for a number of rides.
    fn new(ride_count: usize) -> Self {
        Self {
            items: Vec::new(),
            covers: vec![Vec::new(); ride_count],
        }
    }

    /// Reserves the id of a ticket that is filled in later.
    fn reserve_id(&mut self) -> u16 {
        self.items.push(None);
        u16::try_from(self.items.len() - 1).unwrap_or(u16::MAX)
    }

    /// Marks a ride as covered by a ticket.
    fn cover_with(&mut self, ride: usize, ticket_id: u16) {
        self.covers[ride].push(ticket_id);
    }

    /// Fills a reserved ticket. The rides were already marked as covered.
    #[allow(clippy::too_many_arguments)]
    fn fill(
        &mut self,
        id: u16,
        product: TicketProduct,
        title: String,
        detail: String,
        rides: &[usize],
        price: Price,
        booking_link: Option<btt_model::BookingLink>,
    ) {
        self.items[usize::from(id)] = Some(TicketItem {
            id,
            product,
            title: title.into(),
            detail: detail.into(),
            ride_indices: rides
                .iter()
                .map(|&ride| u16::try_from(ride).unwrap_or(u16::MAX))
                .collect(),
            price,
            booking_link,
            live_quote_request: None,
        });
    }

    /// Adds a ticket that covers rides and marks them as covered by it.
    fn add(
        &mut self,
        product: TicketProduct,
        title: String,
        detail: String,
        rides: &[usize],
        price: Price,
        booking_link: Option<btt_model::BookingLink>,
    ) -> u16 {
        let id = self.reserve_id();
        for &ride in rides {
            self.cover_with(ride, id);
        }
        self.fill(id, product, title, detail, rides, price, booking_link);
        id
    }

    /// Finishes the strategy.
    fn finish(self, kind: StrategyKind, title: SmolStr, notes: Vec<SmolStr>) -> TicketStrategy {
        let items: Vec<TicketItem> = self.items.into_iter().flatten().collect();
        let total = total_of(items.iter().map(|item| &item.price));
        TicketStrategy {
            kind,
            title,
            items,
            total,
            covers: self.covers,
            notes,
        }
    }
}
