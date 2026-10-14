//! Runs the whole pipeline on two small fixture feeds in an in-memory database.
//!
//! Feed A uses DHID-style stop IDs without parent stations and runs the ICE 105 from Köln to
//! Frankfurt. Feed B uses parent stations and runs the same train from Köln through Frankfurt
//! to Wien, under the label RJ 105. The build must merge the two copies, drop feed A's bus
//! route, match all stations to the Trainline fixture and add a footpath between the two
//! Frankfurt stations.
//!
//! A second pair of feeds shares DHID station keys, as DELFI and VBB do.

use std::io::Write;
use std::path::{Path, PathBuf};

use btt_builder::config::BuilderConfig;
use btt_builder::database::WorkDatabase;
use btt_builder::load::ValidityWindow;
use btt_builder::{FeedInput, PipelineInputs, PipelineOutcome, run_pipeline};
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
id;name;uic;latitude;longitude;is_city;db_id;time_zone;same_as;parent_station_id;uic8_sncf;entur_id
1;Köln Hbf;8000207;50.943029;6.958729;f;8000207;Europe/Berlin;;;;
2;Frankfurt (Main) Hbf;8000105;50.107145;8.663789;f;8000105;Europe/Berlin;;;;
3;Wien Hbf;8103000;48.185184;16.376413;f;;Europe/Vienna;;;;
4;Frankfurt (Main) Süd;8002041;50.099365;8.686457;f;8002041;Europe/Berlin;;;;
5;Frankfurt;;50.110922;8.682127;t;;Europe/Berlin;;;;
6;Köln Hauptbahnhof;;50.943;6.959;f;;Europe/Berlin;1;;;
7;Köln Hbf Gleis 1;;50.9429;6.9587;f;;;;1;;
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

/// A fixture feed with the Transitous country configuration that holds its license.
struct FixtureFeed<'a> {
    /// Files of the feed ZIP.
    files: &'a [(&'a str, &'a str)],
    /// File name of the country configuration, such as `de.json`.
    country_file_name: &'a str,
    /// Contents of the country configuration.
    country_config: &'a str,
}

/// Runs the pipeline on fixture feeds in a fresh temporary directory. The feeds pair up with
/// the feeds of the configuration in order.
fn run_fixture_pipeline(
    test_name: &str,
    config_text: &str,
    feeds: &[FixtureFeed<'_>],
    trainline_stations: &str,
) -> PipelineOutcome {
    let directory = temporary_directory(test_name);
    let write = |name: &str, contents: &str| {
        let path = directory.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    };
    let config_path = write("builder.toml", config_text);
    let stopwords_path = write("stopwords.toml", STOPWORDS);
    let config = BuilderConfig::read(&config_path, &stopwords_path).unwrap();
    let feed_inputs = config
        .feeds
        .iter()
        .zip(feeds)
        .map(|(feed_config, feed)| {
            let zip_path = directory.join(format!("{}.gtfs.zip", feed_config.mirror_name));
            write_zip(&zip_path, feed.files);
            FeedInput {
                config: feed_config,
                zip_path,
                country_config_path: write(feed.country_file_name, feed.country_config),
            }
        })
        .collect();
    let inputs = PipelineInputs {
        feeds: feed_inputs,
        trainline_stations_csv: write("stations.csv", trainline_stations),
        station_name_stopwords: &config.station_name_stopwords,
        night_train_categories: &config.night_train_categories,
        window: ValidityWindow {
            start: NaiveDate::from_ymd_opt(2026, 10, 20).unwrap(),
            days: 14,
        },
        bundle_version: 2_026_102_000,
    };
    let database = WorkDatabase::open(None, &directory, false).unwrap();
    let outcome = run_pipeline(&database, &inputs).unwrap();
    drop(database);
    let _ = std::fs::remove_dir_all(&directory);
    outcome
}

#[test]
fn builds_a_merged_bundle_from_two_feeds() {
    let outcome = run_fixture_pipeline(
        "pipeline",
        CONFIG,
        &[
            FixtureFeed {
                files: FEED_A,
                country_file_name: "de.json",
                country_config: TRANSITOUS_DE,
            },
            FixtureFeed {
                files: FEED_B,
                country_file_name: "at.json",
                country_config: TRANSITOUS_AT,
            },
        ],
        TRAINLINE_STATIONS,
    );
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
}

const SHARED_KEY_CONFIG: &str = r#"
validity_window_days = 14

[[feed]]
mirror_name = "de_TestC"
station_key_pattern = '^(de:\d+:\d+)'
station_key_namespace = "dhid"
[feed.train_label]
field = "trip_short_name"
pattern = '^0*(?<number>\d+)$'
category_field = "route_short_name"

[[feed]]
mirror_name = "de_TestD"
station_key_pattern = '^(de:\d+:\d+)'
station_key_namespace = "dhid"
[feed.train_label]
field = "trip_short_name"
pattern = '^0*(?<number>\d+)$'
category_field = "route_short_name"
"#;

/// Two stations with local transit names and one station Trainline does not list.
const FEED_C: &[(&str, &str)] = &[
    (
        "agency.txt",
        "agency_id,agency_name,agency_url,agency_timezone\n\
         db,DB Regio AG,https://www.bahn.de,Europe/Berlin\n",
    ),
    (
        "routes.txt",
        "route_id,agency_id,route_short_name,route_type\nre1,db,RE1,106\n",
    ),
    (
        "trips.txt",
        "route_id,service_id,trip_id,trip_short_name\nre1,daily,re-1,4711\n",
    ),
    (
        "stops.txt",
        "stop_id,stop_name,stop_lat,stop_lon,parent_station\n\
         de:11000:900003200:1:1,S+U Berlin Hauptbahnhof,52.5253,13.3694,\n\
         de:11000:900120005:1:1,S Ostbahnhof (Berlin),52.5103,13.4351,\n\
         de:11000:900120004:1:1,S Warschauer Str. (Berlin),52.5059,13.4492,\n",
    ),
    (
        "stop_times.txt",
        "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
         re-1,10:00:00,10:00:00,de:11000:900003200:1:1,1\n\
         re-1,10:09:00,10:10:00,de:11000:900120005:1:1,2\n\
         re-1,10:13:00,10:13:00,de:11000:900120004:1:1,3\n",
    ),
    (
        "calendar.txt",
        "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
         daily,1,1,1,1,1,1,1,20261001,20261031\n",
    ),
];

/// The same stations under other platform IDs, served by a train with another number.
const FEED_D: &[(&str, &str)] = &[
    (
        "agency.txt",
        "agency_id,agency_name,agency_url,agency_timezone\n\
         sbahn,S-Bahn Berlin GmbH,https://www.s-bahn-berlin.de,Europe/Berlin\n",
    ),
    (
        "routes.txt",
        "route_id,agency_id,route_short_name,route_type\ns5,sbahn,S5,109\n",
    ),
    (
        "trips.txt",
        "route_id,service_id,trip_id,trip_short_name\ns5,daily,s5-1,5047\n",
    ),
    (
        "stops.txt",
        "stop_id,stop_name,stop_lat,stop_lon,parent_station\n\
         de:11000:900120004::2,S Warschauer Str. (Berlin),52.5058,13.4490,\n\
         de:11000:900120005::3,S Ostbahnhof (Berlin),52.5102,13.4348,\n",
    ),
    (
        "stop_times.txt",
        "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
         s5-1,11:00:00,11:00:00,de:11000:900120004::2,1\n\
         s5-1,11:03:00,11:03:00,de:11000:900120005::3,2\n",
    ),
    (
        "calendar.txt",
        "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
         daily,1,1,1,1,1,1,1,20261001,20261031\n",
    ),
];

const BERLIN_TRAINLINE_STATIONS: &str = "\
id;name;uic;latitude;longitude;is_city;db_id;time_zone;same_as;parent_station_id;uic8_sncf;entur_id
7540;Berlin Ostbahnhof;8003004;52.510972;13.434567;f;8010255;Europe/Berlin;;;;
7630;Berlin Hbf;8065969;52.534589;13.369548;f;8011160;Europe/Berlin;;;;
";

const TRANSITOUS_DE_SHARED: &str = r#"{"sources": [
    {"name": "TestC", "type": "http", "url": "https://example.org/c.zip",
     "license": {"spdx-identifier": "CC-BY-4.0"}},
    {"name": "TestD", "type": "http", "url": "https://example.org/d.zip",
     "license": {"spdx-identifier": "CC-BY-4.0"}}
]}"#;

#[test]
fn feeds_with_a_shared_key_namespace_share_stations() {
    let feed = |files| FixtureFeed {
        files,
        country_file_name: "de.json",
        country_config: TRANSITOUS_DE_SHARED,
    };
    let outcome = run_fixture_pipeline(
        "shared-keys",
        SHARED_KEY_CONFIG,
        &[feed(FEED_C), feed(FEED_D)],
        BERLIN_TRAINLINE_STATIONS,
    );
    let bundle = LoadedTimetableBundle::from_bundle(&outcome.bundle).unwrap();
    let mut station_names: Vec<_> = (0..bundle.station_count())
        .map(|index| bundle.station_name(BundleStationIndex(u32::try_from(index).unwrap())))
        .collect();
    station_names.sort_unstable();
    // "S+U Berlin Hauptbahnhof" and "S Ostbahnhof (Berlin)" match Trainline once the mode
    // prefix is dropped and the city moves to the front. The Trainline Hbf sits 1 km off, which
    // only an equal name bridges. Warschauer Straße matches nothing, but both feeds use its
    // key, so it is one station.
    assert_eq!(
        station_names,
        [
            "Berlin Hbf",
            "Berlin Ostbahnhof",
            "S Warschauer Str. (Berlin)"
        ]
    );
    let warschauer = station(&bundle, "S Warschauer Str. (Berlin)");
    assert_eq!(bundle.route_stops_at(warschauer).len(), 2);
}

const LOCATION_CONFIG: &str = r#"
validity_window_days = 14

[[feed]]
mirror_name = "de_TestE"
[feed.train_label]
field = "trip_short_name"
pattern = '^(?<number>\d+)$'
category_field = "route_short_name"

[[feed]]
mirror_name = "de_TestF"
[feed.train_label]
field = "trip_short_name"
pattern = '^(?<number>\d+)$'
category_field = "route_short_name"
"#;

/// Feed E runs train 100 from Alpha Nord through Beta to Gamma. Alpha Bus lies 70 m west of
/// Alpha Nord in the same feed.
const FEED_E: &[(&str, &str)] = &[
    (
        "agency.txt",
        "agency_id,agency_name,agency_url,agency_timezone\n\
         e,Rail E,https://example.org,Europe/Berlin\n",
    ),
    (
        "routes.txt",
        "route_id,agency_id,route_short_name,route_type\nre,e,RE,106\n",
    ),
    (
        "trips.txt",
        "route_id,service_id,trip_id,trip_short_name\n\
         re,daily,e-100,100\n\
         re,daily,e-200,200\n",
    ),
    (
        "stops.txt",
        "stop_id,stop_name,stop_lat,stop_lon\n\
         alpha,Alpha Nord,50.0000,8.0000\n\
         alpha-bus,Alpha Bus,50.0000,7.9990\n\
         beta,Beta,50.1000,8.1000\n\
         gamma,Gamma,50.2000,8.2000\n",
    ),
    (
        "stop_times.txt",
        "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
         e-100,10:00:00,10:00:00,alpha,1\n\
         e-100,10:30:00,10:31:00,beta,2\n\
         e-100,11:00:00,11:00:00,gamma,3\n\
         e-200,12:00:00,12:00:00,alpha-bus,1\n\
         e-200,12:30:00,12:30:00,beta,2\n",
    ),
    (
        "calendar.txt",
        "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
         daily,1,1,1,1,1,1,1,20261001,20261031\n",
    ),
];

/// Feed F runs the same train 100 under other stop names and goes on to Delta. Its Alpha lies
/// 60 m east of feed E's Alpha Nord, its Beta Hbf 300 m north of feed E's Beta and its Gamma
/// Süd 300 m south of feed E's Gamma.
const FEED_F: &[(&str, &str)] = &[
    (
        "agency.txt",
        "agency_id,agency_name,agency_url,agency_timezone\n\
         f,Rail F,https://example.org,Europe/Berlin\n",
    ),
    (
        "routes.txt",
        "route_id,agency_id,route_short_name,route_type\nre,f,RE,106\n",
    ),
    (
        "trips.txt",
        "route_id,service_id,trip_id,trip_short_name\nre,daily,f-100,100\n",
    ),
    (
        "stops.txt",
        "stop_id,stop_name,stop_lat,stop_lon\n\
         a,Alpha,50.0000,8.00084\n\
         b,Beta Hbf,50.1027,8.1000\n\
         g,Gamma Süd,50.1973,8.2000\n\
         d,Delta,50.3000,8.3000\n",
    ),
    (
        "stop_times.txt",
        "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
         f-100,10:00:00,10:00:00,a,1\n\
         f-100,10:30:00,10:31:00,b,2\n\
         f-100,11:00:00,11:01:00,g,3\n\
         f-100,11:30:00,11:30:00,d,4\n",
    ),
    (
        "calendar.txt",
        "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
         daily,1,1,1,1,1,1,1,20261001,20261031\n",
    ),
];

const TRANSITOUS_DE_LOCATION: &str = r#"{"sources": [
    {"name": "TestE", "type": "http", "url": "https://example.org/e.zip",
     "license": {"spdx-identifier": "CC0-1.0"}},
    {"name": "TestF", "type": "http", "url": "https://example.org/f.zip",
     "license": {"spdx-identifier": "CC0-1.0"}}
]}"#;

const NO_TRAINLINE_STATIONS: &str = "id;name;uic;latitude;longitude;is_city;db_id;time_zone;same_as;parent_station_id;uic8_sncf;entur_id\n";

#[test]
fn stations_and_trains_merge_by_location() {
    let feed = |files| FixtureFeed {
        files,
        country_file_name: "de.json",
        country_config: TRANSITOUS_DE_LOCATION,
    };
    let outcome = run_fixture_pipeline(
        "location",
        LOCATION_CONFIG,
        &[feed(FEED_E), feed(FEED_F)],
        NO_TRAINLINE_STATIONS,
    );
    let bundle = LoadedTimetableBundle::from_bundle(&outcome.bundle).unwrap();
    let mut station_names: Vec<_> = (0..bundle.station_count())
        .map(|index| bundle.station_name(BundleStationIndex(u32::try_from(index).unwrap())))
        .collect();
    station_names.sort_unstable();
    // Alpha and Alpha Nord lie 60 m apart in different feeds and merge. Alpha Bus lies 70 m
    // from Alpha Nord but in the same feed, so it stays apart. Beta and Beta Hbf lie 300 m
    // apart and stay two stations, as do Gamma and Gamma Süd.
    assert_eq!(
        station_names,
        [
            "Alpha",
            "Alpha Bus",
            "Beta",
            "Beta Hbf",
            "Delta",
            "Gamma",
            "Gamma Süd"
        ]
    );

    // Train 100 still merges. Only Alpha is the same station in both copies, but Beta and Gamma
    // lie within 600 m of their counterparts. The window spans the
    // end of summer time, so each feed has a summer and a winter copy, and both pairs merge.
    assert_eq!(outcome.merge_counts.get(&(0, 1)), Some(&2));
    let alpha = station(&bundle, "Alpha");
    let delta = station(&bundle, "Delta");
    let merged_route = bundle
        .route_stops_at(alpha)
        .map(|stop| bundle.route(stop.raptor_route))
        .find(|route| route.stop_count == 4)
        .expect("the merged train runs from Alpha to Delta");
    assert_eq!(bundle.pattern_stop(&merged_route, 3).station(), delta);
    assert_eq!(merged_route.trip_count, 2);
}

const CODE_CONFIG: &str = r#"
validity_window_days = 14

[[feed]]
mirror_name = "de_TestG"
station_code = "uic_in_stop_id"
[feed.train_label]
field = "trip_short_name"
pattern = '^(?<number>\d+)$'
category_field = "route_short_name"
"#;

/// Feed G runs train 300 from Bad Schandau to Dresden Hbf, with their UIC codes as stop IDs.
const FEED_G: &[(&str, &str)] = &[
    (
        "agency.txt",
        "agency_id,agency_name,agency_url,agency_timezone\n\
         g,Rail G,https://example.org,Europe/Berlin\n",
    ),
    (
        "routes.txt",
        "route_id,agency_id,route_short_name,route_type\nre,g,RE,106\n",
    ),
    (
        "trips.txt",
        "route_id,service_id,trip_id,trip_short_name\nre,daily,g-300,300\n",
    ),
    (
        "stops.txt",
        "stop_id,stop_name,stop_lat,stop_lon\n\
         8010022,Bad Schandau,50.9189,14.1389\n\
         8010085,Dresden Hbf,51.0406,13.7320\n",
    ),
    (
        "stop_times.txt",
        "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
         g-300,10:00:00,10:00:00,8010022,1\n\
         g-300,11:00:00,11:00:00,8010085,2\n",
    ),
    (
        "calendar.txt",
        "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
         daily,1,1,1,1,1,1,1,20261001,20261031\n",
    ),
];

const TRANSITOUS_DE_CODE: &str = r#"{"sources": [
    {"name": "TestG", "type": "http", "url": "https://example.org/g.zip",
     "license": {"spdx-identifier": "CC0-1.0"}}
]}"#;

/// The Trainline uic column of the two German stations holds the code of another station, as it
/// does for about half of the German rows. The row of Oberhausen-Osterfeld Süd carries the UIC
/// code of Bad Schandau and lies far away.
const MISLABELLED_UIC_STATIONS: &str = "\
id;name;uic;latitude;longitude;is_city;db_id;time_zone;same_as;parent_station_id;uic8_sncf;entur_id
1;Bad Schandau;8006006;50.918713;14.139861;f;8010022;Europe/Berlin;;;;
2;Oberhausen-Osterfeld Süd;8010022;51.499704;6.885224;f;8004541;Europe/Berlin;;;;
3;Dresden Hbf;8006050;51.040562;13.732038;f;8010085;Europe/Berlin;;;;
";

#[test]
fn german_uic_codes_match_on_the_eva_number() {
    let outcome = run_fixture_pipeline(
        "german_codes",
        CODE_CONFIG,
        &[FixtureFeed {
            files: FEED_G,
            country_file_name: "de.json",
            country_config: TRANSITOUS_DE_CODE,
        }],
        MISLABELLED_UIC_STATIONS,
    );
    let bundle = LoadedTimetableBundle::from_bundle(&outcome.bundle).unwrap();
    let bad_schandau = station(&bundle, "Bad Schandau");
    assert_eq!(bundle.station_trainline_id(bad_schandau), Some(1));
    let dresden = station(&bundle, "Dresden Hbf");
    assert_eq!(bundle.station_trainline_id(dresden), Some(3));
}
