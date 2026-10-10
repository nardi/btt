//! Compares kinds of model for the ticket price, by cross-validation on a file of live prices.
//!
//! The reference is the shipped kind of model, a Bayesian ridge regression on the log price with
//! one-hot terms, fitted in one batch. The others add a learner for what the terms miss:
//! gradient boosted trees on the residuals of the regression, a Gaussian process on those
//! residuals, and trees on their own. The residuals that the learners are trained on come from an
//! inner cross-validation, so they are as large as the residuals on prices the regression has not
//! seen. The observations are split into five folds by a hash of their route, so a route is never
//! in the training and the test part at once.
//!
//! Run `cargo run --release -p btt-fares --example price_model_families -- <prices.jsonl>`.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use btt_fares::{PriceModel, RideFacts, seed_observations};
use chrono::{Datelike, Timelike};
use friedrich::gaussian_process::GaussianProcess;
use gbdt::config::Config;
use gbdt::decision_tree::{Data, DataVec};
use gbdt::gradient_boost::GBDT;
use nalgebra::{Cholesky, DMatrix, DVector};

/// Number of folds.
const FOLDS: u64 = 5;

/// What the models read from one observation.
struct Facts {
    /// The log of the price in euro.
    log_price: f64,
    /// The total distance in kilometres.
    kilometres: f64,
    /// The country the model gives the run.
    country: String,
    /// The kind the model gives the run.
    kind: String,
    /// The operator of the longest ride.
    operator: String,
    /// The category of the longest ride.
    category: String,
    /// Days between the lookup and the departure.
    days_ahead: f64,
    /// Local hour of the departure.
    hour: f64,
    /// Day of the week of the departure, from 0 for Monday.
    weekday: f64,
    /// Whether the run crosses a border.
    border: bool,
    /// The fold the route belongs to.
    fold: u64,
}

/// A price with its place in the training part.
type Indexed<'a> = (usize, &'a Facts);

/// A term with its value and prior standard deviation.
type Term = (String, f64, f64);

/// The terms of the shipped model, with the base fare and the bend of each operator.
fn terms(facts: &Facts) -> Vec<Term> {
    let u = ((facts.kilometres + 10.0) / 110.0).ln();
    vec![
        ("intercept".into(), 1.0, 10.0),
        ("slope".into(), u, 1.0),
        (format!("country {}", facts.country), 1.0, 0.35),
        (format!("country slope {}", facts.country), u, 0.1),
        (format!("operator {}", facts.operator), 1.0, 0.15),
        (format!("operator slope {}", facts.operator), u, 0.1),
        (
            format!("operator base {}", facts.operator),
            10.0 / (facts.kilometres + 5.0),
            0.3,
        ),
        (format!("operator curve {}", facts.operator), u * u, 0.1),
        (format!("kind {}", facts.kind), 1.0, 0.2),
        (format!("kind slope {}", facts.kind), u, 0.05),
        (
            format!("country kind {} {}", facts.country, facts.kind),
            1.0,
            0.1,
        ),
        (
            format!("category {} {}", facts.operator, facts.category),
            1.0,
            0.1,
        ),
        ("border".into(), f64::from(u8::from(facts.border)), 0.2),
    ]
}

/// A Bayesian ridge regression fitted in one batch.
struct Regression {
    /// The column of each term.
    index: HashMap<String, usize>,
    /// Posterior mean of the weights.
    mean: DVector<f64>,
    /// Posterior covariance of the weights.
    covariance: DMatrix<f64>,
    /// Variance of the residuals.
    noise_variance: f64,
}

impl Regression {
    /// Fits the terms to the prices, in two passes: the second uses the noise the first leaves.
    fn new(train: &[&Facts]) -> Self {
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut prior_precision = Vec::new();
        let rows: Vec<Vec<(usize, f64)>> = train
            .iter()
            .map(|facts| {
                terms(facts)
                    .into_iter()
                    .map(|(name, value, standard_deviation)| {
                        let next = index.len();
                        let column = *index.entry(name).or_insert_with(|| {
                            prior_precision.push(1.0 / (standard_deviation * standard_deviation));
                            next
                        });
                        (column, value)
                    })
                    .collect()
            })
            .collect();
        let size = index.len();
        let mut fit = Self {
            index,
            mean: DVector::zeros(size),
            covariance: DMatrix::zeros(size, size),
            noise_variance: 0.09,
        };
        for _ in 0..2 {
            let mut precision = DMatrix::from_diagonal(&DVector::from_vec(prior_precision.clone()));
            let mut information = DVector::zeros(size);
            for (row, facts) in rows.iter().zip(train) {
                for &(i, a) in row {
                    information[i] += a * facts.log_price / fit.noise_variance;
                    for &(j, b) in row {
                        precision[(i, j)] += a * b / fit.noise_variance;
                    }
                }
            }
            let factor = Cholesky::new(precision).expect("the precision is positive definite");
            fit.mean = factor.solve(&information);
            fit.covariance = factor.inverse();
            let squared: f64 = rows
                .iter()
                .zip(train)
                .map(|(row, facts)| {
                    let predicted: f64 = row.iter().map(|&(i, a)| a * fit.mean[i]).sum();
                    (facts.log_price - predicted).powi(2)
                })
                .sum();
            // Counts are small enough for `f64`.
            #[expect(clippy::cast_precision_loss)]
            let count = train.len().max(1) as f64;
            fit.noise_variance = (squared / count).max(0.0025);
        }
        fit
    }

    /// Returns the mean and the standard deviation of the log price of a price.
    fn predict(&self, facts: &Facts) -> (f64, f64) {
        let mut predicted = 0.0;
        let mut variance = self.noise_variance;
        let mut known = Vec::new();
        for (name, value, standard_deviation) in terms(facts) {
            match self.index.get(&name) {
                Some(&column) => known.push((column, value)),
                None => variance += (value * standard_deviation).powi(2),
            }
        }
        for &(i, a) in &known {
            predicted += a * self.mean[i];
            for &(j, b) in &known {
                variance += a * b * self.covariance[(i, j)];
            }
        }
        (predicted, variance.sqrt())
    }
}

/// The numbers a learner sees for a price, with the categories as codes in order of their
/// first appearance.
struct Coding {
    /// The code of each category by its kind and name.
    codes: HashMap<String, f64>,
}

impl Coding {
    /// Returns an empty coding.
    fn new() -> Self {
        Self {
            codes: HashMap::new(),
        }
    }

    /// Returns the code of a category, which it assigns when the category is new.
    fn code(&mut self, kind: &str, name: &str) -> f64 {
        let next = self.codes.len();
        // Counts are small enough for `f64`.
        #[expect(clippy::cast_precision_loss)]
        let fresh = next as f64;
        *self.codes.entry(format!("{kind}:{name}")).or_insert(fresh)
    }

    /// Returns the features of a price for trees, which split on any of them.
    fn tree_features(&mut self, facts: &Facts, regression_mean: f64) -> Vec<f32> {
        let values = [
            (facts.kilometres + 10.0).ln(),
            self.code("country", &facts.country),
            self.code("operator", &facts.operator),
            self.code("kind", &facts.kind),
            self.code("category", &facts.category),
            facts.days_ahead,
            facts.hour,
            facts.weekday,
            f64::from(u8::from(facts.border)),
            regression_mean,
        ];
        #[expect(clippy::cast_possible_truncation)]
        values.iter().map(|&value| value as f32).collect()
    }

    /// Returns the features of a price for a Gaussian process: the numbers on comparable scales,
    /// and the categories as one-hot columns so that two prices of one operator are close.
    fn process_features(&mut self, facts: &Facts, names: &Names) -> Vec<f64> {
        let _ = self;
        let mut features = vec![
            (facts.kilometres + 10.0).ln() / 3.0,
            (1.0 + facts.days_ahead).ln() / 3.0,
            facts.hour / 12.0,
            facts.weekday / 3.5,
            f64::from(u8::from(facts.border)),
        ];
        for operator in &names.operators {
            features.push(f64::from(u8::from(*operator == facts.operator)));
        }
        for country in &names.countries {
            features.push(f64::from(u8::from(*country == facts.country)));
        }
        features
    }
}

/// The operators and the countries, for one-hot columns.
struct Names {
    /// Every operator.
    operators: Vec<String>,
    /// Every country.
    countries: Vec<String>,
}

/// Errors of the predictions for the test parts.
#[derive(Default)]
struct Score {
    /// Absolute errors of the log price.
    errors: Vec<f64>,
}

impl Score {
    /// Prints the score as one row.
    fn print(&self, name: &str) {
        let mut errors = self.errors.clone();
        errors.sort_by(f64::total_cmp);
        // Counts are small enough for `f64`.
        #[expect(clippy::cast_precision_loss)]
        let count = errors.len().max(1) as f64;
        let median = errors.get(errors.len() / 2).copied().unwrap_or(0.0);
        let mean = errors.iter().sum::<f64>() / count;
        let upper = errors
            .get(errors.len() * 9 / 10)
            .copied()
            .unwrap_or_default();
        println!(
            "{name:<40} {:>7.3}x {:>8.3} {:>8.3}x",
            median.exp(),
            mean,
            upper.exp()
        );
    }
}

/// Returns the residuals of the regression on prices it has not seen, from an inner
/// cross-validation of the training part.
fn honest_residuals(train: &[&Facts]) -> Vec<f64> {
    let mut residuals = vec![0.0; train.len()];
    for inner in 0..FOLDS {
        let (fit_part, held): (Vec<Indexed<'_>>, Vec<Indexed<'_>>) = train
            .iter()
            .copied()
            .enumerate()
            .partition(|(_, facts)| (facts.fold * 7 + hash_of(facts)) % FOLDS != inner);
        let fit_facts: Vec<&Facts> = fit_part.iter().map(|&(_, facts)| facts).collect();
        let regression = Regression::new(&fit_facts);
        for (position, facts) in held {
            residuals[position] = facts.log_price - regression.predict(facts).0;
        }
    }
    residuals
}

/// Returns the residuals of the regression on prices it has not seen, divided by the standard
/// deviation that it predicted for them, from an inner cross-validation of the training part.
fn honest_standardized(train: &[&Facts]) -> Vec<f64> {
    let mut standardized = vec![0.0; train.len()];
    for inner in 0..FOLDS {
        let (fit_part, held): (Vec<Indexed<'_>>, Vec<Indexed<'_>>) = train
            .iter()
            .copied()
            .enumerate()
            .partition(|(_, facts)| (facts.fold * 7 + hash_of(facts)) % FOLDS != inner);
        let fit_facts: Vec<&Facts> = fit_part.iter().map(|&(_, facts)| facts).collect();
        let regression = Regression::new(&fit_facts);
        for (position, facts) in held {
            let (mean, standard_deviation) = regression.predict(facts);
            standardized[position] = (facts.log_price - mean) / standard_deviation;
        }
    }
    standardized
}

/// Returns a number from the route of a price that is independent of its fold.
fn hash_of(facts: &Facts) -> u64 {
    let mut hasher = DefaultHasher::new();
    (facts.kilometres.to_bits(), &facts.operator, &facts.category).hash(&mut hasher);
    hasher.finish() % FOLDS
}

/// Trains boosted trees on rows of features and targets.
fn train_trees(rows: Vec<(Vec<f32>, f32)>, depth: u32, rounds: usize, shrinkage: f32) -> GBDT {
    let mut config = Config::new();
    config.set_feature_size(rows[0].0.len());
    config.set_max_depth(depth);
    config.set_iterations(rounds);
    config.set_shrinkage(shrinkage);
    config.set_loss("SquaredError");
    config.set_min_leaf_size(8);
    config.set_debug(false);
    let mut data: DataVec = rows
        .into_iter()
        .map(|(features, target)| Data::new_training_data(features, 1.0, target, None))
        .collect();
    let mut model = GBDT::new(&config);
    model.fit(&mut data);
    model
}

/// Predicts one row with boosted trees.
fn tree_predict(model: &GBDT, features: Vec<f32>) -> f64 {
    let data: DataVec = vec![Data::new_test_data(features, None)];
    f64::from(model.predict(&data)[0])
}

// The experiment reads best as one pass over the folds.
#[expect(clippy::too_many_lines)]
fn main() {
    let path = std::env::args().nth(1).expect("pass a file of live prices");
    let contents = std::fs::read_to_string(&path).expect("the file reads");
    let observations: Vec<_> = seed_observations(&contents)
        .expect("the file parses")
        .into_iter()
        .filter(|observation| observation.price.euro() >= 0.5)
        .collect();
    let model = PriceModel::prior().expect("the model files load");
    let mut facts = Vec::new();
    let mut folded = Vec::new();
    for observation in &observations {
        let run = &observation.run;
        let mut hasher = DefaultHasher::new();
        (&run.from_station, &run.to_station).hash(&mut hasher);
        let mut fold = hasher.finish() % FOLDS;
        let rides: Vec<RideFacts<'_>> = run.rides.iter().map(RideFacts::of_description).collect();
        let Some(estimate) = model.estimate(&rides) else {
            continue;
        };
        let longest = rides
            .iter()
            .max_by(|a, b| a.distance_kilometres.total_cmp(&b.distance_kilometres))
            .expect("a run has rides");
        // With `BTT_FOLD_BY_OPERATOR` set, an operator is in one fold only, so the test part holds
        // operators that the training part has no price of.
        if std::env::var_os("BTT_FOLD_BY_OPERATOR").is_some() {
            let mut hasher = DefaultHasher::new();
            longest.operator.hash(&mut hasher);
            fold = hasher.finish() % FOLDS;
        }
        let border = rides
            .first()
            .zip(rides.last())
            .is_some_and(|(first, last)| first.from_country != last.to_country);
        // Day counts are small enough for `f64`.
        #[expect(clippy::cast_precision_loss)]
        let days_ahead = (run.local_departure.date() - observation.looked_up_on)
            .num_days()
            .max(0) as f64;
        folded.push((observation.clone(), fold));
        facts.push(Facts {
            log_price: observation.price.euro().ln(),
            kilometres: rides.iter().map(|ride| ride.distance_kilometres).sum(),
            country: estimate.country.as_deref().unwrap_or("??").to_owned(),
            kind: format!("{:?}", estimate.kind),
            operator: longest.operator.to_owned(),
            category: longest.category.to_uppercase(),
            days_ahead,
            hour: f64::from(run.local_departure.hour()),
            weekday: f64::from(run.local_departure.weekday().num_days_from_monday()),
            border,
            fold,
        });
    }
    let mut names = Names {
        operators: facts.iter().map(|f| f.operator.clone()).collect(),
        countries: facts.iter().map(|f| f.country.clone()).collect(),
    };
    names.operators.sort();
    names.operators.dedup();
    names.countries.sort();
    names.countries.dedup();
    println!("{} prices in {FOLDS} folds by route\n", facts.len());
    println!(
        "{:<40} {:>8} {:>8} {:>9}",
        "model", "median", "mean", "90th"
    );

    let mut scores: Vec<(String, Score)> = Vec::new();
    let mut record = |name: &str, score: Score| {
        score.print(name);
        scores.push((name.to_owned(), score));
    };

    // The reference regression.
    let mut reference = Score::default();
    // Trees and processes on the residuals of the regression.
    let settings: [(u32, usize, f32); 4] =
        [(2, 60, 0.1), (3, 100, 0.1), (4, 150, 0.05), (6, 200, 0.05)];
    let mut boosted: Vec<Score> = settings.iter().map(|_| Score::default()).collect();
    let mut direct = Score::default();
    let mut process = Score::default();
    let mut process_trees = Score::default();
    let mut by_operator: HashMap<String, Vec<f64>> = HashMap::new();
    let mut gaussian = Interval::default();
    let mut empirical = Interval::default();
    let mut empirical_operator = Interval::default();
    let mut scaled = Interval::default();
    let mut standardized_range = Interval::default();
    for fold in 0..FOLDS {
        let (train, test): (Vec<&Facts>, Vec<&Facts>) = facts.iter().partition(|f| f.fold != fold);
        let regression = Regression::new(&train);
        let residuals = honest_residuals(&train);
        let mut coding = Coding::new();
        // The inner predictions give the trees the regression's own opinion as a feature.
        let inner_means: Vec<f64> = train
            .iter()
            .zip(&residuals)
            .map(|(facts, residual)| facts.log_price - residual)
            .collect();
        let train_rows: Vec<(Vec<f32>, f32)> = train
            .iter()
            .zip(&inner_means)
            .zip(&residuals)
            .map(|((facts, mean), residual)| {
                #[expect(clippy::cast_possible_truncation)]
                (coding.tree_features(facts, *mean), *residual as f32)
            })
            .collect();
        let boosted_models: Vec<GBDT> = settings
            .iter()
            .map(|&(depth, rounds, shrinkage)| {
                train_trees(train_rows.clone(), depth, rounds, shrinkage)
            })
            .collect();
        let direct_rows: Vec<(Vec<f32>, f32)> = train
            .iter()
            .map(|facts| {
                #[expect(clippy::cast_possible_truncation)]
                (coding.tree_features(facts, 0.0), facts.log_price as f32)
            })
            .collect();
        let direct_model = train_trees(direct_rows, 5, 200, 0.05);
        // A Gaussian process on the residuals.
        let inputs: Vec<Vec<f64>> = train
            .iter()
            .map(|facts| coding.process_features(facts, &names))
            .collect();
        let process_model = GaussianProcess::builder(inputs, residuals.clone())
            .set_noise(0.2)
            .fit_kernel()
            .fit_prior()
            .train();
        // The 10th and 90th percentile of the residuals, for all prices and for each operator.
        let all_quantiles = quantiles(&residuals);
        let standard_quantiles = quantiles(&honest_standardized(&train));
        let mut operator_residuals: HashMap<&str, Vec<f64>> = HashMap::new();
        for (facts, residual) in train.iter().zip(&residuals) {
            operator_residuals
                .entry(facts.operator.as_str())
                .or_default()
                .push(*residual);
        }
        let operator_quantiles: HashMap<&str, (f64, f64)> = operator_residuals
            .iter()
            .filter(|(_, values)| values.len() >= 15)
            .map(|(operator, values)| (*operator, quantiles(values)))
            .collect();
        for facts in &test {
            let (mean, standard_deviation) = regression.predict(facts);
            reference.errors.push((facts.log_price - mean).abs());
            by_operator
                .entry(facts.operator.clone())
                .or_default()
                .push((facts.log_price - mean).abs());
            gaussian.add(
                facts.log_price,
                mean - 1.2816 * standard_deviation,
                mean + 1.2816 * standard_deviation,
            );
            empirical.add(
                facts.log_price,
                mean + all_quantiles.0,
                mean + all_quantiles.1,
            );
            let own = operator_quantiles
                .get(facts.operator.as_str())
                .copied()
                .unwrap_or(all_quantiles);
            empirical_operator.add(facts.log_price, mean + own.0, mean + own.1);
            // The spread of an operator, shrunk to the spread of all prices by the number of its
            // prices, scales the quantiles of the residuals in units of that spread.
            standardized_range.add(
                facts.log_price,
                mean + standard_quantiles.0 * standard_deviation,
                mean + standard_quantiles.1 * standard_deviation,
            );
            let spread_of = |values: &[f64]| {
                // Counts are small enough for `f64`.
                #[expect(clippy::cast_precision_loss)]
                let count = values.len().max(1) as f64;
                (values.iter().map(|value| value * value).sum::<f64>() / count).sqrt()
            };
            let global_spread = spread_of(&residuals);
            let own_residuals = operator_residuals
                .get(facts.operator.as_str())
                .map_or(&[][..], Vec::as_slice);
            // Counts are small enough for `f64`.
            #[expect(clippy::cast_precision_loss)]
            let weight = own_residuals.len() as f64 / (own_residuals.len() as f64 + 20.0);
            let spread = weight * spread_of(own_residuals) + (1.0 - weight) * global_spread;
            let standard: Vec<f64> = residuals
                .iter()
                .map(|value| value / global_spread)
                .collect();
            let (low, high) = quantiles(&standard);
            scaled.add(facts.log_price, mean + low * spread, mean + high * spread);
            let features = coding.tree_features(facts, mean);
            for (score, model) in boosted.iter_mut().zip(&boosted_models) {
                let correction = tree_predict(model, features.clone());
                score
                    .errors
                    .push((facts.log_price - mean - correction).abs());
            }
            let predicted = tree_predict(&direct_model, coding.tree_features(facts, 0.0));
            direct.errors.push((facts.log_price - predicted).abs());
            let correction = process_model.predict(&coding.process_features(facts, &names));
            process
                .errors
                .push((facts.log_price - mean - correction).abs());
            let both = correction + tree_predict(&boosted_models[1], features);
            process_trees
                .errors
                .push((facts.log_price - mean - 0.5 * both).abs());
        }
    }
    record("regression (shipped kind of model)", reference);
    for (score, (depth, rounds, shrinkage)) in boosted.into_iter().zip(settings) {
        record(
            &format!("regression + trees d{depth} n{rounds} s{shrinkage}"),
            score,
        );
    }
    record("regression + Gaussian process", process);
    record("regression + half of process and trees", process_trees);
    record("trees alone d5 n200", direct);
    println!(
        "
80 percent ranges, from the 10th to the 90th percentile"
    );
    println!("{:<40} {:>8} {:>10}", "range", "inside", "log width");
    // The shipped model, learning online from the training part of each fold.
    let mut shipped = Interval::default();
    for fold in 0..FOLDS {
        let mut shipped_model = PriceModel::prior().expect("the model files load");
        for (observation, _) in folded.iter().filter(|(_, other)| *other != fold) {
            shipped_model.observe(observation);
        }
        for (observation, _) in folded.iter().filter(|(_, other)| *other == fold) {
            let rides: Vec<RideFacts<'_>> = observation
                .run
                .rides
                .iter()
                .map(RideFacts::of_description)
                .collect();
            if let Some(estimate) = shipped_model.estimate(&rides) {
                shipped.add(
                    observation.price.euro().ln(),
                    estimate.low.ln(),
                    estimate.high.ln(),
                );
            }
        }
    }
    shipped.print("shipped model, online range");
    gaussian.print("regression, from the posterior");
    empirical.print("regression, residual quantiles");
    empirical_operator.print("regression, residual quantiles per operator");
    scaled.print("regression, quantiles scaled by operator");
    standardized_range.print("posterior sd times residual quantiles");
    println!(
        "
mean error by operator, regression"
    );
    let mut operators: Vec<(String, Vec<f64>)> = by_operator.into_iter().collect();
    operators.sort_by(|a, b| b.1.iter().sum::<f64>().total_cmp(&a.1.iter().sum::<f64>()));
    for (operator, errors) in operators.into_iter().take(14) {
        // Counts are small enough for `f64`.
        #[expect(clippy::cast_precision_loss)]
        let count = errors.len() as f64;
        println!(
            "  {operator:<24} {:>4} prices, mean error {:.3}, share of all error {:.0}%",
            errors.len(),
            errors.iter().sum::<f64>() / count,
            100.0 * errors.iter().sum::<f64>() / scores_total(&scores)
        );
    }
}

/// Returns the sum of the errors of the reference regression.
fn scores_total(scores: &[(String, Score)]) -> f64 {
    scores
        .first()
        .map_or(1.0, |(_, score)| score.errors.iter().sum())
}

/// Returns the 10th and the 90th percentile of some numbers.
fn quantiles(values: &[f64]) -> (f64, f64) {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let at = |share: f64| {
        // The index is inside the slice.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let index = (share * (sorted.len() as f64 - 1.0)).round() as usize;
        sorted[index]
    };
    (at(0.1), at(0.9))
}

/// How many prices fall inside their range, and how wide the ranges are.
#[derive(Default)]
struct Interval {
    /// Prices inside their range.
    inside: usize,
    /// All prices.
    count: usize,
    /// The sum of the widths of the ranges on the log scale.
    width: f64,
}

impl Interval {
    /// Adds a price and its range.
    fn add(&mut self, log_price: f64, low: f64, high: f64) {
        self.count += 1;
        self.inside += usize::from(log_price >= low && log_price <= high);
        self.width += high - low;
    }

    /// Prints the coverage and the mean width as a row.
    fn print(&self, name: &str) {
        // Counts are small enough for `f64`.
        #[expect(clippy::cast_precision_loss)]
        let count = self.count.max(1) as f64;
        #[expect(clippy::cast_precision_loss)]
        let inside = self.inside as f64 / count;
        println!(
            "{name:<40} {:>7.0}% {:>10.3}",
            inside * 100.0,
            self.width / count
        );
    }
}
