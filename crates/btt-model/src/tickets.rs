//! The tickets a trip needs, as the optimizer reports them.

use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use specta::Type;
use thiserror::Error;

use crate::itinerary::TripPlaceId;
use crate::plan::{LayoutLink, TrainLabel};
use crate::time::UnixTimeMinutes;
use crate::trip::{PassDays, TravelClass};

/// An amount of money in euro cents.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Type,
)]
#[serde(transparent)]
pub struct EuroCents(pub u32);

impl EuroCents {
    /// No money.
    pub const ZERO: Self = Self(0);

    /// Rounds an amount in euro to whole cents. A negative amount becomes zero.
    pub fn from_euro(euro: f64) -> Self {
        // The clamp keeps the rounded value within `u32`.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Self((euro * 100.0).round().clamp(0.0, f64::from(u32::MAX)) as u32)
    }

    /// Returns the amount in euro.
    pub fn euro(self) -> f64 {
        f64::from(self.0) / 100.0
    }

    /// Adds two amounts, saturating at the largest amount.
    #[must_use]
    pub fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

/// How sure a price is, from the most to the least. The order is the order of confidence, so the
/// lowest confidence of several prices is their maximum.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Type,
)]
pub enum PriceKind {
    /// A published price list or a function of the distance. It is exact for the fare it names.
    Formula,
    /// A quote from an operator's interface for a train and a date.
    Live,
    /// A rough estimate with a lowest and a highest price.
    Range,
}

/// Where a price comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct PriceSource {
    /// Name of the source, such as "Interrail price list".
    pub name: SmolStr,
    /// What the source says about how the price was found, for the tooltip.
    pub note: SmolStr,
    /// The day the source was valid or fetched.
    pub as_of: Option<NaiveDate>,
    /// A page where the traveller can check the price.
    pub page: Option<SmolStr>,
}

/// A price with the confidence in it and its source.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct Price {
    /// The lowest price the source allows. Equal to `high` for a formula.
    pub low: EuroCents,
    /// The highest price the source allows.
    pub high: EuroCents,
    /// How sure the price is.
    pub kind: PriceKind,
    /// Where the price comes from.
    pub source: PriceSource,
    /// The price in the currency the source gave it in, when that is not the euro.
    #[serde(default)]
    pub original: Option<ForeignAmount>,
}

/// An amount in a currency other than the euro.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct ForeignAmount {
    /// The amount in hundredths of the currency, such as 5000 for 50 francs.
    pub hundredths: u32,
    /// ISO code of the currency, such as `CHF`.
    pub currency: SmolStr,
}

impl Price {
    /// Returns the price the optimizer decides with, which is the middle of the range.
    pub fn middle(&self) -> EuroCents {
        EuroCents(self.low.0.midpoint(self.high.0))
    }
}

/// A sum of prices with the lowest confidence among its parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct PriceTotal {
    /// The sum of the lowest prices.
    pub low: EuroCents,
    /// The sum of the highest prices.
    pub high: EuroCents,
    /// The lowest confidence of any part.
    pub kind: PriceKind,
}

/// An estimated price with its low end, its median and its high end. The median is the number
/// the planner decides with, and the ends go along for display.
///
/// The fields stay private, so the only way to build a value is the constructor, which checks
/// that low is no more than median and median no more than high. Deserialization goes through
/// the constructor too, so a bad file cannot bypass the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Type)]
pub struct EuroRange {
    /// The low end of the estimate.
    low: EuroCents,
    /// The median of the estimate.
    median: EuroCents,
    /// The high end of the estimate.
    high: EuroCents,
}

impl<'de> Deserialize<'de> for EuroRange {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// The three ends as they travel, in the order the constructor checks them.
        #[derive(Deserialize)]
        struct EuroRangeEnds {
            low: EuroCents,
            median: EuroCents,
            high: EuroCents,
        }
        let ends = EuroRangeEnds::deserialize(deserializer)?;
        Self::new(ends.low, ends.median, ends.high).map_err(serde::de::Error::custom)
    }
}

/// Returned when the ends of a `EuroRange` are not in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("the range must hold low <= median <= high, got {low}, {median}, {high} euro cents")]
pub struct UnorderedEuroRangeError {
    /// The low end that was rejected, in euro cents.
    pub low: u32,
    /// The median that was rejected, in euro cents.
    pub median: u32,
    /// The high end that was rejected, in euro cents.
    pub high: u32,
}

impl EuroRange {
    /// Checks the order of the ends and wraps them.
    ///
    /// # Errors
    ///
    /// Returns an error when `low` is more than `median` or `median` is more than `high`.
    pub fn new(
        low: EuroCents,
        median: EuroCents,
        high: EuroCents,
    ) -> Result<Self, UnorderedEuroRangeError> {
        if low > median || median > high {
            return Err(UnorderedEuroRangeError {
                low: low.0,
                median: median.0,
                high: high.0,
            });
        }
        Ok(Self { low, median, high })
    }

    /// Returns the low end of the estimate.
    pub fn low(self) -> EuroCents {
        self.low
    }

    /// Returns the median of the estimate.
    pub fn median(self) -> EuroCents {
        self.median
    }

    /// Returns the high end of the estimate.
    pub fn high(self) -> EuroCents {
        self.high
    }
}

/// Where a link leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum BookingLinkKind {
    /// The shop of the operator, where the traveller starts a search.
    Search,
    /// The page that sells the product.
    Product,
}

/// A link to the place where a ticket is bought.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct BookingLink {
    /// Text of the link, such as "Open the Deutsche Bahn shop".
    pub label: SmolStr,
    /// Address of the page.
    pub url: SmolStr,
    /// What the link leads to.
    pub kind: BookingLinkKind,
}

/// What a ticket is.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum TicketProduct {
    /// A ticket for one train or a run of trains on one day.
    SingleTicket,
    /// An Interrail pass.
    InterrailPass {
        /// The `id` of the pass in the price list.
        pass_id: SmolStr,
        /// The first day of validity.
        first_valid_date: NaiveDate,
        /// The days the pass is used on. For a continuous pass, every day of validity.
        travel_days: Vec<NaiveDate>,
    },
    /// A seat reservation for a train that a pass covers.
    Reservation,
    /// Berth or sleeper accommodation on a night train.
    NightAccommodation,
}

/// One ticket to buy.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct TicketItem {
    /// Identifies the ticket within its strategy, and is what a ride's cover refers to.
    pub id: u16,
    /// What the ticket is.
    pub product: TicketProduct,
    /// Short name, such as "Interrail Global Pass, 7 days in 1 month, 2nd class, adult".
    pub title: SmolStr,
    /// What the ticket covers, in words.
    pub detail: SmolStr,
    /// Indices into `TicketPlan::rides` of the rides the ticket covers.
    pub ride_indices: Vec<u16>,
    /// What the ticket costs.
    pub price: Price,
    /// Where to buy it.
    pub booking_link: Option<BookingLink>,
    /// What to ask a booking site for the price of this ticket, if a site can tell it.
    pub live_quote_request: Option<LiveQuoteRequest>,
}

/// A request for the live price of a ticket on the booking site that sells it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct LiveQuoteRequest {
    /// The booking link of the ticket, which names the site and the search.
    pub booking_link: SmolStr,
    /// Departure of the first train the ticket covers, which picks the trip from the answer.
    pub first_departure_time: UnixTimeMinutes,
    /// Number of trains the ticket covers.
    pub ride_count: u8,
    /// The trains the ticket covers as category and number, such as `ICE 848`, in order. A site
    /// that lists connections matches the trip by them, because it can differ from the timetable
    /// on a city train at the start or the end.
    #[serde(default)]
    pub train_labels: Vec<SmolStr>,
    /// Class the price is asked for.
    pub travel_class: TravelClass,
    /// The search to run on the booking site, for a site whose booking link does not name it.
    #[serde(default)]
    pub site_search: Option<SiteSearch>,
    /// The run of trains the ticket covers, so the price model can learn from the live price.
    /// `None` for a ticket that is not for one adult in second class, whose price the model
    /// does not predict.
    #[serde(default)]
    pub run: Option<RunDescription>,
    /// Ask the booking site again, whatever it answered to an earlier lookup of this ticket.
    #[serde(default)]
    pub refresh: bool,
    /// Set when the request is for the accommodation of a night train and not for a ticket.
    #[serde(default)]
    pub accommodation: Option<AccommodationRequest>,
}

/// A request for the live price of the accommodation of a night train.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct AccommodationRequest {
    /// The train as the shop names it, such as `NJ 466`.
    pub train: SmolStr,
    /// Whether the traveller has a pass, who pays the accommodation with its reservation. Without
    /// one, the ticket holds a seat and the accommodation is the extra over it.
    pub with_pass: bool,
    /// The estimated price of a seat on the train. A train that sells no seat sells its beds at the
    /// price of the whole ticket, so the lookup subtracts this from a bed price to get the extra
    /// over the seat that the ticket item already holds.
    #[serde(default)]
    pub seat_estimate: Option<EuroCents>,
}

/// The run of trains that one ticket covers, as the price model reads it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct RunDescription {
    /// Name of the station where the run starts.
    pub from_station: SmolStr,
    /// Name of the station where the run ends.
    pub to_station: SmolStr,
    /// Departure of the first train on the local clock of its station.
    pub local_departure: NaiveDateTime,
    /// The trains of the run in order.
    pub rides: Vec<RideDescription>,
}

/// One train of a run, as the price model reads it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct RideDescription {
    /// The operator as the app names it, such as `db-fernverkehr`. Empty when not known.
    pub operator: SmolStr,
    /// Category of the train, such as "ICE".
    pub category: SmolStr,
    /// Country of the boarding station as a two-letter code, if known.
    pub from_country: Option<SmolStr>,
    /// Country of the alighting station, if known.
    pub to_country: Option<SmolStr>,
    /// Length of the ride along its line, in metres.
    pub distance_metres: u32,
    /// Whether the train has couchettes or sleepers.
    pub offers_berths: bool,
    /// The train as category and number, such as `ICE 848`. Empty in prices saved before the app
    /// kept it.
    #[serde(default)]
    pub train: SmolStr,
}

/// A live price of a ticket, kept so the price model can learn from it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct PriceObservation {
    /// The run the ticket covers.
    pub run: RunDescription,
    /// The lowest price for one adult in second class.
    pub price: EuroCents,
    /// The price in the currency of the booking site, when that is not the euro.
    pub original: Option<ForeignAmount>,
    /// The day the price was looked up.
    pub looked_up_on: NaiveDate,
    /// Name of the source, such as "DB booking site".
    pub source: SmolStr,
}

/// A search on a booking site whose booking link opens its start page.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct SiteSearch {
    /// Code of the first station on the site.
    pub origin_code: SmolStr,
    /// Code of the last station on the site.
    pub destination_code: SmolStr,
    /// Departure of the first train on the local clock of its station, as `YYYY-MM-DDTHH:MM`.
    pub local_departure: SmolStr,
}

/// The answer of a booking site about a ticket.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum LiveQuote {
    /// The site sells the ticket at this price.
    Priced(Price),
    /// The site shows the trip and sells no ticket for it, such as a train whose sale has not
    /// opened or that is sold out.
    Unavailable,
}

/// What a strategy is.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum StrategyKind {
    /// Single tickets for every ride.
    SingleTickets,
    /// An Interrail pass for the days where it pays, with single tickets for the other rides.
    Pass,
}

/// A way to cover every ride of the trip.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct TicketStrategy {
    /// What the strategy is.
    pub kind: StrategyKind,
    /// Short name for the comparison.
    pub title: SmolStr,
    /// The tickets to buy.
    pub items: Vec<TicketItem>,
    /// What the strategy costs in all.
    pub total: PriceTotal,
    /// For each ride of `TicketPlan::rides`, the ids of the tickets that cover it.
    pub covers: Vec<Vec<u16>>,
    /// Things the traveller should know, such as rides that no ticket was priced for.
    pub notes: Vec<SmolStr>,
}

/// A train of the trip as the tickets screen shows it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct TicketRide {
    /// The place the leg of the ride arrives at.
    pub leg_arrival_place_id: TripPlaceId,
    /// How the train is shown.
    pub train_label: TrainLabel,
    /// Name of the station where the traveller boards.
    pub from_name: SmolStr,
    /// Name of the station where the traveller leaves the train.
    pub to_name: SmolStr,
    /// Departure.
    pub departure_time: UnixTimeMinutes,
    /// Arrival.
    pub arrival_time: UnixTimeMinutes,
    /// IANA time zone of the boarding station.
    pub time_zone: SmolStr,
    /// Whether the train has berths.
    pub offers_berths: bool,
    /// Pages that show the layout of the train, the best first.
    #[serde(default)]
    pub layout_links: Vec<LayoutLink>,
}

/// The tickets a trip needs for one traveller.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct TicketPlan {
    /// The trains of the trip in travel order.
    pub rides: Vec<TicketRide>,
    /// The ways to cover them, the cheapest at the middle of the ranges first.
    pub strategies: Vec<TicketStrategy>,
    /// Name of the traveller the plan is for, or `None` when the plan assumes an adult.
    pub traveller_name: Option<SmolStr>,
    /// Remarks about the plan as a whole, such as an assumed traveller.
    pub notes: Vec<SmolStr>,
}

/// The chosen journey of one leg, as the frontend hands it over to price the trip.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct TicketLeg {
    /// The place the leg arrives at.
    pub arrival_place_id: TripPlaceId,
    /// The journey the traveller rides.
    pub journey: crate::plan::Journey,
    /// The stations the journey's rides refer to.
    pub stations: Vec<crate::plan::LegPlanStation>,
}

/// What a pass option of a leg costs, as the choice of a pass sees it. It mirrors
/// `PassJourneyOption` without the journey.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct PassOptionSummary {
    /// The journey's cost in minutes of train travel, without the price.
    pub cost_minutes: u32,
    /// What the pass does not cover, in euro cents.
    pub price_cents: EuroCents,
    /// How many travel days the journey uses.
    pub days: PassDays,
    /// The first travel day it uses.
    pub first_date: NaiveDate,
}

/// One leg of a trip as the choice of a pass sees it: the journey it takes on single tickets
/// and its best journeys on the travel days of a pass.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct PassScenarioLeg {
    /// The leg's departure date.
    pub date: NaiveDate,
    /// The cost of the journey the leg takes on single tickets, in minutes of train travel,
    /// without the price.
    pub single_cost_minutes: u32,
    /// The median price of that journey's tickets. `None` when no price is known, which the
    /// choice counts as nothing.
    pub single_price: Option<EuroCents>,
    /// The leg's pass options, in the order of `LegPlan::pass_options`.
    pub pass_options: Vec<PassOptionSummary>,
}

/// The pass of a scenario and its window of validity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct ScenarioPass {
    /// The pass's name, such as "Interrail Global Pass, 4 days in 1 month".
    pub name: SmolStr,
    /// The first day of validity.
    pub first_valid_date: NaiveDate,
    /// The last day of validity.
    pub last_valid_date: NaiveDate,
    /// The price of the pass for the traveller.
    pub price: EuroCents,
}

/// One way of covering the whole trip: single tickets everywhere, or a pass with the legs that
/// ride on its travel days.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct PassScenario {
    /// The pass, or `None` when every leg takes single tickets.
    pub pass: Option<ScenarioPass>,
    /// For each leg in the order the request gave them, the pass option it takes as an index
    /// into its pass options, or `None` when it takes single tickets.
    pub pass_option_of_leg: Vec<Option<u8>>,
    /// The total cost of the journeys in minutes of train travel.
    pub minutes: u32,
    /// The total price of the tickets, the pass included. The median of the estimates.
    pub price: EuroCents,
}
