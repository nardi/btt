//! Finds the cheapest tickets for the rides of a trip.
//!
//! The problem splits by day. For one day, the cheapest single tickets are a shortest path over
//! the rides of the day, where a ticket covers a run of consecutive rides. For a pass, a day is
//! worth what its single tickets cost minus what the pass days cost in reservations. A flexi
//! pass takes the days of the highest worth, and a continuous pass takes every day that pays.
//! The start of the pass is tried at every day of the trip.

use std::ops::Range;

use btt_fares::{
    FareData, PassKind, PassType, PriceAgeGroup, PriceBook, ReservationRule, RideFacts, RuleScope,
};
use btt_model::{
    AgeGroup, EuroCents, Price, PriceKind, PriceSource, PriceTotal, StrategyKind, TicketItem,
    TicketPreferences, TicketProduct, TicketStrategy, TravelClass,
};
use chrono::NaiveDate;
use smol_str::SmolStr;

use crate::links::{pass_link, shop_link};
use crate::rides::RideInput;

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
                                category: &ride.train_label.category,
                                distance_kilometres: ride.distance_kilometres,
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
                    .pass_accepted(&ride.train_label.category)
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
            let cost_on_pass = remaining + reservations;
            let singles = singles_by_day[day_index].middle;
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

    /// Adds the night accommodation of every ride that has a rule to a strategy.
    fn add_accommodation(&self, builder: &mut StrategyBuilder) {
        for index in 0..self.rides.len() {
            if let Some(rule) = self.night_rule(index) {
                let ride = &self.rides[index];
                builder.add(
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
                    fee_price(rule, self.settings.class),
                    Some(shop_link(ride.from_country.as_deref())),
                );
            }
        }
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
            builder.add(
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
                Some(self.single_ticket_link(rides)),
            );
        }
    }

    /// Returns the link to buy a ticket for a run of rides. Italo trains are sold on the Italo
    /// site, and the others at the shop of the country of the first ride.
    fn single_ticket_link(&self, rides: &[RideInput]) -> btt_model::BookingLink {
        let all_italo = rides
            .iter()
            .all(|ride| ride.train_label.category.eq_ignore_ascii_case("Italo"));
        let italo = all_italo
            .then(|| {
                crate::links::italo_link(
                    &self.data.italo_stations,
                    &rides[0],
                    &rides[rides.len() - 1],
                    self.settings.age_group,
                )
            })
            .flatten();
        italo.unwrap_or_else(|| shop_link(rides[0].from_country.as_deref()))
    }

    /// Builds the strategy of single tickets for every ride.
    fn single_ticket_strategy(&self, singles_by_day: &[Cover]) -> TicketStrategy {
        let mut builder = StrategyBuilder::new(self.rides.len());
        for cover in singles_by_day {
            self.add_single_tickets(&mut builder, &cover.runs);
        }
        self.add_accommodation(&mut builder);
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
        self.add_accommodation(&mut builder);
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
    ) {
        let id = self.reserve_id();
        for &ride in rides {
            self.cover_with(ride, id);
        }
        self.fill(id, product, title, detail, rides, price, booking_link);
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
