//! Writes the dictionary of known trains, `data/vagonweb-trains.txt`, from the sitemaps of
//! vagonWEB.
//!
//! Usage: `cargo run --release -p btt-builder --example vagonweb_dictionary -- [output] [cache]`
//!
//! The output defaults to `data/vagonweb-trains.txt` and the cache directory for the downloads
//! to `build-cache/vagonweb`. The site refuses requests from the servers of GitHub, so run this
//! on a normal connection and commit the result. Run it again when the site publishes the pages
//! of a new timetable year, which starts on the second Sunday of December.

use std::path::PathBuf;
use std::time::Duration;

use btt_builder::download::Downloader;
use btt_builder::layout_pages::{TrainDictionary, VAGONWEB_TRAIN_SITEMAP_URLS, parse_sitemap};

/// Pause between requests to vagonWEB, which asks for 15 seconds in its robots.txt.
const REQUEST_GAP: Duration = Duration::from_secs(15);

fn main() {
    let mut arguments = std::env::args_os().skip(1).map(PathBuf::from);
    let output = arguments
        .next()
        .unwrap_or_else(|| PathBuf::from("data/vagonweb-trains.txt"));
    let cache = arguments
        .next()
        .unwrap_or_else(|| PathBuf::from("build-cache/vagonweb"));
    let downloader = Downloader::with_request_gap(cache, REQUEST_GAP).expect("a usable cache");
    let mut trains = Vec::new();
    for (position, url) in VAGONWEB_TRAIN_SITEMAP_URLS.iter().enumerate() {
        let path = downloader
            .fetch(url, &format!("sitemap-{position}.xml"))
            .expect("a downloaded sitemap");
        let text = std::fs::read_to_string(path).expect("a readable sitemap");
        trains.extend(parse_sitemap(&text));
    }
    let dictionary = TrainDictionary::new(trains);
    std::fs::write(&output, dictionary.to_text()).expect("a writable output file");
    println!(
        "{} trains written to {}",
        dictionary.train_count(),
        output.display()
    );
}
