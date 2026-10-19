//! A trip with its planned journeys, as the traveller shares it with the web viewer.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use smol_str::SmolStr;
use specta::Type;
use uuid::Uuid;

use crate::coordinates::{GeoCoordinates, Latitude, Longitude};
use crate::itinerary::{Itinerary, ItineraryRoute, TripPlace, TripPlaceId, ViaPlace};
use crate::plan::{
    Journey, JourneyCostParts, LegPlan, LegPlanStation, LegPlanStationIndex, LegPlanningResult,
    TrainLabel, TrainRide, TransferAndRide,
};
use crate::trip::{Trip, TripId, TripPreferences};

/// Size of the steps that a snapshot places its places in, in microdegrees, which is about 11
/// meters. A place is a point that the traveller picked on a map or found by search, and a
/// difference of a few meters changes neither the map nor the stations around it, but it saves
/// a byte in every link.
pub(crate) const PLACE_STEP_MICRODEGREES: i64 = 100;

/// Schema version that `StoredSnapshot` documents written by this build carry.
///
/// A change that old viewers cannot read bumps it. The viewer reads every older version, so a
/// link that was sent once keeps opening.
pub const SNAPSHOT_SCHEMA_VERSION: u16 = 1;

/// A snapshot as it is exported, with the schema version of its document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct StoredSnapshot {
    /// Version of the document layout, which the reader checks before it reads the rest.
    pub schema_version: u16,
    /// The snapshot itself.
    pub snapshot: TripSnapshot,
}

/// The result for one leg of a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SnapshotLeg {
    /// Place at which the leg arrives, which identifies the leg in the itinerary.
    pub arrival_place_id: TripPlaceId,
    /// The journeys of the leg, or why there are none.
    pub result: LegPlanningResult,
}

/// Everything the web viewer shows of a trip, already computed.
///
/// The viewer needs no timetable, no prices and no planner, so the snapshot holds the chosen and
/// alternative journeys of every leg with their prices. It leaves out what only the desktop app
/// uses: traveller profiles, the cost parts of a journey, the geometry of the trains and the
/// options for pass days. `TripSnapshot::new` removes those, so a snapshot decoded from a link
/// equals the snapshot that was encoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TripSnapshot {
    /// The trip, with the places, the stays and the time windows.
    pub trip: Trip,
    /// The planned legs in itinerary order. A leg that was not planned when the traveller
    /// shared the trip has no entry.
    pub legs: Vec<SnapshotLeg>,
    /// The day the legs were planned and priced, which the viewer shows at the top.
    pub planned_at: NaiveDate,
    /// Version of the timetable bundle the plans come from. Zero for plans from the stub
    /// backend.
    pub bundle_version: u32,
}

impl TripSnapshot {
    /// Builds a snapshot of a trip and the results the traveller sees for its legs.
    ///
    /// Drops what the viewer does not show, and legs that the itinerary does not have. The
    /// legs end up in itinerary order.
    pub fn new(
        trip: &Trip,
        legs: &[SnapshotLeg],
        planned_at: NaiveDate,
        bundle_version: u32,
    ) -> Self {
        let mut ordered_legs: Vec<SnapshotLeg> = Vec::new();
        for itinerary_leg in trip.itinerary.legs() {
            let arrival_place_id = itinerary_leg.arrival_place.id;
            if let Some(leg) = legs
                .iter()
                .find(|leg| leg.arrival_place_id == arrival_place_id)
            {
                ordered_legs.push(SnapshotLeg {
                    arrival_place_id,
                    result: shared_result(&leg.result),
                });
            }
        }
        Self {
            trip: shared_trip(trip),
            legs: ordered_legs,
            planned_at,
            bundle_version,
        }
    }

    /// Returns the snapshot with only the chosen journey of every leg. The link is a lot
    /// smaller, which a QR code needs.
    #[must_use]
    pub fn chosen_journeys_only(mut self) -> Self {
        for leg in &mut self.legs {
            if let LegPlanningResult::Found(plan) = &mut leg.result {
                plan.alternative_journeys.clear();
                drop_unused_stations(plan);
            }
        }
        self
    }
}

/// Returns the trip as the snapshot keeps it: without an ID, without traveller profiles, without
/// the journeys the traveller picked and with the default preferences except for the time
/// windows and the travel class.
fn shared_trip(trip: &Trip) -> Trip {
    Trip {
        id: TripId(Uuid::nil()),
        name: trip.name.clone(),
        start_date: trip.start_date,
        itinerary: snapped_itinerary(&trip.itinerary),
        preferences: TripPreferences {
            default_windows: trip.preferences.default_windows,
            travel_class: trip.preferences.travel_class,
            ..TripPreferences::default()
        },
        next_place_id: trip.next_place_id,
        chosen_journeys: Vec::new(),
    }
}

/// Returns the itinerary with every place at the nearest multiple of `PLACE_STEP_MICRODEGREES`.
fn snapped_itinerary(itinerary: &Itinerary) -> Itinerary {
    let snapped = |place: &TripPlace| {
        let snap = |microdegrees: i32| {
            let steps = (i64::from(microdegrees) + PLACE_STEP_MICRODEGREES / 2)
                .div_euclid(PLACE_STEP_MICRODEGREES);
            // The limits of both axes are multiples of the step, so the result stays in range.
            i32::try_from(steps * PLACE_STEP_MICRODEGREES).unwrap_or(microdegrees)
        };
        let coordinates = GeoCoordinates {
            latitude: Latitude::from_microdegrees(snap(place.coordinates.latitude.microdegrees()))
                .unwrap_or(place.coordinates.latitude),
            longitude: Longitude::from_microdegrees(snap(
                place.coordinates.longitude.microdegrees(),
            ))
            .unwrap_or(place.coordinates.longitude),
        };
        TripPlace {
            coordinates,
            ..place.clone()
        }
    };
    match itinerary {
        Itinerary::Empty => Itinerary::Empty,
        Itinerary::OriginOnly(origin) => Itinerary::OriginOnly(snapped(origin)),
        Itinerary::Route(route) => Itinerary::Route(ItineraryRoute {
            origin: snapped(&route.origin),
            via_places: route
                .via_places
                .iter()
                .map(|via_place| ViaPlace {
                    place: snapped(&via_place.place),
                    ..via_place.clone()
                })
                .collect(),
            final_leg_travel_mode: route.final_leg_travel_mode,
            destination: snapped(&route.destination),
        }),
    }
}

/// Returns a leg result as the snapshot keeps it.
fn shared_result(result: &LegPlanningResult) -> LegPlanningResult {
    match result {
        LegPlanningResult::Found(plan) => {
            let mut shared = LegPlan {
                chosen_journey: shared_journey(&plan.chosen_journey),
                alternative_journeys: plan
                    .alternative_journeys
                    .iter()
                    .map(shared_journey)
                    .collect(),
                station_table: plan.station_table.clone(),
                follows_traveller_choice: plan.follows_traveller_choice,
                best_journey_cost: 0,
                pass_options: SmallVec::new(),
                departure_window: None,
            };
            drop_unused_stations(&mut shared);
            LegPlanningResult::Found(Box::new(shared))
        }
        LegPlanningResult::NoJourneyFound(reason) => LegPlanningResult::NoJourneyFound(*reason),
    }
}

/// Removes the stations that no journey of the plan uses, and the repeats of a station, which
/// the planner's table holds for every station it considered near the places. The journeys
/// point at the new table.
fn drop_unused_stations(plan: &mut LegPlan) {
    let mut used = vec![false; plan.station_table.len()];
    let journeys = std::iter::once(&plan.chosen_journey).chain(&plan.alternative_journeys);
    for journey in journeys {
        let footpath_station = journey.arrival_footpath.map(|footpath| footpath.to_station);
        let ride_stations = journey
            .rides()
            .flat_map(|ride| [ride.boarding_station, ride.alighting_station]);
        for station in ride_stations.chain(footpath_station) {
            if let Some(flag) = used.get_mut(usize::from(station.0)) {
                *flag = true;
            }
        }
    }
    let mut kept: SmallVec<[LegPlanStation; 8]> = SmallVec::new();
    let mut mapping = Vec::with_capacity(plan.station_table.len());
    for (index, station) in plan.station_table.iter().enumerate() {
        let position = match kept.iter().position(|candidate| candidate == station) {
            Some(position) => position,
            None if used[index] => {
                kept.push(station.clone());
                kept.len() - 1
            }
            None => 0,
        };
        // The table holds at most 256 stations, which the index of a station can name.
        mapping.push(LegPlanStationIndex(u8::try_from(position).unwrap_or(0)));
    }
    for journey in std::iter::once(&mut plan.chosen_journey).chain(&mut plan.alternative_journeys) {
        journey.remap_stations(&mapping);
    }
    plan.station_table = kept;
}

/// Returns a journey as the snapshot keeps it: without the geometry of its trains, without their
/// layout links, without the agency of its trains and with only the door-to-door time and the price of its cost parts.
fn shared_journey(journey: &Journey) -> Journey {
    let shared_ride = |ride: &TrainRide| TrainRide {
        intermediate_station_coordinates: SmallVec::new(),
        path_coordinates: Vec::new(),
        layout_links: Vec::new(),
        train_label: TrainLabel {
            agency: SmolStr::default(),
            ..ride.train_label.clone()
        },
        ..ride.clone()
    };
    Journey {
        access: journey.access,
        first_ride: shared_ride(&journey.first_ride),
        later_rides: journey
            .later_rides
            .iter()
            .map(|later| TransferAndRide {
                transfer_duration: later.transfer_duration,
                ride: shared_ride(&later.ride),
            })
            .collect(),
        arrival_footpath: journey.arrival_footpath,
        egress: journey.egress,
        sleeping_ride: journey.sleeping_ride,
        cost_parts: journey.cost_parts.map(|parts| JourneyCostParts {
            door_to_door_minutes: parts.door_to_door_minutes,
            way_to_station_minutes: 0,
            way_to_station_extra_cost: 0,
            transfer_count: 0,
            transfer_cost_each: 0,
            minutes_outside_arrival_window: 0,
            arrival_window_cost: 0,
            sleeping_minutes: 0,
            night_minutes_awake: 0,
            night_awake_cost: 0,
            price: parts.price,
            price_cost: 0,
            rest_of_chain_cost: 0,
            total_cost: 0,
        }),
        alternative_kind: journey.alternative_kind,
    }
}
