//! Runs the whole pipeline on two small fixture feeds in an in-memory database.
//!
//! Feed A uses DHID-style stop IDs without parent stations and runs the ICE 105 from Köln to
//! Frankfurt. Feed B uses parent stations and runs the same train from Köln through Frankfurt
//! to Wien, under the label RJ 105. The build must merge the two copies, drop feed A's bus
//! route, match all stations to the Trainline fixture and add a footpath between the two
//! Frankfurt stations.

use std::io::Write;
use std::path::{Path, PathBuf};

use btt_builder::config::BuilderConfig;
use btt_builder::database::WorkDatabase;
use btt_builder::load::ValidityWindow;
use btt_builder::{FeedInput, PipelineInputs, run_pipeline};
use btt_timetable::{BundleStationIndex, BundleTripIndex, LoadedTimetableBundle, RaptorRouteIndex};
use chrono::NaiveDate;

const CONFIG: &str = r#"
validity_window_days = 14

[[feed]]
mirror_name = "de_TestA"
station_key_pattern = '^(de:\d+:\d+)'
[feed.train_label]
field = "trip_short_name"
pattern = '^0*(?<number>\d+)$'
category_field = "route_short_name"

[[feed]]
mirror_name = "at_TestB"
[feed.train_label]
field = "trip_short_name"
pattern = '^(?<category>[A-Za-z]+)\s*(?<number>\d+)$'
category_field = "route_short_name"
"#;

const STOPWORDS: &str = r#"stopwords = ["hbf", "hauptbahnhof"]"#;

const FEED_A: &[(&str, &str)] = &[
    (
        "agency.txt",
        "agency_id,agency_name,agency_url,agency_timezone\n\
         db,DB Fernverkehr AG,https://www.bahn.de,Europe/Berlin\n",
    ),
    (
        "routes.txt",
        "route_id,agency_id,route_short_name,route_type\n\
         ice,db,ICE,101\n\
         regional,db,RB,106\n\
         bus,db,100,3\n",
    ),
    (
        "trips.txt",
        "route_id,service_id,trip_id,trip_short_name\n\
         ice,daily,ice-105,0105\n\
         regional,daily,rb-1,4711\n\
         bus,daily,bus-1,1\n",
    ),
    (
        "stops.txt",
        "stop_id,stop_name,stop_lat,stop_lon,parent_station\n\
         de:1:1,Köln Hbf,50.9430,6.9589,\n\
         de:1:1:7:1,Köln Hbf,50.9431,6.9580,\n\
         de:1:2:1:1,Frankfurt(Main)Hbf,50.1070,8.6630,\n\
         de:1:3:1:1,Frankfurt(Main)Süd,50.0996,8.6862,\n",
    ),
    (
        "stop_times.txt",
        "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
         ice-105,08:00:00,08:00:00,de:1:1:7:1,1\n\
         ice-105,09:04:30,09:08:00,de:1:2:1:1,2\n\
         rb-1,10:00:00,10:00:00,de:1:3:1:1,1\n\
         rb-1,10:10:00,10:10:00,de:1:2:1:1,2\n\
         bus-1,10:00:00,10:00:00,de:1:1:7:1,1\n\
         bus-1,10:30:00,10:30:00,de:1:2:1:1,2\n",
    ),
    (
        "calendar.txt",
        "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
         daily,1,1,1,1,1,1,1,20261001,20261022\n",
    ),
    (
        "calendar_dates.txt",
        "service_id,date,exception_type\ndaily,20261021,2\n",
    ),
];

const FEED_B: &[(&str, &str)] = &[
    (
        "agency.txt",
        "agency_id,agency_name,agency_url,agency_timezone\n\
         oebb,ÖBB,https://www.oebb.at,Europe/Vienna\n",
    ),
    (
        "routes.txt",
        "route_id,agency_id,route_short_name,route_type\nrj,oebb,RJ,2\n",
    ),
    (
        "trips.txt",
        "route_id,service_id,trip_id,trip_short_name\nrj,all,rj-105,RJ 105\n",
    ),
    (
        "stops.txt",
        "stop_id,stop_name,stop_lat,stop_lon,parent_station,location_type\n\
         P0,Köln Hbf,50.9429,6.9587,,1\n\
         P1,Frankfurt (Main) Hbf,50.1071,8.6636,,1\n\
         P2,Wien Hbf,48.1852,16.3761,,1\n\
         0,Köln Hbf Gleis 1,50.9429,6.9587,P0,\n\
         1,Frankfurt (Main) Hbf Gleis 9,50.1071,8.6636,P1,\n\
         2,Wien Hbf Bahnsteig 5,48.1852,16.3761,P2,\n",
    ),
    (
        "stop_times.txt",
        "trip_id,arrival_time,departure_time,stop_id,stop_sequence,pickup_type,drop_off_type\n\
         rj-105,08:00:00,08:00:00,0,1,0,1\n\
         rj-105,09:05:00,09:08:00,1,2,,\n\
         rj-105,15:00:00,15:00:00,2,3,1,0\n",
    ),
    (
        "calendar_dates.txt",
        ("service_id,date,exception_type\n\
         all,20261020,1\nall,20261021,1\nall,20261022,1\n"),
    ),
];

const TRAINLINE_STATIONS: &str = "\
id;name;uic;latitude;longitude;is_city;db_id;time_zone;same_as
1;Köln Hbf;8000207;50.943029;6.958729;f;8000207;Europe/Berlin;
2;Frankfurt (Main) Hbf;8000105;50.107145;8.663789;f;8000105;Europe/Berlin;
3;Wien Hbf;8103000;48.185184;16.376413;f;;Europe/Vienna;
4;Frankfurt (Main) Süd;8002041;50.099365;8.686457;f;8002041;Europe/Berlin;
5;Frankfurt;;50.110922;8.682127;t;;Europe/Berlin;
6;Köln Hauptbahnhof;;50.943;6.959;f;;Europe/Berlin;1
";

const TRANSITOUS_DE: &str = r#"{"sources": [
    {"name": "TestA", "type": "http", "url": "https://example.org/a.zip",
     "license": {"spdx-identifier": "CC-BY-4.0", "url": "https://example.org/license"}}
]}"#;

const TRANSITOUS_AT: &str = r#"{"sources": [
    {"name": "TestB", "type": "http", "url": "https://example.org/b.zip",
     "license": {"spdx-identifier": "CC0-1.0"}}
]}"#;

/// Writes a ZIP with the given files.
fn write_zip(path: &Path, files: &[(&str, &str)]) {
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, contents) in files {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(contents.as_bytes()).unwrap();
    }
    writer.finish().unwrap();
}

/// Creates a fresh temporary directory for one test.
fn temporary_directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("btt-builder-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

/// Finds a station of the bundle by name.
fn station(bundle: &LoadedTimetableBundle, name: &str) -> BundleStationIndex {
    (0..bundle.station_count())
        .map(|index| BundleStationIndex(u32::try_from(index).unwrap()))
        .find(|&index| bundle.station_name(index) == name)
        .unwrap_or_else(|| panic!("no station named {name}"))
}

#[test]
fn builds_a_merged_bundle_from_two_feeds() {
    let directory = temporary_directory("pipeline");
    let write = |name: &str, contents: &str| {
        let path = directory.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    };
    let config_path = write("builder.toml", CONFIG);
    let stopwords_path = write("stopwords.toml", STOPWORDS);
    let config = BuilderConfig::read(&config_path, &stopwords_path).unwrap();
    let first_feed_zip = directory.join("de_TestA.gtfs.zip");
    let second_feed_zip = directory.join("at_TestB.gtfs.zip");
    write_zip(&first_feed_zip, FEED_A);
    write_zip(&second_feed_zip, FEED_B);

    let inputs = PipelineInputs {
        feeds: vec![
            FeedInput {
                config: &config.feeds[0],
                zip_path: first_feed_zip,
                country_config_path: write("de.json", TRANSITOUS_DE),
            },
            FeedInput {
                config: &config.feeds[1],
                zip_path: second_feed_zip,
                country_config_path: write("at.json", TRANSITOUS_AT),
            },
        ],
        trainline_stations_csv: write("stations.csv", TRAINLINE_STATIONS),
        station_name_stopwords: &config.station_name_stopwords,
        window: ValidityWindow {
            start: NaiveDate::from_ymd_opt(2026, 10, 20).unwrap(),
            days: 14,
        },
        bundle_version: 2_026_102_000,
    };
    let database = WorkDatabase::open(None, &directory, false).unwrap();
    let outcome = run_pipeline(&database, &inputs).unwrap();
    assert_eq!(outcome.merge_counts.get(&(0, 1)), Some(&1));
    let bundle = LoadedTimetableBundle::from_bundle(&outcome.bundle).unwrap();

    let station_names: Vec<_> = (0..bundle.station_count())
        .map(|index| bundle.station_name(BundleStationIndex(u32::try_from(index).unwrap())))
        .collect();
    assert_eq!(station_names.len(), 4, "stations: {station_names:?}");
    let cologne = station(&bundle, "Köln Hbf");
    let frankfurt = station(&bundle, "Frankfurt (Main) Hbf");
    let frankfurt_south = station(&bundle, "Frankfurt (Main) Süd");
    let vienna = station(&bundle, "Wien Hbf");
    assert_eq!(bundle.station_trainline_id(cologne), Some(1));
    assert_eq!(bundle.station_time_zone(vienna), "Europe/Vienna");

    let footpath = bundle
        .footpaths_from(frankfurt_south)
        .find(|footpath| footpath.to_station == frankfurt);
    assert!(footpath.is_some_and(|footpath| footpath.duration_minutes > 15));

    // The bus never reaches the bundle, so Köln has only the RJ route.
    let routes_at_cologne: Vec<_> = bundle.route_stops_at(cologne).collect();
    assert_eq!(routes_at_cologne.len(), 1);
    let route = bundle.route(routes_at_cologne[0].raptor_route);
    assert_eq!(
        route.stop_count, 3,
        "the merged train runs Köln, Frankfurt, Wien"
    );
    assert_eq!(bundle.pattern_stop(&route, 2).station(), vienna);
    assert!(!bundle.pattern_stop(&route, 0).alighting_allowed());
    assert!(!bundle.pattern_stop(&route, 2).boarding_allowed());
    // 08:00 in Köln in summer time is 06:00 UTC.
    assert_eq!(bundle.stop_event(&route, 0, 0).departure.0, 360);

    // Feed B runs on days 0 to 2. Feed A skips day 1, so the merged copy runs on days 0 and 2
    // and feed B keeps day 1 on its own, with the same stops.
    let mut trip_days = Vec::new();
    for offset in 0..route.trip_count {
        let trip = BundleTripIndex(route.first_trip_index + offset);
        let days = bundle.trip_service_days(trip);
        trip_days.push(
            (0..3)
                .filter(|&day| days.runs_on(day))
                .collect::<Vec<u16>>(),
        );
        assert_eq!(bundle.trip_train_label(trip).0, "RJ");
    }
    trip_days.sort();
    assert_eq!(trip_days, [vec![0, 2], vec![1]]);

    let licenses: Vec<_> = bundle
        .feed_attributions()
        .map(|(_, license, _)| license.to_owned())
        .collect();
    assert_eq!(licenses, ["CC-BY-4.0", "CC0-1.0"]);
    assert_eq!(
        bundle.route(RaptorRouteIndex(0)).stop_count,
        2,
        "the regional train has its own route"
    );

    drop(database);
    let _ = std::fs::remove_dir_all(&directory);
}
