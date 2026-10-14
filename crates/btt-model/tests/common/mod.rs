//! Snapshots that the link tests and the viewer fixtures share.

// The fixtures list every field of the journeys they build, which takes lines.
#![allow(clippy::too_many_lines)]

use std::num::{NonZeroU8, NonZeroU32};

use btt_model::{
    AcceptedBerths, AlternativeKind, ArrivalFootpath, BerthType, DailyTimeWindow, DurationMinutes,
    EuroCents, EuroRange, GeoCoordinates, Itinerary, ItineraryRoute, Journey, JourneyCostParts,
    LegPlan, LegPlanStation, LegPlanStationIndex, LegPlanningResult, LegTravelMode, LocalTimeOfDay,
    NoJourneyReason, OvernightLegOptions, PlaceStay, PlaceWindowOverride, SameDayStayRange,
    SnapshotLeg, StationSearchRadius, StationWay, TrainLabel, TrainRide, TransferAndRide, Trip,
    TripPlace, TripPlaceId, TripSnapshot, UnixTimeMinutes, ViaPlace, WayToStationMode,
};
use chrono::NaiveDate;
use smallvec::{SmallVec, smallvec};

/// Returns a date.
pub fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// Returns the minutes since the Unix epoch of a time in UTC.
pub fn utc(date: NaiveDate, hour: u32, minute: u32) -> UnixTimeMinutes {
    let seconds = date
        .and_hms_opt(hour, minute, 0)
        .unwrap()
        .and_utc()
        .timestamp();
    UnixTimeMinutes(u32::try_from(seconds / 60).unwrap())
}

fn coordinates(latitude: f64, longitude: f64) -> GeoCoordinates {
    GeoCoordinates::from_degrees(latitude, longitude).unwrap()
}

pub fn place(id: u32, name: &str, latitude: f64, longitude: f64) -> TripPlace {
    TripPlace {
        id: TripPlaceId(NonZeroU32::new(id).unwrap()),
        name: name.into(),
        coordinates: coordinates(latitude, longitude),
        station_search_radius: StationSearchRadius::DEFAULT,
        window_override: None,
    }
}

fn station(name: &str, latitude: f64, longitude: f64, time_zone: &str) -> LegPlanStation {
    LegPlanStation {
        name: name.into(),
        coordinates: coordinates(latitude, longitude),
        time_zone: time_zone.into(),
    }
}

pub fn way(minutes: u16, meters: u32) -> StationWay {
    StationWay {
        duration: DurationMinutes(minutes),
        distance_meters: meters,
        mode: if meters > 1000 {
            WayToStationMode::LocalTransport
        } else {
            WayToStationMode::Walk
        },
    }
}

fn label(category: &str, number: &str, operator: &str, agency: &str) -> TrainLabel {
    TrainLabel {
        category: category.into(),
        number: number.into(),
        operator: operator.into(),
        agency: agency.into(),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn ride(
    boarding: u8,
    departure: UnixTimeMinutes,
    alighting: u8,
    arrival: UnixTimeMinutes,
    train_label: TrainLabel,
    offers_berths: bool,
) -> TrainRide {
    TrainRide {
        boarding_station: LegPlanStationIndex(boarding),
        departure_time: departure,
        alighting_station: LegPlanStationIndex(alighting),
        arrival_time: arrival,
        train_label,
        offers_berths,
        intermediate_station_coordinates: SmallVec::new(),
        path_coordinates: Vec::new(),
    }
}

/// Returns cost parts that hold only what a snapshot keeps.
pub fn cost_parts(door_to_door_minutes: u32, price: Option<(u32, u32, u32)>) -> JourneyCostParts {
    JourneyCostParts {
        door_to_door_minutes,
        way_to_station_minutes: 0,
        way_to_station_extra_cost: 0,
        transfer_count: 0,
        transfer_cost_each: 0,
        minutes_outside_arrival_window: 0,
        arrival_window_cost: 0,
        sleeping_minutes: 0,
        night_minutes_awake: 0,
        night_awake_cost: 0,
        price: price.map(|(low, median, high)| {
            EuroRange::new(EuroCents(low), EuroCents(median), EuroCents(high)).unwrap()
        }),
        price_cost: 0,
        rest_of_chain_cost: 0,
        total_cost: 0,
    }
}

pub fn trip(itinerary: Itinerary) -> Trip {
    let mut trip = Trip::new("Vienna and Rome".into(), date(2026, 10, 14));
    trip.itinerary = itinerary;
    trip.next_place_id = NonZeroU32::new(10).unwrap();
    trip
}

fn found(plan: LegPlan) -> LegPlanningResult {
    LegPlanningResult::Found(Box::new(plan))
}

/// Amsterdam, two nights in Vienna, a night train to Rome, then a leg without a journey. It
/// uses a transfer, a footpath, a night train, an alternative of each kind and every variant of
/// the itinerary the link writes.
pub fn demo_snapshot() -> TripSnapshot {
    let day_one = date(2026, 10, 14);
    let day_three = date(2026, 10, 16);
    let mut amsterdam = place(1, "Amsterdam", 52.3789, 4.9003);
    amsterdam.window_override = Some(PlaceWindowOverride::Departure(
        DailyTimeWindow::from_start_and_end(
            LocalTimeOfDay::from_hour_and_minute(6, 30).unwrap(),
            LocalTimeOfDay::from_hour_and_minute(9, 0).unwrap(),
        ),
    ));
    let itinerary = Itinerary::Route(ItineraryRoute {
        origin: amsterdam,
        via_places: vec![
            ViaPlace {
                arriving_leg_travel_mode: LegTravelMode::Daytime,
                place: place(2, "Wien", 48.1852, 16.3761),
                stay: PlaceStay::Nights(NonZeroU8::new(2).unwrap()),
            },
            ViaPlace {
                arriving_leg_travel_mode: LegTravelMode::Overnight(OvernightLegOptions {
                    bedtime: LocalTimeOfDay::from_hour_and_minute(22, 30).unwrap(),
                    wake_up_time: LocalTimeOfDay::from_hour_and_minute(7, 0).unwrap(),
                    accepted_berths: AcceptedBerths::Either {
                        preferred: Some(BerthType::Sleeper),
                    },
                }),
                place: place(3, "Roma", 41.901, 12.5007),
                stay: PlaceStay::SameDay {
                    stay_range_override: Some(SameDayStayRange {
                        minimum: DurationMinutes(120),
                        maximum_beyond_minimum: Some(DurationMinutes(180)),
                    }),
                    prefers_longer_stay: true,
                },
            },
        ],
        final_leg_travel_mode: LegTravelMode::Daytime,
        destination: place(4, "Napoli", 40.8518, 14.2681),
    });
    let trip = trip(itinerary);

    // Leg one: with a transfer in Frankfurt and a walk at the end.
    let stations_one: SmallVec<[LegPlanStation; 8]> = smallvec![
        station("Amsterdam Centraal", 52.3789, 4.9003, "Europe/Amsterdam"),
        station("Frankfurt (Main) Hbf", 50.1071, 8.6638, "Europe/Berlin"),
        station("Wien Hbf", 48.1852, 16.3761, "Europe/Vienna"),
        station("Wien Meidling", 48.1742, 16.3319, "Europe/Vienna"),
    ];
    let ice = label("ICE", "125", "db-fernverkehr", "DB Fernverkehr AG");
    let railjet = label("RJX", "63", "oebb", "ÖBB-Personenverkehr AG");
    let chosen_one = Journey {
        access: way(4, 300),
        first_ride: ride(
            0,
            utc(day_one, 6, 2),
            1,
            utc(day_one, 9, 41),
            ice.clone(),
            false,
        ),
        later_rides: smallvec![TransferAndRide {
            transfer_duration: DurationMinutes(19),
            ride: ride(
                1,
                utc(day_one, 10, 0),
                2,
                utc(day_one, 15, 36),
                railjet.clone(),
                false
            ),
        }],
        arrival_footpath: None,
        egress: way(9, 2100),
        sleeping_ride: None,
        cost_parts: Some(cost_parts(578, Some((9900, 12900, 15900)))),
        alternative_kind: None,
    };
    let cheaper_one = Journey {
        access: way(4, 300),
        first_ride: ride(
            0,
            utc(day_one, 4, 40),
            1,
            utc(day_one, 8, 55),
            label("IC", "2013", "db-fernverkehr", "DB Fernverkehr AG"),
            false,
        ),
        later_rides: smallvec![TransferAndRide {
            // A transfer that is not the gap between the two rides.
            transfer_duration: DurationMinutes(31),
            ride: ride(
                1,
                utc(day_one, 9, 40),
                3,
                utc(day_one, 16, 5),
                railjet,
                false
            ),
        }],
        arrival_footpath: Some(ArrivalFootpath {
            to_station: LegPlanStationIndex(2),
            way: way(7, 1600),
        }),
        egress: way(9, 2100),
        sleeping_ride: None,
        cost_parts: Some(cost_parts(675, Some((4900, 6900, 9900)))),
        alternative_kind: Some(AlternativeKind::Cheaper),
    };
    let faster_one = Journey {
        access: way(4, 300),
        first_ride: ride(0, utc(day_one, 7, 2), 2, utc(day_one, 15, 10), ice, false),
        later_rides: SmallVec::new(),
        arrival_footpath: None,
        egress: way(9, 2100),
        sleeping_ride: None,
        cost_parts: Some(cost_parts(500, None)),
        alternative_kind: Some(AlternativeKind::Faster),
    };
    let leg_one = LegPlan {
        chosen_journey: chosen_one,
        alternative_journeys: smallvec![cheaper_one, faster_one],
        station_table: stations_one,
        follows_traveller_choice: true,
        best_journey_cost: 0,
        pass_options: SmallVec::new(),
        departure_window: None,
    };

    // Leg two: a night train from Vienna to Rome, ridden on the third day.
    let stations_two: SmallVec<[LegPlanStation; 8]> = smallvec![
        station("Wien Hbf", 48.1852, 16.3761, "Europe/Vienna"),
        station("Roma Termini", 41.9009, 12.5017, "Europe/Rome"),
    ];
    let night = Journey {
        access: way(9, 2100),
        first_ride: ride(
            0,
            utc(day_three, 19, 40),
            1,
            utc(date(2026, 10, 17), 7, 55),
            label("NJ", "295", "oebb", "ÖBB-Personenverkehr AG"),
            true,
        ),
        later_rides: SmallVec::new(),
        arrival_footpath: None,
        egress: way(3, 200),
        sleeping_ride: Some(0),
        cost_parts: Some(cost_parts(740, Some((8900, 11900, 21900)))),
        alternative_kind: None,
    };
    let leg_two = LegPlan {
        chosen_journey: night,
        alternative_journeys: SmallVec::new(),
        station_table: stations_two,
        follows_traveller_choice: false,
        best_journey_cost: 0,
        pass_options: SmallVec::new(),
        departure_window: None,
    };

    let arrival_ids = [
        TripPlaceId(NonZeroU32::new(2).unwrap()),
        TripPlaceId(NonZeroU32::new(3).unwrap()),
        TripPlaceId(NonZeroU32::new(4).unwrap()),
    ];
    TripSnapshot::new(
        &trip,
        &[
            SnapshotLeg {
                arrival_place_id: arrival_ids[0],
                result: found(leg_one),
            },
            SnapshotLeg {
                arrival_place_id: arrival_ids[1],
                result: found(leg_two),
            },
            SnapshotLeg {
                arrival_place_id: arrival_ids[2],
                result: LegPlanningResult::NoJourneyFound(NoJourneyReason::NoConnectionInWindow),
            },
        ],
        date(2026, 10, 11),
        2_026_101_101,
    )
}

/// A trip of ten legs with five journeys each, which has more of everything than a real trip.
pub fn stress_snapshot() -> TripSnapshot {
    let start = date(2026, 10, 14);
    let names = [
        ("Amsterdam", 52.3789, 4.9003),
        ("Köln", 50.9430, 6.9589),
        ("Frankfurt", 50.1071, 8.6638),
        ("Nürnberg", 49.4456, 11.0827),
        ("München", 48.1402, 11.5586),
        ("Innsbruck", 47.2632, 11.4009),
        ("Verona", 45.4286, 10.9822),
        ("Bologna", 44.5058, 11.3430),
        ("Firenze", 43.7764, 11.2478),
        ("Roma", 41.9009, 12.5017),
        ("Napoli", 40.8518, 14.2681),
    ];
    let time_zones = [
        "Europe/Amsterdam",
        "Europe/Berlin",
        "Europe/Berlin",
        "Europe/Berlin",
        "Europe/Berlin",
        "Europe/Vienna",
        "Europe/Rome",
        "Europe/Rome",
        "Europe/Rome",
        "Europe/Rome",
        "Europe/Rome",
    ];
    let mut places = names
        .iter()
        .enumerate()
        .map(|(index, (name, latitude, longitude))| {
            place(
                u32::try_from(index).unwrap() + 1,
                name,
                *latitude,
                *longitude,
            )
        })
        .collect::<Vec<_>>();
    let destination = places.pop().unwrap();
    let origin = places.remove(0);
    let via_places = places
        .into_iter()
        .map(|place| ViaPlace {
            arriving_leg_travel_mode: LegTravelMode::Daytime,
            place,
            stay: PlaceStay::Nights(NonZeroU8::new(1).unwrap()),
        })
        .collect();
    let trip = trip(Itinerary::Route(ItineraryRoute {
        origin,
        via_places,
        final_leg_travel_mode: LegTravelMode::Daytime,
        destination,
    }));
    let mut legs = Vec::new();
    for leg_index in 0..10_usize {
        let day = start + chrono::Days::new(leg_index as u64);
        let from = names[leg_index];
        let to = names[leg_index + 1];
        let stations: SmallVec<[LegPlanStation; 8]> = smallvec![
            station(
                &format!("{} Hbf", from.0),
                from.1,
                from.2,
                time_zones[leg_index]
            ),
            station(
                &format!("{} Hbf", to.0),
                to.1,
                to.2,
                time_zones[leg_index + 1]
            ),
            station(
                &format!("{} Süd", from.0),
                from.1 - 0.01,
                from.2 + 0.02,
                time_zones[leg_index]
            ),
            station(
                &format!("{} Mitte", to.0),
                to.1 + 0.01,
                to.2 - 0.02,
                time_zones[leg_index + 1]
            ),
        ];
        let journey = |variant: u32| {
            let offset = variant * 37 + u32::try_from(leg_index).unwrap() * 11;
            Journey {
                access: way(u16::try_from(3 + variant).unwrap(), 400 + 300 * variant),
                first_ride: ride(
                    u8::try_from(variant % 2 * 2).unwrap(),
                    utc(day, 6 + variant, offset % 60),
                    1,
                    utc(day, 8 + variant, (offset + 25) % 60),
                    label(
                        "ICE",
                        &format!("{}", 500 + offset),
                        "db-fernverkehr",
                        "DB Fernverkehr AG",
                    ),
                    false,
                ),
                later_rides: smallvec![TransferAndRide {
                    transfer_duration: DurationMinutes(17),
                    ride: ride(
                        1,
                        utc(day, 8 + variant, (offset + 42) % 60),
                        3,
                        utc(day, 11 + variant, (offset + 5) % 60),
                        label(
                            "RJX",
                            &format!("{}", 60 + offset),
                            "oebb",
                            "ÖBB-Personenverkehr AG"
                        ),
                        false,
                    ),
                }],
                arrival_footpath: None,
                egress: way(8, 1500),
                sleeping_ride: None,
                cost_parts: Some(cost_parts(
                    300 + offset,
                    Some((
                        3000 + 100 * offset,
                        5000 + 150 * offset,
                        8000 + 200 * offset,
                    )),
                )),
                alternative_kind: (variant == 1).then_some(AlternativeKind::Cheaper),
            }
        };
        legs.push(SnapshotLeg {
            arrival_place_id: TripPlaceId(
                NonZeroU32::new(u32::try_from(leg_index).unwrap() + 2).unwrap(),
            ),
            result: found(LegPlan {
                chosen_journey: journey(0),
                alternative_journeys: (1..5).map(journey).collect(),
                station_table: stations,
                follows_traveller_choice: false,
                best_journey_cost: 0,
                pass_options: SmallVec::new(),
                departure_window: None,
            }),
        });
    }
    TripSnapshot::new(&trip, &legs, date(2026, 10, 11), 2_026_101_101)
}
