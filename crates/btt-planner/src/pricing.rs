//! Prices a planned journey, so the planner can show what a journey costs next to its time.

use btt_model::{EuroRange, Journey, LegPlanStation};

use btt_timetable::RaptorRouteIndex;
use smol_str::SmolStr;

/// Which tickets a leg is searched with: single tickets, or the travel days of a pass. A pass
/// covers a ride for nothing but what it charges on top, such as a reservation, and a ride
/// the pass does not take costs its fare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PriceMode {
    /// Every ride costs its fare.
    Single,
    /// Rides the pass covers cost what it charges on top, and the search counts the travel
    /// days the journey uses.
    Pass,
}

/// What rides of one class cost on the days of a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RidePassRow {
    /// Whether the pass covers the trains of the class.
    pub takes: bool,
    /// What a ride costs on the pass, in whole euro cents, such as a reservation or the pass
    /// price of a night accommodation. Zero when the pass covers the ride for nothing.
    pub cents: u32,
}

/// The pass prices of the classes of rides of one bundle, built once beside the fare rows.
#[derive(Debug, Clone, Default)]
pub struct RidePassTable {
    /// One row per class, in the order the classes were given.
    pub rows: Vec<RidePassRow>,
}

/// Prices the rides of a planned journey, with the same sources the ticket plan uses.
///
/// The planner calls this as a generic parameter, so every call is a static dispatch. The
/// trait lives in the planner and speaks the model's types only, so implementing it needs no
/// fare crate and the planner stays free of `btt-fares`.
pub trait JourneyPricer: Send + Sync + 'static {
    /// Returns the estimated price of the single tickets and the night accommodation of a
    /// journey, or `None` when no price is known for its trains.
    fn journey_price(&self, journey: &Journey, stations: &[LegPlanStation]) -> Option<EuroRange>;

    /// Returns the price of a journey on the days of a pass: what the pass does not cover,
    /// such as reservations, the night accommodation and the fares of trains it does not take.
    /// Returns `None` when no pass covers the journey.
    fn journey_price_with_pass(
        &self,
        journey: &Journey,
        stations: &[LegPlanStation],
    ) -> Option<EuroRange> {
        let _ = (journey, stations);
        None
    }

    /// Returns the prices of the classes of rides the planner searches over, or `None` when
    /// the planner should search without prices. The rows come from the same model that
    /// prices tickets, and are a snapshot: what the model learns later does not change them.
    fn price_table(&self, classes: &[RidePriceClass]) -> Option<RidePriceTable>;

    /// Returns what rides of the classes cost on the days of a pass, or `None` when no pass
    /// covers any of them. The rows follow the same rules the ticket optimizer applies to a
    /// pass holder.
    fn pass_price_table(&self, classes: &[RidePriceClass]) -> Option<RidePassTable> {
        let _ = classes;
        None
    }
}

/// A pricer that knows no prices, for backends and tests that plan without fares.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoPrice;

impl JourneyPricer for NoPrice {
    fn journey_price(&self, _: &Journey, _: &[LegPlanStation]) -> Option<EuroRange> {
        None
    }

    fn price_table(&self, _: &[RidePriceClass]) -> Option<RidePriceTable> {
        None
    }
}

/// A class of rides that share one price row: their operator, their category, whether their
/// trains have berths and the time zone of the station they board at, which says the country.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RidePriceClass {
    /// The operator as the app names it, such as `db-fernverkehr`. Empty when not known.
    pub operator: SmolStr,
    /// The category of the train, such as "ICE".
    pub category: SmolStr,
    /// Whether the trains have couchettes or sleepers, which makes them a night kind.
    pub offers_berths: bool,
    /// The IANA time zone of the boarding station.
    pub time_zone: SmolStr,
}

/// The log price of the rides of one class, as the search computes it. The price of a ride of
/// `kilometres` is `exp(constant + distance·u + curve·u² + base·b)`, where `u` is the log of
/// the kilometres over the reference distance and `b` fades with the distance the way the
/// price model's base term does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RidePriceRow {
    /// The log price at the reference distance, with every offset of the class.
    pub constant: f32,
    /// The coefficient of the log distance, the sum of every slope of the class.
    pub distance: f32,
    /// The coefficient of the squared log distance.
    pub curve: f32,
    /// The coefficient of the base term, which stands in for a base fare.
    pub base: f32,
    /// The kilometres the country's tariff shifts the distance by.
    pub shift: f32,
}

/// The price rows of the classes of rides of one bundle, built once from the model that
/// prices tickets.
#[derive(Debug, Clone, Default)]
pub struct RidePriceTable {
    /// One row per class, in the order the classes were given.
    pub rows: Vec<RidePriceRow>,
    /// The distance the log price divides by, in kilometres.
    pub reference_kilometres: f32,
}

/// The prices the search adds to the cost of a ride: the row of each class of ride, the class
/// of each boarding position, the distance between the stops of each route and the operator of
/// each route. Built once when the bundle loads, so the search reads flat arrays.
#[derive(Debug, Clone, Default)]
pub struct RidePrices {
    /// Where each route's boarding positions start in `row_of_boarding` and `stop_distances`.
    route_offsets: Vec<u32>,
    /// The row of each boarding position of each route, or `NO_ROW` when the ride has no
    /// price.
    row_of_boarding: Vec<u16>,
    /// The distance from the first stop to each stop of each route, in units of 100 metres.
    stop_distances: Vec<u16>,
    /// The operator of each route, as an index into the classes, for rides that continue a
    /// ticket run.
    operator_of_route: Vec<u16>,
    /// One row per class of ride.
    rows: Vec<RidePriceRow>,
    /// What rides of each class cost on the days of a pass, in the same order as the rows.
    pass_rows: Vec<RidePassRow>,
    /// The time zone of each class, as an index into the zones of `utc_offset_minutes`.
    zone_of_row: Vec<u16>,
    /// The offset of local time from UTC in minutes, per time zone and day of the bundle,
    /// flattened as `zone * validity_days + day`. A pass counts a ride on its local date.
    utc_offset_minutes: Vec<i16>,
    /// Number of days the bundle runs, which is the stride of `utc_offset_minutes`.
    validity_days: u16,
    /// The distance the log price divides by, in kilometres.
    reference_kilometres: f32,
    /// Where the prices of each row start in `curve_cents`, with one more entry for the end.
    curve_start: Vec<u32>,
    /// The price of a ride of a whole number of kilometres in euro cents, for each row from
    /// zero kilometres on. A row covers twice its longest route, which holds a ride that
    /// continues a run. The search looks prices up here, because the formula costs a logarithm
    /// and an exponential and the search asks for millions of prices.
    curve_cents: Vec<u32>,
}

/// The most a price in a curve can be, in euro cents. It keeps the interpolation in `u32`.
const MAXIMUM_CURVE_CENTS: u32 = 100_000_000;

/// The row of rides with no price, which count nothing.
const NO_ROW: u16 = u16::MAX;

/// Returns what rides of the classes cost on the days of a pass. A pricer without pass prices
/// gives rows that no pass takes, which leaves the search in `Single` mode.
fn pass_rows_of(pricer: &impl JourneyPricer, classes: &[RidePriceClass]) -> Vec<RidePassRow> {
    pricer.pass_price_table(classes).map_or_else(
        || {
            vec![
                RidePassRow {
                    takes: false,
                    cents: 0,
                };
                classes.len()
            ]
        },
        |table| table.rows,
    )
}

/// Returns the position of a time zone in a list of them, adding it when it is new. Returns
/// `None` when the list outgrows the index type.
fn zone_index(zones: &mut Vec<SmolStr>, time_zone: &SmolStr) -> Option<u16> {
    let position = zones.iter().position(|zone| zone == time_zone);
    let position = position.unwrap_or_else(|| {
        zones.push(time_zone.clone());
        zones.len() - 1
    });
    u16::try_from(position).ok()
}

/// Returns the price of a ride of a distance in whole euro cents, from the formula.
fn exact_cents(row: RidePriceRow, reference_kilometres: f32, kilometres: f64) -> u32 {
    // Prices stay far below the range of `u32` cents, and a price that is not a number counts
    // as nothing.
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let cents = unrounded_cents(row, reference_kilometres, kilometres).round() as u32;
    cents.min(MAXIMUM_CURVE_CENTS)
}

/// Returns the curves of the rows, as where each row's prices start and the prices. A row's
/// curve reaches twice its longest route, in whole kilometres and one more, so that a price
/// between two kilometres always has both of them.
fn price_curves(
    rows: &[RidePriceRow],
    reference_kilometres: f32,
    longest_units_of_row: &[u32],
) -> (Vec<u32>, Vec<u32>) {
    let mut starts = Vec::with_capacity(rows.len() + 1);
    let mut cents = Vec::new();
    for (index, &row) in rows.iter().enumerate() {
        starts.push(u32::try_from(cents.len()).unwrap_or(u32::MAX));
        let longest_units = longest_units_of_row.get(index).copied().unwrap_or(0);
        let kilometres = (2 * longest_units).div_ceil(10) + 1;
        cents.extend(
            (0..=kilometres)
                .map(|kilometre| exact_cents(row, reference_kilometres, f64::from(kilometre))),
        );
    }
    starts.push(u32::try_from(cents.len()).unwrap_or(u32::MAX));
    (starts, cents)
}

/// Returns the price of a ride of a distance in euro cents, before it is rounded.
fn unrounded_cents(row: RidePriceRow, reference_kilometres: f32, kilometres: f64) -> f64 {
    let shift = f64::from(row.shift);
    let distance = ((kilometres + shift) / (f64::from(reference_kilometres) + shift)).ln();
    let base = 10.0 / (kilometres + 5.0);
    let log_price = f64::from(row.constant)
        + f64::from(row.distance) * distance
        + f64::from(row.curve) * distance * distance
        + f64::from(row.base) * base;
    log_price.exp() * 100.0
}

impl RidePrices {
    /// Builds the prices of all rides of a bundle: the classes of its routes' boarding
    /// positions, filled with the rows the pricer gives. Returns `None` when the pricer knows
    /// no prices, in which case the search runs without them.
    ///
    /// The distances are straight lines between consecutive stops, which matches how tickets
    /// are priced closely enough for a search that the ranking corrects.
    pub fn build(
        bundle: &btt_timetable::LoadedTimetableBundle,
        operators: &crate::operators::OperatorTable,
        routes_with_berths: &[bool],
        pricer: &impl JourneyPricer,
    ) -> Option<Self> {
        let mut route_offsets: Vec<u32> = Vec::with_capacity(bundle.route_count());
        let mut row_of_boarding: Vec<u16> = Vec::new();
        let mut stop_distances: Vec<u16> = Vec::new();
        let mut operator_of_route: Vec<u16> = Vec::with_capacity(bundle.route_count());
        let mut operators_seen: rustc_hash::FxHashMap<SmolStr, u16> =
            rustc_hash::FxHashMap::default();
        let mut classes: Vec<RidePriceClass> = Vec::new();
        let mut classes_seen: rustc_hash::FxHashMap<RidePriceClass, u16> =
            rustc_hash::FxHashMap::default();
        let mut zones: Vec<SmolStr> = Vec::new();
        let mut zone_of_row: Vec<u16> = Vec::new();
        let mut longest_units_of_row: Vec<u32> = Vec::new();
        for route_index in 0..bundle.route_count() {
            let index = RaptorRouteIndex(u32::try_from(route_index).unwrap_or(u32::MAX));
            let route = bundle.route(index);
            // The label of the route's trains, from its first trip.
            let (category, _, agency) =
                bundle.trip_train_label(btt_timetable::BundleTripIndex(route.first_trip_index));
            let operator = operators.operator_of(agency, category);
            let offers_berths = routes_with_berths
                .get(route_index)
                .copied()
                .unwrap_or(false);
            let operator_index = if let Some(&known) = operators_seen.get(operator.as_str()) {
                known
            } else {
                let next = u16::try_from(operators_seen.len()).unwrap_or(u16::MAX);
                operators_seen.insert(operator.clone(), next);
                next
            };
            operator_of_route.push(operator_index);
            let route_start = stop_distances.len();
            route_offsets.push(u32::try_from(route_start).unwrap_or(u32::MAX));
            let mut units: u32 = 0;
            let mut previous_station = None;
            for position in 0..route.stop_count {
                let station = bundle.pattern_stop(&route, position).station();
                if let Some(previous) = previous_station.replace(station) {
                    let metres = bundle
                        .station_coordinates(previous)
                        .haversine_distance_meters(bundle.station_coordinates(station));
                    // The distance is far below the range of `u32` hundreds of metres.
                    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let hundreds = (metres / 100.0).round() as u32;
                    units = units.saturating_add(hundreds);
                }
                stop_distances.push(u16::try_from(units).unwrap_or(u16::MAX));
                // An operator the app cannot name gets no price and no price term.
                let row = if operator.is_empty() {
                    NO_ROW
                } else {
                    let class = RidePriceClass {
                        operator: operator.clone(),
                        category: category.into(),
                        offers_berths,
                        time_zone: bundle.station_time_zone(station).into(),
                    };
                    if let Some(&known) = classes_seen.get(&class) {
                        known
                    } else {
                        let next = u16::try_from(classes.len()).ok()?;
                        zone_of_row.push(zone_index(&mut zones, &class.time_zone)?);
                        classes.push(class.clone());
                        classes_seen.insert(class, next);
                        next
                    }
                };
                row_of_boarding.push(row);
            }
            // A ride on the route is no longer than the route, so the rows it uses need prices
            // up to its length.
            longest_units_of_row.resize(classes.len(), 0);
            for &row in row_of_boarding[route_start..]
                .iter()
                .filter(|&&row| row != NO_ROW)
            {
                let longest = &mut longest_units_of_row[usize::from(row)];
                *longest = (*longest).max(units);
            }
        }
        let table = pricer.price_table(&classes)?;
        let (curve_start, curve_cents) = price_curves(
            &table.rows,
            table.reference_kilometres,
            &longest_units_of_row,
        );
        let pass_rows = pass_rows_of(pricer, &classes);
        let validity_days = bundle.validity_days();
        Some(Self {
            route_offsets,
            row_of_boarding,
            stop_distances,
            operator_of_route,
            rows: table.rows,
            pass_rows,
            zone_of_row,
            utc_offset_minutes: utc_offsets(&zones, bundle.validity_start(), validity_days),
            validity_days,
            reference_kilometres: table.reference_kilometres,
            curve_start,
            curve_cents,
        })
    }

    /// Returns the row of the rides that board a route at a position, or `None` when they have
    /// no price.
    fn row_of(&self, raptor_route: RaptorRouteIndex, boarding_position: u16) -> Option<usize> {
        let offset = self.route_offsets[raptor_route.as_usize()] as usize;
        self.row_of_boarding
            .get(offset + usize::from(boarding_position))
            .filter(|&&row| row != NO_ROW)
            .map(|&row| usize::from(row))
    }

    /// Returns the travel day of a pass that a ride boarding a route at a position and a
    /// moment counts on, as local days since the bundle starts, or `None` when the pass does
    /// not take the ride. The day is the local date of the boarding station, the way the
    /// ticket optimizer counts it.
    pub(crate) fn pass_travel_day(
        &self,
        raptor_route: RaptorRouteIndex,
        boarding_position: u16,
        departure: u32,
    ) -> Option<u16> {
        let row = self.row_of(raptor_route, boarding_position)?;
        if !self.pass_rows.get(row)?.takes {
            return None;
        }
        let utc_day = (departure / MINUTES_PER_DAY).min(u32::from(self.validity_days) - 1);
        let zone = usize::from(self.zone_of_row[row]);
        let offset =
            self.utc_offset_minutes[zone * usize::from(self.validity_days) + utc_day as usize];
        // A zone west of UTC can put the first minutes of the bundle on the day before it,
        // which counts as its first day. No train of the bundle runs there.
        let local = i64::from(departure) + i64::from(offset);
        u16::try_from(local.div_euclid(i64::from(MINUTES_PER_DAY)).max(0)).ok()
    }

    /// Returns whether the pass covers the trains of any class the planner searches over.
    pub fn any_pass_takes(&self) -> bool {
        self.pass_rows.iter().any(|row| row.takes)
    }

    /// Returns the distance of a ride in units of 100 metres.
    fn distance_units(
        &self,
        raptor_route: RaptorRouteIndex,
        boarding_position: u16,
        alighting_position: u16,
    ) -> u32 {
        let offset = self.route_offsets[raptor_route.as_usize()] as usize;
        let from = u32::from(self.stop_distances[offset + usize::from(boarding_position)]);
        let to = u32::from(self.stop_distances[offset + usize::from(alighting_position)]);
        to.saturating_sub(from)
    }

    /// Returns the price of a distance in whole euro cents, rounded the way the plan shows
    /// prices. The curve of the row gives it between two whole kilometres, which differs from
    /// the formula by less than a cent for any ride long enough to matter, and a distance past
    /// the curve takes the formula.
    fn price_cents(&self, row_index: usize, distance_units: u32) -> u32 {
        let start = self.curve_start[row_index] as usize;
        let end = self.curve_start[row_index + 1] as usize;
        let whole_kilometres = (distance_units / 10) as usize;
        if start + whole_kilometres + 1 < end {
            let low = self.curve_cents[start + whole_kilometres];
            let high = self.curve_cents[start + whole_kilometres + 1];
            let tenths = distance_units % 10;
            return (low * (10 - tenths) + high * tenths + 5) / 10;
        }
        exact_cents(
            self.rows[row_index],
            self.reference_kilometres,
            f64::from(distance_units) / 10.0,
        )
    }

    /// Returns the price of a ride in whole euro cents, charged as the extra over the ride
    /// before it when the two continue a ticket run of the same operator. Rides without a
    /// price count nothing.
    pub(crate) fn ride_price_cents(
        &self,
        raptor_route: RaptorRouteIndex,
        boarding_position: u16,
        alighting_position: u16,
        previous: Option<btt_router::PreviousRide>,
        mode: PriceMode,
    ) -> u32 {
        let Some(row_index) = self.row_of(raptor_route, boarding_position) else {
            return 0;
        };
        let distance = self.distance_units(raptor_route, boarding_position, alighting_position);
        // On the days of a pass a covered ride costs what the pass charges on top, and a ride
        // the pass does not take costs its fare, as one ticket on its own.
        if mode == PriceMode::Pass {
            let pass_row = self.pass_rows[row_index];
            return if pass_row.takes {
                pass_row.cents
            } else {
                self.price_cents(row_index, distance)
            };
        }
        match previous {
            // A ticket covers rides of one operator as one run, so the second ride pays the
            // extra over the first.
            Some(previous)
                if self.operator_of_route[raptor_route.as_usize()]
                    == self.operator_of_route[previous.raptor_route.as_usize()] =>
            {
                let previous_distance = self.distance_units(
                    previous.raptor_route,
                    previous.boarding_position,
                    previous.alighting_position,
                );
                self.price_cents(row_index, previous_distance + distance)
                    .saturating_sub(self.price_cents(row_index, previous_distance))
            }
            _ => self.price_cents(row_index, distance),
        }
    }

    /// Returns what boarding a route at an earlier position costs at most on top of boarding
    /// it at a later one, for any stop after both, in whole euro cents. Returns `None` when no
    /// such bound is known.
    ///
    /// On single tickets that is the price of the stops in between, without the discount of a
    /// run. It holds only when both boardings are priced by the same class of ride, because a
    /// ride is priced by the class of the station it boards at, and a route that crosses a
    /// border changes class. A ride from the earlier class can cost more over the same
    /// distance than one from the later class, by more than the stops in between cost. On the
    /// days of a pass a covered ride costs the same from any stop, so an earlier boarding that
    /// the pass takes for no more than the later one costs nothing on top, and otherwise its
    /// whole charge bounds the extra.
    pub(crate) fn boarding_extension_cents(
        &self,
        raptor_route: RaptorRouteIndex,
        earlier: u16,
        later: u16,
        mode: PriceMode,
    ) -> Option<u32> {
        let (earlier_row, later_row) = match (
            self.row_of(raptor_route, earlier),
            self.row_of(raptor_route, later),
        ) {
            // A ride that has no price costs nothing from either stop.
            (None, _) => return Some(0),
            (Some(_), None) => return None,
            (Some(earlier_row), Some(later_row)) => (earlier_row, later_row),
        };
        if mode == PriceMode::Pass {
            let (earlier_pass, later_pass) =
                (self.pass_rows[earlier_row], self.pass_rows[later_row]);
            if earlier_pass.takes && later_pass.takes {
                return Some(if earlier_pass.cents <= later_pass.cents {
                    0
                } else {
                    earlier_pass.cents
                });
            }
        }
        (earlier_row == later_row)
            .then(|| self.ride_price_cents(raptor_route, earlier, later, None, mode))
    }
}

/// Minutes in a day.
const MINUTES_PER_DAY: u32 = 1440;

/// Returns the offset of local time from UTC in minutes for each time zone and each day of a
/// bundle, flattened as `zone * validity_days + day`. The offset of a day is the one at noon
/// UTC, so a ride in the hour around a change to or from summer time can count on the wrong
/// date. A zone that does not parse counts as UTC.
fn utc_offsets(
    zones: &[SmolStr],
    validity_start: chrono::NaiveDate,
    validity_days: u16,
) -> Vec<i16> {
    use chrono::{Offset, TimeZone};

    let mut offsets = Vec::with_capacity(zones.len() * usize::from(validity_days));
    for zone in zones {
        let time_zone = zone.parse::<chrono_tz::Tz>().ok();
        for day in 0..validity_days {
            let noon = (validity_start + chrono::Days::new(u64::from(day)))
                .and_hms_opt(12, 0, 0)
                .map(|moment| moment.and_utc());
            let seconds = match (time_zone, noon) {
                (Some(time_zone), Some(noon)) => time_zone
                    .offset_from_utc_datetime(&noon.naive_utc())
                    .fix()
                    .local_minus_utc(),
                _ => 0,
            };
            offsets.push(i16::try_from(seconds / 60).unwrap_or(0));
        }
    }
    offsets
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns the row of a ride whose price grows with the distance, at a price of
    /// `e^constant` euro for a ride of the reference distance.
    fn row(constant: f32) -> RidePriceRow {
        RidePriceRow {
            constant,
            distance: 0.7,
            curve: 0.0,
            base: 0.0,
            shift: 10.0,
        }
    }

    /// The prices of one route of four stops, 50 kilometres apart. The first two stops are in
    /// an expensive class and the last two in a cheap one, like a route that crosses a border.
    fn border_prices() -> RidePrices {
        let rows = vec![row(3.0), row(1.0)];
        // The route is 150 kilometres long, which is 1500 units of 100 metres.
        let (curve_start, curve_cents) = price_curves(&rows, 100.0, &[1500, 1500]);
        RidePrices {
            route_offsets: vec![0],
            row_of_boarding: vec![0, 0, 1, 1],
            stop_distances: vec![0, 500, 1000, 1500],
            operator_of_route: vec![0],
            rows,
            pass_rows: vec![
                RidePassRow {
                    takes: true,
                    cents: 700,
                },
                RidePassRow {
                    takes: true,
                    cents: 900,
                },
            ],
            zone_of_row: vec![0, 0],
            utc_offset_minutes: vec![0],
            validity_days: 1,
            reference_kilometres: 100.0,
            curve_start,
            curve_cents,
        }
    }

    const ROUTE: RaptorRouteIndex = RaptorRouteIndex(0);

    #[test]
    fn the_extension_within_a_class_is_the_price_of_the_stops_between() {
        let prices = border_prices();
        let extension = prices.boarding_extension_cents(ROUTE, 0, 1, PriceMode::Single);
        assert_eq!(extension, Some(prices.price_cents(0, 500)));
        // The same holds in the cheap class.
        assert_eq!(
            prices.boarding_extension_cents(ROUTE, 2, 3, PriceMode::Single),
            Some(prices.price_cents(1, 500))
        );
    }

    #[test]
    fn no_extension_is_known_across_a_change_of_class() {
        let prices = border_prices();
        assert_eq!(
            prices.boarding_extension_cents(ROUTE, 1, 2, PriceMode::Single),
            None
        );
        assert_eq!(
            prices.boarding_extension_cents(ROUTE, 0, 3, PriceMode::Single),
            None
        );
        // The price of the stops in between would not bound the extra here. Riding from the
        // second stop to the last in the expensive class costs more over the extra 50
        // kilometres than the 50 kilometres between the second and third stop cost alone.
        let from_earlier = prices.price_cents(0, 1000);
        let from_later = prices.price_cents(1, 500);
        let between = prices.price_cents(0, 500);
        assert!(from_earlier - from_later > between);
    }

    #[test]
    fn on_pass_days_a_covered_ride_costs_the_same_from_any_stop() {
        let prices = border_prices();
        // The earlier class charges 7 euro and the later 9 euro, so boarding earlier costs
        // nothing on top.
        assert_eq!(
            prices.boarding_extension_cents(ROUTE, 1, 2, PriceMode::Pass),
            Some(0)
        );
    }

    #[test]
    fn the_curve_agrees_with_the_formula_to_a_cent() {
        let prices = border_prices();
        for row_index in 0..2 {
            for units in (50..=3000).step_by(7) {
                let row = prices.rows[row_index];
                let exact = exact_cents(row, 100.0, f64::from(units) / 10.0);
                let looked_up = prices.price_cents(row_index, units);
                assert!(
                    looked_up.abs_diff(exact) <= 1,
                    "row {row_index} at {units} units: {looked_up} against {exact}"
                );
            }
        }
    }

    #[test]
    fn a_distance_past_the_curve_takes_the_formula() {
        let prices = border_prices();
        // The curve reaches twice the longest route, which is 300 kilometres.
        let units = 5000;
        assert_eq!(
            prices.price_cents(0, units),
            exact_cents(prices.rows[0], 100.0, 500.0)
        );
    }
}
