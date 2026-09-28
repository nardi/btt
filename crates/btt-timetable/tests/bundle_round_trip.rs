//! Builds small bundles, archives them and reads them back.

use btt_model::GeoCoordinates;
use btt_timetable::{
    BundleBuildError, BundleBuilder, BundleStationIndex, BundleTripIndex, FeedAttributionInput,
    LoadedTimetableBundle, RaptorRouteIndex, ServiceDayBitset, StationIndex, StationInput,
    TrainLabelInput, TripInput, TripStopInput,
};
use chrono::NaiveDate;

fn station(name: &str, latitude: f64, longitude: f64) -> StationInput {
    StationInput {
        name: name.to_owned(),
        coordinates: GeoCoordinates::from_degrees(latitude, longitude).unwrap(),
        time_zone: "Europe/Berlin".to_owned(),
        trainline_station_id: None,
    }
}

fn stop(station: BundleStationIndex, arrival: u16, departure: u16) -> TripStopInput {
    TripStopInput {
        station,
        arrival,
        departure,
        boarding_allowed: true,
        alighting_allowed: true,
    }
}

fn every_day() -> ServiceDayBitset {
    let mut days = ServiceDayBitset::default();
    for day in 0..30 {
        days.insert(day);
    }
    days
}

fn label(number: &str) -> TrainLabelInput {
    TrainLabelInput {
        category: "ICE".into(),
        number: number.into(),
        agency: "DB".into(),
        offers_berths: number == "3",
    }
}

fn builder() -> BundleBuilder {
    BundleBuilder::new(
        NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
        30,
        2_026_100_100,
    )
    .unwrap()
}

#[test]
fn bundle_survives_archiving() {
    let mut builder = builder();
    let cologne = builder
        .add_station(station("Köln Hbf", 50.9430, 6.9589))
        .unwrap();
    let frankfurt = builder
        .add_station(station("Frankfurt (Main) Hbf", 50.1071, 8.6636))
        .unwrap();
    let munich = builder
        .add_station(station("München Hbf", 48.1402, 11.5586))
        .unwrap();
    builder.add_footpath(frankfurt, cologne, 5);
    for (number, start) in [("1", 360), ("3", 420)] {
        builder.add_trip(TripInput {
            stops: vec![
                stop(cologne, start, start),
                stop(frankfurt, start + 64, start + 68),
                stop(munich, start + 260, start + 260),
            ],
            service_days: every_day(),
            train_label: label(number),
        });
    }
    builder.add_attribution(FeedAttributionInput {
        feed_name: "DELFI".into(),
        license: "CC-BY-4.0".into(),
        url: "https://www.opendata-oepnv.de".into(),
    });
    let bundle = LoadedTimetableBundle::from_bundle(&builder.build().unwrap()).unwrap();

    assert_eq!(bundle.station_count(), 3);
    assert_eq!(bundle.station_name(frankfurt), "Frankfurt (Main) Hbf");
    assert_eq!(bundle.station_time_zone(munich), "Europe/Berlin");
    assert_eq!(
        bundle.validity_start(),
        NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()
    );
    assert_eq!(
        bundle.route_count(),
        1,
        "both trips share one pattern and never overtake"
    );

    let route = bundle.route(RaptorRouteIndex(0));
    assert_eq!((route.stop_count, route.trip_count), (3, 2));
    assert_eq!(bundle.stop_event(&route, 1, 1).departure.0, 488);
    assert_eq!(bundle.pattern_stop(&route, 2).station(), munich);
    assert_eq!(
        bundle
            .route_stops_at(frankfurt)
            .map(|stop| stop.stop_position)
            .collect::<Vec<_>>(),
        [1]
    );
    assert_eq!(
        bundle.footpaths_from(frankfurt).next().unwrap().to_station,
        cologne
    );
    assert_eq!(
        bundle.trip_train_label(BundleTripIndex(1)),
        ("ICE", "3", "DB")
    );
    assert!(bundle.trip_offers_berths(BundleTripIndex(1)));
    assert!(!bundle.trip_offers_berths(BundleTripIndex(0)));
    assert!(bundle.trip_service_days(BundleTripIndex(0)).runs_on(29));
    assert!(bundle.station_service_days(cologne).runs_on(0));
    assert_eq!(bundle.feed_attributions().next().unwrap().0, "DELFI");
}

#[test]
fn overtaking_trips_get_separate_routes() {
    let mut builder = builder();
    let a = builder.add_station(station("A", 50.0, 8.0)).unwrap();
    let b = builder.add_station(station("B", 50.5, 8.5)).unwrap();
    // The slow train leaves first and arrives last, so the fast one overtakes it.
    builder.add_trip(TripInput {
        stops: vec![stop(a, 600, 600), stop(b, 720, 720)],
        service_days: every_day(),
        train_label: label("slow"),
    });
    builder.add_trip(TripInput {
        stops: vec![stop(a, 610, 610), stop(b, 660, 660)],
        service_days: every_day(),
        train_label: label("fast"),
    });
    let bundle = LoadedTimetableBundle::from_bundle(&builder.build().unwrap()).unwrap();
    assert_eq!(bundle.route_count(), 2);
}

#[test]
fn trips_past_midnight_mark_the_next_day_at_later_stations() {
    let mut builder = builder();
    let vienna = builder
        .add_station(station("Wien Hbf", 48.1852, 16.3761))
        .unwrap();
    let rome = builder
        .add_station(station("Roma Termini", 41.9010, 12.5007))
        .unwrap();
    let mut first_day_only = ServiceDayBitset::default();
    first_day_only.insert(0);
    builder.add_trip(TripInput {
        stops: vec![stop(vienna, 1170, 1170), stop(rome, 1950, 1950)],
        service_days: first_day_only,
        train_label: label("NJ 40233"),
    });
    let built = builder.build().unwrap();
    assert_eq!(built.raptor_routes[0].longest_trip_days, 2);
    let bundle = LoadedTimetableBundle::from_bundle(&built).unwrap();
    assert!(bundle.station_service_days(vienna).runs_on(0));
    assert!(!bundle.station_service_days(rome).runs_on(0));
    assert!(bundle.station_service_days(rome).runs_on(1));
}

#[test]
fn invalid_trips_are_rejected() {
    let mut builder = builder();
    let a = builder.add_station(station("A", 50.0, 8.0)).unwrap();
    let b = builder.add_station(station("B", 50.5, 8.5)).unwrap();
    builder.add_trip(TripInput {
        stops: vec![stop(a, 600, 600), stop(b, 590, 590)],
        service_days: every_day(),
        train_label: label("1"),
    });
    assert_eq!(builder.build(), Err(BundleBuildError::TimesGoBackwards(0)));
}

#[test]
fn bundle_files_load_through_a_memory_map() {
    let mut builder = builder();
    let a = builder.add_station(station("A", 50.0, 8.0)).unwrap();
    let b = builder.add_station(station("B", 50.5, 8.5)).unwrap();
    builder.add_trip(TripInput {
        stops: vec![stop(a, 600, 600), stop(b, 660, 660)],
        service_days: every_day(),
        train_label: label("1"),
    });
    let path = std::env::temp_dir().join(format!("btt-bundle-{}.bin", std::process::id()));
    LoadedTimetableBundle::write_bundle(&builder.build().unwrap(), &path).unwrap();
    let bundle = LoadedTimetableBundle::open(&path).unwrap();
    assert_eq!(bundle.station_name(b), "B");
    drop(bundle);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn corrupt_files_are_rejected() {
    let path = std::env::temp_dir().join(format!("btt-corrupt-{}.bin", std::process::id()));
    std::fs::write(&path, [0u8; 64]).unwrap();
    assert!(LoadedTimetableBundle::open(&path).is_err());
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn station_index_finds_stations_within_a_radius() {
    let mut builder = builder();
    let hauptbahnhof = builder
        .add_station(station("Frankfurt (Main) Hbf", 50.1071, 8.6636))
        .unwrap();
    let sud = builder
        .add_station(station("Frankfurt (Main) Süd", 50.0996, 8.6862))
        .unwrap();
    let mainz = builder
        .add_station(station("Mainz Hbf", 50.0012, 8.2587))
        .unwrap();
    builder.add_trip(TripInput {
        stops: vec![
            stop(hauptbahnhof, 600, 600),
            stop(sud, 610, 610),
            stop(mainz, 640, 640),
        ],
        service_days: every_day(),
        train_label: label("1"),
    });
    let bundle = LoadedTimetableBundle::from_bundle(&builder.build().unwrap()).unwrap();
    let index = StationIndex::new(&bundle);
    let roemer = GeoCoordinates::from_degrees(50.1106, 8.6821).unwrap();
    let nearby = index.stations_within(&bundle, roemer, 5_000.0);
    let stations: Vec<_> = nearby.iter().map(|nearby| nearby.station).collect();
    assert_eq!(stations, [sud, hauptbahnhof]);
}
