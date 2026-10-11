//! Prices a planned journey, so the planner can show what a journey costs next to its time.

use btt_model::{EuroRange, Journey, LegPlanStation};

use btt_timetable::RaptorRouteIndex;
use smol_str::SmolStr;

/// Prices the rides of a planned journey, with the same sources the ticket plan uses.
///
/// The planner calls this as a generic parameter, so every call is a static dispatch. The
/// trait lives in the planner and speaks the model's types only, so implementing it needs no
/// fare crate and the planner stays free of `btt-fares`.
pub trait JourneyPricer: Send + Sync + 'static {
    /// Returns the estimated price of the single tickets and the night accommodation of a
    /// journey, or `None` when no price is known for its trains.
    fn journey_price(&self, journey: &Journey, stations: &[LegPlanStation]) -> Option<EuroRange>;

    /// Returns the prices of the classes of rides the planner searches over, or `None` when
    /// the planner should search without prices. The rows come from the same model that
    /// prices tickets, and are a snapshot: what the model learns later does not change them.
    fn price_table(&self, classes: &[RidePriceClass]) -> Option<RidePriceTable>;
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
    /// The distance the log price divides by, in kilometres.
    reference_kilometres: f32,
}

/// The row of rides with no price, which count nothing.
const NO_ROW: u16 = u16::MAX;

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
            route_offsets.push(u32::try_from(stop_distances.len()).unwrap_or(u32::MAX));
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
                        classes.push(class.clone());
                        classes_seen.insert(class, next);
                        next
                    }
                };
                row_of_boarding.push(row);
            }
        }
        let table = pricer.price_table(&classes)?;
        Some(Self {
            route_offsets,
            row_of_boarding,
            stop_distances,
            operator_of_route,
            rows: table.rows,
            reference_kilometres: table.reference_kilometres,
        })
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
    /// prices.
    fn price_cents(&self, row: RidePriceRow, distance_units: u32) -> u32 {
        let kilometres = f64::from(distance_units) / 10.0;
        let shift = f64::from(row.shift);
        let distance = ((kilometres + shift) / (f64::from(self.reference_kilometres) + shift)).ln();
        let base = 10.0 / (kilometres + 5.0);
        let log_price = f64::from(row.constant)
            + f64::from(row.distance) * distance
            + f64::from(row.curve) * distance * distance
            + f64::from(row.base) * base;
        // Prices stay far below the range of `u32` cents.
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let cents = (log_price.exp() * 100.0).round() as u32;
        cents
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
    ) -> u32 {
        let offset = self.route_offsets[raptor_route.as_usize()] as usize;
        let Some(&row_index) = self
            .row_of_boarding
            .get(offset + usize::from(boarding_position))
        else {
            return 0;
        };
        if row_index == NO_ROW {
            return 0;
        }
        let row = self.rows[usize::from(row_index)];
        let distance = self.distance_units(raptor_route, boarding_position, alighting_position);
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
                self.price_cents(row, previous_distance + distance)
                    .saturating_sub(self.price_cents(row, previous_distance))
            }
            _ => self.price_cents(row, distance),
        }
    }

    /// Returns the price of a ride in whole euro cents, without the discount of a run.
    pub(crate) fn full_price_cents(
        &self,
        raptor_route: RaptorRouteIndex,
        boarding_position: u16,
        alighting_position: u16,
    ) -> u32 {
        self.ride_price_cents(raptor_route, boarding_position, alighting_position, None)
    }
}
