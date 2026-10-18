//! Lists the operators whose trains stop before the end of a bundle, with their last day. The
//! app warns about these operators when a trip runs past that day.
//!
//! Run `cargo run --release -p btt-planner --example operator_ends -- <bundle>`.

use std::sync::Arc;

use btt_geo::TimeZoneFinder;
use btt_planner::{NoPrice, TimetableBundleRouter};

fn main() {
    let path = std::env::args().nth(1).expect("pass the bundle");
    let router =
        TimetableBundleRouter::open(path.as_ref(), Arc::new(TimeZoneFinder::new()), NoPrice)
            .expect("bundle opens");
    let (first, last) = router.validity_window();
    println!("Bundle covers {first} to {last}");
    for end in router.operator_ends() {
        println!("{}  {}  {}", end.last_departure_date, end.country, end.name);
    }
}
