//! Compares the train labels of a bundle with the train pages of vagonWEB, to propose the
//! category and operator tables in `data/vagonweb.toml`.
//!
//! Usage: `cargo run --release -p btt-builder --example vagonweb_survey -- <bundle> <sitemap>...`
//!
//! For each category and agency of the bundle with enough trains, it prints the vagonWEB
//! operator and category pairs that have a page for the most of its train numbers.

use std::collections::BTreeMap;
use std::path::PathBuf;

use btt_builder::layout_pages::{TrainDictionary, parse_sitemap};
use btt_timetable::LoadedTimetableBundle;

/// Fewest trains a category and agency need to appear in the survey.
const MINIMUM_TRAINS: usize = 30;

fn main() {
    let arguments: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let (bundle_path, sitemap_paths) = arguments.split_first().expect("a bundle path");
    let bundle = LoadedTimetableBundle::open(bundle_path).expect("a readable bundle");
    let mut trains = Vec::new();
    for path in sitemap_paths {
        trains.extend(parse_sitemap(
            &std::fs::read_to_string(path).expect("a readable sitemap"),
        ));
    }
    let index = TrainDictionary::new(trains);
    println!("{} train pages", index.train_count());

    // Pages by number, so each train number finds the pairs that have a page for it.
    let mut pairs_by_number: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for (operator, category, number, _year) in index.trains() {
        pairs_by_number
            .entry(number)
            .or_default()
            .push((operator, category));
    }

    let mut labels_by_group: BTreeMap<(&str, &str), Vec<&str>> = BTreeMap::new();
    for (category, number, agency) in bundle.train_labels() {
        labels_by_group
            .entry((category, agency))
            .or_default()
            .push(number.trim_start_matches('0'));
    }
    for ((category, agency), numbers) in &labels_by_group {
        if numbers.len() < MINIMUM_TRAINS {
            continue;
        }
        let mut hits: BTreeMap<(&str, &str), usize> = BTreeMap::new();
        for number in numbers {
            for pair in pairs_by_number.get(number).into_iter().flatten() {
                *hits.entry(*pair).or_default() += 1;
            }
        }
        let mut best: Vec<_> = hits.into_iter().collect();
        best.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        let summary: Vec<String> = best
            .iter()
            .take(3)
            .map(|((operator, vagonweb_category), count)| {
                format!("{operator}/{vagonweb_category} {count}")
            })
            .collect();
        println!(
            "{category:>14} | {agency:<40} | {:>6} | {}",
            numbers.len(),
            summary.join(", ")
        );
    }
}
