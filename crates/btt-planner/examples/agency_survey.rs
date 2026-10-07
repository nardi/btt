//! Lists the agencies of a bundle with their number of trips and their most common categories,
//! to keep the rules of `data/operators.toml` up to date with the feeds.
//!
//! Run `cargo run --release -p btt-planner --example agency_survey -- <bundle>`.

use std::collections::HashMap;

use btt_timetable::{BundleTripIndex, LoadedTimetableBundle};

fn main() {
    let path = std::env::args().nth(1).expect("pass the bundle");
    let bundle = LoadedTimetableBundle::open(path.as_ref()).expect("bundle opens");
    // Trips of each agency, and of each of its categories.
    let mut agencies: HashMap<String, (usize, HashMap<String, usize>)> = HashMap::new();
    for index in 0..bundle.trip_count() {
        let trip = BundleTripIndex(u32::try_from(index).expect("trip indices fit in u32"));
        let (category, _, agency) = bundle.trip_train_label(trip);
        let entry = agencies.entry(agency.to_owned()).or_default();
        entry.0 += 1;
        *entry.1.entry(category.to_owned()).or_default() += 1;
    }
    let mut agencies: Vec<_> = agencies.into_iter().collect();
    agencies.sort_by_key(|(_, (count, _))| std::cmp::Reverse(*count));
    println!("{} agencies", agencies.len());
    for (agency, (count, categories)) in agencies {
        let mut categories: Vec<_> = categories.into_iter().collect();
        categories.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        let top: Vec<String> = categories
            .iter()
            .take(5)
            .map(|(category, count)| format!("{category}:{count}"))
            .collect();
        println!("{count:>8}  {agency:?}  {}", top.join(" "));
    }
}
