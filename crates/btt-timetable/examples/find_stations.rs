//! Prints the bundle stations whose name contains a text, with their coordinates, Trainline ID and
//! the number of routes and footpaths at each.
//!
//! Run with `cargo run --release -p btt-timetable --example find_stations -- <bundle> <text>`.

use btt_timetable::{BundleStationIndex, LoadedTimetableBundle};

fn main() {
    let mut arguments = std::env::args().skip(1);
    let bundle_path = arguments.next().expect("pass the bundle path");
    let text = arguments
        .next()
        .expect("pass a text to search")
        .to_lowercase();
    let bundle = LoadedTimetableBundle::open(bundle_path.as_ref()).expect("bundle loads");
    for index in 0..bundle.station_count() {
        let station = BundleStationIndex(u32::try_from(index).expect("station index fits"));
        let name = bundle.station_name(station);
        if !name.to_lowercase().contains(&text) {
            continue;
        }
        let coordinates = bundle.station_coordinates(station);
        println!(
            "{index}\t{name}\t{:.5}, {:.5}\tTrainline {:?}\t{} routes\t{} footpaths",
            coordinates.latitude.degrees(),
            coordinates.longitude.degrees(),
            bundle.station_trainline_id(station),
            bundle.route_stops_at(station).len(),
            bundle.footpaths_from(station).len(),
        );
    }
}
