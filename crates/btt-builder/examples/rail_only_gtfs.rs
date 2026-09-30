//! Writes a copy of GTFS feeds with only their rail routes, the trips, stop times, stops and
//! calendars those use, and the agencies. MOTIS imports these for the comparison benchmark, so
//! both routers see the same trains, and the import fits in a laptop's memory.
//!
//! Run with
//! `cargo run --release -p btt-builder --example rail_only_gtfs -- <output directory> <feed.zip>...`.
//! Each feed becomes a ZIP of the same name in the output directory.

use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use btt_builder::database::{WorkDatabase, sql_string};
use btt_builder::feed_archive::{FeedArchive, column_or_null};
use btt_builder::load::RAIL_ROUTE_TYPE_FILTER;

fn main() {
    let mut arguments = std::env::args().skip(1);
    let output_directory = PathBuf::from(arguments.next().expect("pass the output directory"));
    std::fs::create_dir_all(&output_directory).expect("output directory is writable");
    for feed_path in arguments.map(PathBuf::from) {
        let started = std::time::Instant::now();
        let file_name = feed_path.file_name().expect("feed path names a file");
        let output_path = output_directory.join(file_name);
        write_rail_only_feed(&feed_path, &output_path);
        println!("{} in {:?}", output_path.display(), started.elapsed());
    }
}

/// Writes the rail part of one feed as a new ZIP.
fn write_rail_only_feed(feed_path: &Path, output_path: &Path) {
    let temp_directory = output_path.with_extension("work");
    let files_directory = output_path.with_extension("files");
    std::fs::create_dir_all(&files_directory).expect("work directory is writable");
    let database = WorkDatabase::open(None, &temp_directory, true).expect("DuckDB opens");
    let archive = FeedArchive::open(feed_path).expect("feed opens");
    let csv = |file_name: &str| {
        let location = archive
            .csv_location(file_name, &database.zip_reading)
            .expect("feed file is readable");
        format!(
            "read_csv({location}, header = true, all_varchar = true, delim = ',', quote = '\"', \
             escape = '\"', null_padding = true, strict_mode = false)"
        )
    };
    let stop_columns = archive
        .columns("stops.txt")
        .expect("stops.txt has a header");
    let parent_station = column_or_null(&stop_columns, "parent_station");

    database
        .run(&format!(
            "CREATE TABLE rail_routes AS SELECT * FROM {routes} WHERE {RAIL_ROUTE_TYPE_FILTER};
             CREATE TABLE rail_trips AS SELECT * FROM {trips}
                 WHERE route_id IN (SELECT route_id FROM rail_routes);
             CREATE TABLE rail_stop_times AS SELECT * FROM {stop_times}
                 WHERE trip_id IN (SELECT trip_id FROM rail_trips);
             CREATE TABLE all_stops AS SELECT * FROM {stops};
             CREATE TABLE used_stops AS
                 SELECT stop_id FROM all_stops
                 WHERE stop_id IN (SELECT stop_id FROM rail_stop_times);
             -- Stations of the used platforms, so a feed with parent stations stays valid.
             CREATE TABLE rail_stops AS
                 SELECT * FROM all_stops
                 WHERE stop_id IN (SELECT stop_id FROM used_stops)
                    OR stop_id IN (
                        SELECT {parent_station} FROM all_stops
                        WHERE stop_id IN (SELECT stop_id FROM used_stops));",
            routes = csv("routes.txt"),
            trips = csv("trips.txt"),
            stop_times = csv("stop_times.txt"),
            stops = csv("stops.txt"),
        ))
        .expect("rail tables build");

    let mut written = vec![
        copy_table(&database, "rail_routes", &files_directory, "routes.txt"),
        copy_table(&database, "rail_trips", &files_directory, "trips.txt"),
        copy_table(
            &database,
            "rail_stop_times",
            &files_directory,
            "stop_times.txt",
        ),
        copy_table(&database, "rail_stops", &files_directory, "stops.txt"),
    ];
    for (file_name, filter) in [
        ("agency.txt", None),
        ("feed_info.txt", None),
        (
            "calendar.txt",
            Some("service_id IN (SELECT service_id FROM rail_trips)"),
        ),
        (
            "calendar_dates.txt",
            Some("service_id IN (SELECT service_id FROM rail_trips)"),
        ),
    ] {
        if !archive.has(file_name) {
            continue;
        }
        let table = file_name.trim_end_matches(".txt");
        database
            .run(&format!(
                "CREATE TABLE {table} AS SELECT * FROM {} WHERE {};",
                csv(file_name),
                filter.unwrap_or("true")
            ))
            .expect("optional file loads");
        written.push(copy_table(&database, table, &files_directory, file_name));
    }
    drop(database);

    let mut zip = zip::ZipWriter::new(File::create(output_path).expect("output ZIP is writable"));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for path in written {
        let name = path
            .file_name()
            .expect("written file has a name")
            .to_string_lossy();
        zip.start_file(name, options).expect("ZIP entry starts");
        zip.write_all(&std::fs::read(&path).expect("written file reads"))
            .expect("ZIP entry writes");
    }
    zip.finish().expect("ZIP finishes");
    let _ = std::fs::remove_dir_all(&files_directory);
    let _ = std::fs::remove_dir_all(&temp_directory);
}

/// Writes a table as a GTFS file and returns its path.
fn copy_table(database: &WorkDatabase, table: &str, directory: &Path, file_name: &str) -> PathBuf {
    let path = directory.join(file_name);
    database
        .run(&format!(
            "COPY {table} TO {} (HEADER, DELIMITER ',');",
            sql_string(&path.display().to_string().replace('\\', "/"))
        ))
        .expect("table writes");
    path
}
