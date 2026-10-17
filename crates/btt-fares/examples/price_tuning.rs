//! Tunes the priors of the price model by cross-validation on a file of live prices.
//!
//! The observations are split into five folds by a hash of their route, so a route is never in
//! the training and the test part at once. The shipped model learns online from the training
//! part of each fold, as in the app, and predicts the test part. The split is repeated with
//! several seeds of the hash to make the scores less noisy.
//!
//! The report shows the median error as a factor, the mean absolute error of the log price, the
//! share of test prices inside the range and the mean negative log density of the log price
//! under the predicted normal distribution, which rewards a range that is both narrow and
//! right.
//!
//! Run `cargo run --release -p btt-fares --example price_tuning -- <prices.jsonl> [mode]`, where
//! the mode is `variants` (the default), `tune` or `groups`.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use btt_fares::{PriceModel, RideFacts, seed_observations};
use btt_model::PriceObservation;

/// The text of the shipped configuration.
const SHIPPED_CONFIG: &str = include_str!("../../../data/fares/price_model.toml");

/// Number of folds.
const FOLDS: u64 = 5;

/// Number of different splits into folds.
const SPLITS: u64 = 3;

/// Half the width of the 80 percent range in standard deviations.
const INTERVAL_HALF_WIDTH: f64 = 1.281_551_565_5;

/// Errors and hits of the predictions for the test parts.
#[derive(Default, Clone)]
struct Score {
    /// Absolute errors of the log price.
    errors: Vec<f64>,
    /// Negative log densities of the log price.
    densities: Vec<f64>,
    /// How many test prices fell inside the range.
    inside: usize,
}

impl Score {
    /// Adds a prediction of the log price as a mean and a standard deviation.
    fn add(&mut self, log_price: f64, mean: f64, standard_deviation: f64) {
        let error = (log_price - mean).abs();
        self.errors.push(error);
        let z = error / standard_deviation;
        self.densities
            .push(0.5 * z * z + standard_deviation.ln() + 0.5 * (2.0 * std::f64::consts::PI).ln());
        self.inside += usize::from(error <= INTERVAL_HALF_WIDTH * standard_deviation);
    }

    /// Returns the mean absolute error of the log price.
    fn mean_error(&self) -> f64 {
        mean(&self.errors)
    }

    /// Returns the mean negative log density.
    fn mean_density(&self) -> f64 {
        mean(&self.densities)
    }

    /// Prints the score as one row.
    fn print(&self, name: &str) {
        let mut errors = self.errors.clone();
        errors.sort_by(f64::total_cmp);
        let median = errors.get(errors.len() / 2).copied().unwrap_or(0.0);
        // Counts are small enough for `f64`.
        #[expect(clippy::cast_precision_loss)]
        let inside = self.inside as f64 / self.errors.len().max(1) as f64;
        println!(
            "{name:<44} {:>5} {:>7.3}x {:>7.4} {:>6.1}% {:>7.4}",
            self.errors.len(),
            median.exp(),
            self.mean_error(),
            inside * 100.0,
            self.mean_density(),
        );
    }
}

/// Returns the mean of some values, or 0 for none.
fn mean(values: &[f64]) -> f64 {
    // Counts are small enough for `f64`.
    #[expect(clippy::cast_precision_loss)]
    let count = values.len().max(1) as f64;
    values.iter().sum::<f64>() / count
}

/// An observation with what the report groups it by.
struct Labelled {
    /// The live price.
    observation: PriceObservation,
    /// The country, the operator and the kind the shipped model gives the run.
    groups: [String; 3],
}

/// Returns the first seed of the splits, from `BTT_SPLIT_OFFSET`, so a tuned configuration can
/// be checked on splits it was not tuned on.
fn split_offset() -> u64 {
    std::env::var("BTT_SPLIT_OFFSET")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(0)
}

/// How the prices are split into folds.
#[derive(Clone, Copy)]
enum Folds {
    /// A route is in one fold only.
    Route,
    /// An operator is in one fold only, so the test part holds operators the training part has
    /// no price of.
    Operator,
    /// A country is in one fold only, so the test part holds countries the training part has no
    /// price of.
    Country,
}

/// Returns the folds that `BTT_FOLD_BY` names, `route`, `operator` or `country`, by route
/// when unset.
fn folds_from_environment() -> Folds {
    match std::env::var("BTT_FOLD_BY").as_deref() {
        Ok("operator") => Folds::Operator,
        Ok("country") => Folds::Country,
        _ => Folds::Route,
    }
}

/// Returns the fold of an observation in one split.
fn fold_of(labelled: &Labelled, split: u64, folds: Folds) -> u64 {
    let mut hasher = DefaultHasher::new();
    (split + split_offset()).hash(&mut hasher);
    let run = &labelled.observation.run;
    match folds {
        Folds::Route => (&run.from_station, &run.to_station).hash(&mut hasher),
        Folds::Operator => run
            .rides
            .iter()
            .map(|ride| &ride.operator)
            .collect::<Vec<_>>()
            .hash(&mut hasher),
        Folds::Country => labelled.groups[0].hash(&mut hasher),
    }
    hasher.finish() % FOLDS
}

/// Scores a configuration over every split, with one score for all prices and one per group.
fn score(
    config_text: &str,
    observations: &[Labelled],
    folds: Folds,
) -> (Score, BTreeMap<String, Score>) {
    let mut total = Score::default();
    let mut groups: BTreeMap<String, Score> = BTreeMap::new();
    for split in 0..SPLITS {
        for fold in 0..FOLDS {
            let mut model = PriceModel::with_config(config_text).expect("the configuration loads");
            for labelled in observations
                .iter()
                .filter(|labelled| fold_of(labelled, split, folds) != fold)
            {
                model.observe(&labelled.observation);
            }
            for labelled in observations
                .iter()
                .filter(|labelled| fold_of(labelled, split, folds) == fold)
            {
                let observation = &labelled.observation;
                let rides: Vec<RideFacts<'_>> = observation
                    .run
                    .rides
                    .iter()
                    .map(RideFacts::of_description)
                    .collect();
                let Some(estimate) = model.estimate(&rides) else {
                    continue;
                };
                let mean = estimate.median.ln();
                let standard_deviation =
                    (estimate.high.ln() - estimate.low.ln()) / (2.0 * INTERVAL_HALF_WIDTH);
                let log_price = observation.price.euro().ln();
                total.add(log_price, mean, standard_deviation);
                for group in &labelled.groups {
                    groups.entry(group.clone()).or_default().add(
                        log_price,
                        mean,
                        standard_deviation,
                    );
                }
            }
        }
    }
    (total, groups)
}

/// Returns a configuration with some values changed, each named by its dotted path such as
/// `spread.country`.
fn configured(base: &str, changes: &[(&str, f64)]) -> String {
    let mut table: toml::Table = toml::from_str(base).expect("the configuration parses");
    for (path, value) in changes {
        let mut parts: Vec<&str> = path.split('.').collect();
        let last = parts.pop().expect("a path has a name");
        let mut current = &mut table;
        for part in parts {
            current = current
                .entry(part)
                .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                .as_table_mut()
                .expect("the path leads through tables");
        }
        current.insert(last.to_owned(), toml::Value::Float(*value));
    }
    toml::to_string(&table).expect("the configuration serializes")
}

/// Returns a value of a configuration by its dotted path, or 0 when it is missing.
fn value_of(config_text: &str, path: &str) -> f64 {
    let table: toml::Table = toml::from_str(config_text).expect("the configuration parses");
    let mut current = &toml::Value::Table(table);
    for part in path.split('.') {
        match current.get(part) {
            Some(next) => current = next,
            None => return 0.0,
        }
    }
    // Integers in the configuration are small, such as a weight of 20.
    #[expect(clippy::cast_precision_loss)]
    let value = current
        .as_float()
        .or_else(|| current.as_integer().map(|value| value as f64))
        .unwrap_or(0.0);
    value
}

/// The values the tuning tries, by their dotted path.
const TUNED: [&str; 26] = [
    "noise.standard_deviation",
    "noise.prior_weight",
    "noise.kind_weight",
    "noise.operator_weight",
    "outlier_limit",
    "default_shift_kilometres",
    "global.intercept_standard_deviation",
    "global.slope_standard_deviation",
    "spread.country",
    "spread.country_slope",
    "spread.operator",
    "spread.operator_slope",
    "spread.operator_base",
    "spread.operator_curve",
    "spread.kind",
    "spread.kind_slope",
    "spread.country_kind",
    "spread.category",
    "spread.border",
    "spread.base",
    "spread.country_base",
    "spread.country_curve",
    "spread.country_category",
    "spread.border_base",
    "spread.border_operator",
    "interval_share",
];

/// Scores a configuration with folds by route and with folds by operator.
fn score_both(config_text: &str, observations: &[Labelled]) -> (Score, Score) {
    (
        score(config_text, observations, Folds::Route).0,
        score(config_text, observations, Folds::Operator).0,
    )
}

/// The share that the score on operators without prices counts with in the tuning, next to the
/// score on routes without prices.
const OPERATOR_SHARE: f64 = 0.5;

/// Tunes the values in `TUNED` one at a time, keeping a change when it lowers the objective,
/// for a few rounds over all of them. The objective is the mean error, or the mean negative log
/// density, of the folds by route plus `OPERATOR_SHARE` of that of the folds by operator.
fn tune(observations: &[Labelled], start: String, by_density: bool) {
    let measure = |score: &Score| {
        if by_density {
            score.mean_density()
        } else {
            score.mean_error()
        }
    };
    let objective =
        |(route, operator): &(Score, Score)| measure(route) + OPERATOR_SHARE * measure(operator);
    let print = |(route, operator): &(Score, Score), name: &str| {
        route.print(&format!("{name}, routes"));
        operator.print(&format!("{name}, operators"));
    };
    let mut config = start;
    let mut best = score_both(&config, observations);
    print(&best, "start");
    for round in 0..3 {
        let mut improved = false;
        for path in TUNED {
            // The mean error tunes the terms, and the density then tunes the noise, which
            // mostly sets the width of the range. The interval share is reported below.
            if path == "interval_share" || by_density != path.starts_with("noise.") {
                continue;
            }
            let current = value_of(&config, path);
            let candidates: Vec<f64> = if current == 0.0 {
                vec![0.05, 0.1, 0.2, 0.4]
            } else {
                [0.0, 0.5, 0.7, 1.4, 2.0]
                    .iter()
                    .map(|factor| current * factor)
                    .collect()
            };
            for candidate in candidates {
                // Only optional terms may drop out, and a noise of 0 breaks the model.
                if candidate == 0.0 && !path.starts_with("spread.") {
                    continue;
                }
                let changed = configured(&config, &[(path, candidate)]);
                let Ok(_) = PriceModel::with_config(&changed) else {
                    continue;
                };
                let trial = score_both(&changed, observations);
                if objective(&trial) < objective(&best) - 1e-4 {
                    print(&trial, &format!("{round}: {path} = {candidate:.4}"));
                    best = trial;
                    config = changed;
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
    if let Ok(path) = std::env::var("BTT_TUNED_OUT") {
        std::fs::write(path, &config).expect("the tuned configuration writes");
    }
    println!("\nfinal values");
    for path in TUNED {
        println!("  {path} = {:.4}", value_of(&config, path));
    }
    // The width of the range: the interval share that holds 80 percent of the test prices.
    println!("\nrange width");
    for share in [0.6, 0.65, 0.7, 0.75, 0.8] {
        let trial = score_both(
            &configured(&config, &[("interval_share", share)]),
            observations,
        );
        print(&trial, &format!("interval_share = {share}"));
    }
}

/// The configurations the variant mode compares, each as changes to the shipped one. Each
/// leaves out one group of terms, and the last is the reference model of `docs/price-model.md`,
/// with hand-set spreads and without the terms of countries and borders.
fn variants() -> Vec<(&'static str, Vec<(&'static str, f64)>)> {
    vec![
        ("shipped", vec![]),
        (
            "without base fares",
            vec![("spread.base", 0.0), ("spread.country_base", 0.0)],
        ),
        ("without country curve", vec![("spread.country_curve", 0.0)]),
        (
            "without country category",
            vec![("spread.country_category", 0.0)],
        ),
        (
            "with a fixed border surcharge",
            vec![
                ("spread.border", 0.2),
                ("spread.border_base", 0.0),
                ("spread.border_operator", 0.0),
            ],
        ),
        (
            "without kind and operator noise",
            vec![("noise.kind_weight", 0.0), ("noise.operator_weight", 0.0)],
        ),
        (
            "reference",
            vec![
                ("outlier_limit", 3.0),
                ("default_shift_kilometres", 10.0),
                ("noise.kind_weight", 0.0),
                ("noise.operator_weight", 0.0),
                ("global.intercept_standard_deviation", 0.2),
                ("global.slope_standard_deviation", 0.05),
                ("spread.country_slope", 0.1),
                ("spread.country_base", 0.0),
                ("spread.country_curve", 0.0),
                ("spread.operator", 0.15),
                ("spread.country_kind", 0.1),
                ("spread.category", 0.1),
                ("spread.country_category", 0.0),
                ("spread.base", 0.0),
                ("spread.border", 0.2),
                ("spread.border_base", 0.0),
                ("spread.border_operator", 0.0),
            ],
        ),
    ]
}

/// Prints the header of a table of scores.
fn print_header() {
    println!(
        "{:<44} {:>5} {:>8} {:>7} {:>7} {:>7}",
        "configuration", "count", "median", "mean", "inside", "density"
    );
}

fn main() {
    let path = std::env::args().nth(1).expect("pass a file of live prices");
    let mode = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "variants".to_owned());
    let text = std::fs::read_to_string(&path).expect("the file reads");
    let prior = PriceModel::prior().expect("the model files load");
    let observations: Vec<Labelled> = seed_observations(&text)
        .expect("the file parses")
        .into_iter()
        .filter(|observation| observation.price.euro() >= 0.5)
        .filter_map(|observation| {
            let rides: Vec<RideFacts<'_>> = observation
                .run
                .rides
                .iter()
                .map(RideFacts::of_description)
                .collect();
            let estimate = prior.estimate(&rides)?;
            let operator = rides
                .iter()
                .max_by(|a, b| a.distance_kilometres.total_cmp(&b.distance_kilometres))?
                .operator
                .to_owned();
            let groups = [
                format!("country {}", estimate.country.as_deref().unwrap_or("??")),
                format!("operator {operator}"),
                format!("kind {:?}", estimate.kind),
            ];
            Some(Labelled {
                observation,
                groups,
            })
        })
        .collect();
    println!(
        "{} prices in {FOLDS} folds by route, {SPLITS} splits",
        observations.len()
    );
    print_header();
    match mode.as_str() {
        "tune" | "tune-density" => {
            // The search starts from the file `BTT_TUNE_FROM` names, or else the shipped
            // configuration, and writes its result to `BTT_TUNED_OUT`.
            let start = std::env::var("BTT_TUNE_FROM").map_or_else(
                |_| SHIPPED_CONFIG.to_owned(),
                |path| std::fs::read_to_string(path).expect("the start configuration reads"),
            );
            tune(&observations, start, mode == "tune-density");
        }
        "score" => {
            let path = std::env::args().nth(3).expect("pass a configuration");
            let config = std::fs::read_to_string(path).expect("the configuration reads");
            let (total, groups) = score(&config, &observations, folds_from_environment());
            total.print("all");
            if std::env::args().any(|argument| argument == "--groups") {
                for (name, group) in groups {
                    group.print(&name);
                }
            }
        }
        "groups" => {
            let (total, groups) = score(SHIPPED_CONFIG, &observations, folds_from_environment());
            total.print("all");
            for (name, group) in groups {
                group.print(&name);
            }
        }
        _ => {
            for (name, changes) in variants() {
                let (route, operator) =
                    score_both(&configured(SHIPPED_CONFIG, &changes), &observations);
                route.print(&format!("{name}, routes"));
                operator.print(&format!("{name}, operators"));
            }
        }
    }
}
