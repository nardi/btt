//! Compares sets of terms for the price model by cross-validation on a file of live prices.
//!
//! Each set is a Bayesian ridge regression on the log price with the same kind of one-hot terms
//! as the model, fitted in one batch. The observations are split into five folds by a hash of
//! their route, so a route is never in the training and the test part at once. The report shows
//! the median error as a factor, the mean absolute error of the log price and the share of test
//! prices inside the 80 percent range, with the shipped model as the reference.
//!
//! Run `cargo run --release -p btt-fares --example price_experiments -- <prices.jsonl>`.

use std::collections::{BTreeMap, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};

use btt_fares::{PriceModel, RideFacts, seed_observations};
use btt_model::PriceObservation;
use chrono::{Datelike, Timelike};
use nalgebra::{Cholesky, DMatrix, DVector};

/// Half the width of the 80 percent range in standard deviations.
const INTERVAL_HALF_WIDTH: f64 = 1.281_551_565_5;

/// Number of folds.
const FOLDS: u64 = 5;

/// What the experiments read from one observation.
struct Facts {
    /// The log of the price in euro.
    log_price: f64,
    /// The total distance.
    kilometres: f64,
    /// Name of the first station.
    from_station: String,
    /// Name of the last station.
    to_station: String,
    /// The grid cell of the first station, when its position is known.
    from_region: String,
    /// The grid cell of the last station, when its position is known.
    to_region: String,
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
    hour: u32,
    /// Whether the departure is on a Saturday or Sunday.
    weekend: bool,
    /// The fold the route belongs to.
    fold: u64,
}

/// A term with its value and prior standard deviation.
type Term = (String, f64, f64);

/// Builds the terms of one observation.
type TermBuilder = fn(&Facts) -> Vec<Term>;

/// The distance term with a shift.
fn distance(facts: &Facts, shift: f64) -> f64 {
    ((facts.kilometres + shift) / (100.0 + shift)).ln()
}

/// The terms of the shipped model, with a shift of `shift` kilometres.
fn base_terms(facts: &Facts, shift: f64) -> Vec<Term> {
    let u = distance(facts, shift);
    vec![
        ("intercept".into(), 1.0, 10.0),
        ("slope".into(), u, 1.0),
        (format!("country {}", facts.country), 1.0, 0.35),
        (format!("country slope {}", facts.country), u, 0.1),
        (format!("operator {}", facts.operator), 1.0, 0.15),
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
    ]
}

/// The days ahead as a term per operator.
fn days_terms(facts: &Facts) -> Vec<Term> {
    let days = (1.0 + facts.days_ahead).ln();
    vec![
        ("days".into(), days, 0.3),
        (format!("days {}", facts.operator), days, 0.1),
    ]
}

/// The time of day and the weekend as terms.
fn time_terms(facts: &Facts) -> Vec<Term> {
    let band = match facts.hour {
        0..=5 => "night",
        6..=8 => "morning peak",
        9..=15 => "midday",
        16..=18 => "evening peak",
        _ => "evening",
    };
    vec![
        (format!("hour {band}"), 1.0, 0.2),
        ("weekend".into(), f64::from(u8::from(facts.weekend)), 0.2),
    ]
}

/// A curved distance term and a slope per operator.
fn shape_terms(facts: &Facts) -> Vec<Term> {
    let u = distance(facts, 10.0);
    vec![
        ("curve".into(), u * u, 0.1),
        (format!("curve {}", facts.country), u * u, 0.05),
        (format!("operator slope {}", facts.operator), u, 0.1),
    ]
}

/// An offset for each end station, which catches part of the zone fares of local networks.
fn station_terms(facts: &Facts) -> Vec<Term> {
    vec![
        (format!("station {}", facts.from_station), 1.0, 0.2),
        (format!("station {}", facts.to_station), 1.0, 0.2),
    ]
}

/// A term that rises below 15 km, for the price floor of short trips.
fn short_terms(facts: &Facts) -> Vec<Term> {
    let short = (15.0 / facts.kilometres.max(0.5)).ln().max(0.0);
    vec![
        ("short".into(), short, 0.3),
        (format!("short {}", facts.country), short, 0.2),
    ]
}

/// Hinges in the distance curve at 5, 15 and 40 km, so the slope can change along the way.
fn hinge_terms(facts: &Facts) -> Vec<Term> {
    let u = distance(facts, 10.0);
    let mut terms = Vec::new();
    for knot in [5.0, 15.0, 40.0] {
        let hinge = (u - ((knot + 10.0) / 110.0_f64).ln()).max(0.0);
        terms.push((format!("hinge {knot}"), hinge, 0.3));
        terms.push((format!("hinge {knot} {}", facts.country), hinge, 0.3));
    }
    terms
}

/// An offset for the region of each end station, a grid cell of about 25 km, which stands in for
/// the zone network of local fares.
fn region_terms(facts: &Facts) -> Vec<Term> {
    vec![
        (format!("region {}", facts.from_region), 1.0, 0.2),
        (format!("region {}", facts.to_region), 1.0, 0.2),
    ]
}

/// Returns the grid cell of a station, from a table of names and positions.
fn region_of(positions: &HashMap<String, (f64, f64)>, station: &str) -> String {
    positions.get(station).map_or_else(
        || format!("unknown {station}"),
        |&(latitude, longitude)| {
            format!(
                "{:.0},{:.0}",
                (latitude / 0.25).floor(),
                (longitude / 0.35).floor()
            )
        },
    )
}

/// Reads station positions from the file `BTT_STATION_POSITIONS` names, one station per line
/// as a name, a tab and `latitude, longitude`.
fn station_positions() -> HashMap<String, (f64, f64)> {
    let Ok(path) = std::env::var("BTT_STATION_POSITIONS") else {
        return HashMap::new();
    };
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut positions = HashMap::new();
    for line in text.lines() {
        let Some((name, position)) = line.split_once('\t') else {
            continue;
        };
        let Some((latitude, longitude)) = position.split_once(", ") else {
            continue;
        };
        if let (Ok(latitude), Ok(longitude)) = (latitude.parse(), longitude.parse()) {
            positions
                .entry(name.to_owned())
                .or_insert((latitude, longitude));
        }
    }
    positions
}

/// The shipped terms with an operator slope, which the model has.
fn model_terms(facts: &Facts) -> Vec<Term> {
    let mut terms = base_terms(facts, 10.0);
    terms.extend(shape_terms(facts).into_iter().skip(2));
    terms
}

/// Terms per operator for the base fare and the taper of its tariff.
fn operator_tariff_terms(facts: &Facts, short: bool, base: bool, curve: bool) -> Vec<Term> {
    let mut terms = Vec::new();
    if short {
        let value = (15.0 / facts.kilometres.max(0.5)).ln().max(0.0);
        terms.push((format!("operator short {}", facts.operator), value, 0.3));
    }
    if base {
        // The log price of `base + rate × km` exceeds that of `rate × km` by about
        // `base / (rate × km)`, so a term in `1 / km` stands in for a base fare.
        let value = 10.0 / (facts.kilometres + 5.0);
        terms.push((format!("operator base {}", facts.operator), value, 0.3));
    }
    if curve {
        let u = distance(facts, 10.0);
        terms.push((format!("operator curve {}", facts.operator), u * u, 0.1));
    }
    terms
}

/// The feature sets the experiment compares.
fn experiments() -> Vec<(&'static str, TermBuilder)> {
    vec![
        ("shift 0 km", |facts| base_terms(facts, 0.0)),
        ("shift 5 km", |facts| base_terms(facts, 5.0)),
        ("shift 10 km (shipped)", |facts| base_terms(facts, 10.0)),
        ("shift 20 km", |facts| base_terms(facts, 20.0)),
        ("shift 40 km", |facts| base_terms(facts, 40.0)),
        ("shift 10 km + days", |facts| {
            let mut terms = base_terms(facts, 10.0);
            terms.extend(days_terms(facts));
            terms
        }),
        ("shift 10 km + time", |facts| {
            let mut terms = base_terms(facts, 10.0);
            terms.extend(time_terms(facts));
            terms
        }),
        ("shift 10 km + days + time", |facts| {
            let mut terms = base_terms(facts, 10.0);
            terms.extend(days_terms(facts));
            terms.extend(time_terms(facts));
            terms
        }),
        ("shift 10 km + shape", |facts| {
            let mut terms = base_terms(facts, 10.0);
            terms.extend(shape_terms(facts));
            terms
        }),
        ("shift 10 km + curve", |facts| {
            let mut terms = base_terms(facts, 10.0);
            terms.extend(shape_terms(facts).into_iter().take(2));
            terms
        }),
        ("shift 10 km + operator slope", |facts| {
            let mut terms = base_terms(facts, 10.0);
            terms.extend(shape_terms(facts).into_iter().skip(2));
            terms
        }),
        ("model", model_terms),
        ("model + operator short", |facts| {
            let mut terms = model_terms(facts);
            terms.extend(operator_tariff_terms(facts, true, false, false));
            terms
        }),
        ("model + operator base", |facts| {
            let mut terms = model_terms(facts);
            terms.extend(operator_tariff_terms(facts, false, true, false));
            terms
        }),
        ("model + operator curve", |facts| {
            let mut terms = model_terms(facts);
            terms.extend(operator_tariff_terms(facts, false, false, true));
            terms
        }),
        ("model + base + curve", |facts| {
            let mut terms = model_terms(facts);
            terms.extend(operator_tariff_terms(facts, false, true, true));
            terms
        }),
        ("shift 10 km + stations", |facts| {
            let mut terms = base_terms(facts, 10.0);
            terms.extend(station_terms(facts));
            terms
        }),
        ("shift 10 km + short", |facts| {
            let mut terms = base_terms(facts, 10.0);
            terms.extend(short_terms(facts));
            terms
        }),
        ("shift 10 km + hinges", |facts| {
            let mut terms = base_terms(facts, 10.0);
            terms.extend(hinge_terms(facts));
            terms
        }),
        ("shift 10 km + hinges + time", |facts| {
            let mut terms = base_terms(facts, 10.0);
            terms.extend(hinge_terms(facts));
            terms.extend(time_terms(facts));
            terms
        }),
        ("shift 10 km + regions", |facts| {
            let mut terms = base_terms(facts, 10.0);
            terms.extend(region_terms(facts));
            terms
        }),
        ("shift 10 km + regions + hinges", |facts| {
            let mut terms = base_terms(facts, 10.0);
            terms.extend(region_terms(facts));
            terms.extend(hinge_terms(facts));
            terms
        }),
        ("shift 10 km + all", |facts| {
            let mut terms = base_terms(facts, 10.0);
            terms.extend(days_terms(facts));
            terms.extend(time_terms(facts));
            terms.extend(shape_terms(facts));
            terms
        }),
    ]
}

/// Errors and hits of the predictions for the test parts.
#[derive(Default)]
struct Score {
    /// Absolute errors of the log price.
    errors: Vec<f64>,
    /// How many test prices fell inside the range.
    inside: usize,
}

impl Score {
    /// Adds a prediction.
    fn add(&mut self, log_price: f64, mean: f64, standard_deviation: f64) {
        let error = (log_price - mean).abs();
        self.errors.push(error);
        self.inside += usize::from(error <= INTERVAL_HALF_WIDTH * standard_deviation);
    }

    /// Prints the score as one row.
    fn print(&self, name: &str) {
        let mut errors = self.errors.clone();
        errors.sort_by(f64::total_cmp);
        // Counts are small enough for `f64`.
        #[expect(clippy::cast_precision_loss)]
        let count = errors.len().max(1) as f64;
        let median = errors.get(errors.len() / 2).copied().unwrap_or(0.0);
        let mean = errors.iter().sum::<f64>() / count;
        #[expect(clippy::cast_precision_loss)]
        let inside = self.inside as f64 / count;
        println!(
            "{name:<28} {:>7.3}x {:>8.3} {:>7.0}%",
            median.exp(),
            mean,
            inside * 100.0
        );
    }
}

/// A Bayesian ridge regression fitted in one batch.
struct BatchFit {
    /// The column of each term.
    index: HashMap<String, usize>,
    /// Posterior mean of the weights.
    mean: DVector<f64>,
    /// Posterior covariance of the weights.
    covariance: DMatrix<f64>,
    /// Variance of the residuals.
    noise_variance: f64,
    /// Residual of each training price.
    residuals: Vec<f64>,
}

impl BatchFit {
    /// Fits the terms of `build` to the prices of `train`, in two passes: the second uses the
    /// noise that the first leaves.
    fn new(train: &[&Facts], build: TermBuilder) -> Self {
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut prior_precision = Vec::new();
        let rows: Vec<Vec<(usize, f64)>> = train
            .iter()
            .map(|facts| {
                build(facts)
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
            residuals: Vec::new(),
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
            fit.residuals = rows
                .iter()
                .zip(train)
                .map(|(row, facts)| {
                    facts.log_price - row.iter().map(|&(i, a)| a * fit.mean[i]).sum::<f64>()
                })
                .collect();
            // Counts are small enough for `f64`.
            #[expect(clippy::cast_precision_loss)]
            let count = train.len().max(1) as f64;
            let squared: f64 = fit
                .residuals
                .iter()
                .map(|residual| residual * residual)
                .sum();
            fit.noise_variance = (squared / count).max(0.0025);
        }
        fit
    }

    /// Returns the mean and standard deviation of the log price of a test price. Terms that no
    /// training price had are at their prior, with mean 0 and their own variance.
    fn predict(&self, facts: &Facts, build: TermBuilder) -> (f64, f64) {
        let mut predicted = 0.0;
        let mut variance = self.noise_variance;
        let mut known = Vec::new();
        for (name, value, standard_deviation) in build(facts) {
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

/// Fits each fold's training part and scores its test part.
fn cross_validated_score(facts: &[Facts], build: TermBuilder) -> Score {
    let mut score = Score::default();
    for fold in 0..FOLDS {
        let (train, test): (Vec<&Facts>, Vec<&Facts>) =
            facts.iter().partition(|facts| facts.fold != fold);
        let fit = BatchFit::new(&train, build);
        for facts in test {
            let (mean, standard_deviation) = fit.predict(facts, build);
            score.add(facts.log_price, mean, standard_deviation);
        }
    }
    score
}

/// Scores the shipped model, learning online from the training part of each fold.
fn shipped_model_score(observations: &[(PriceObservation, u64)]) -> Score {
    let mut score = Score::default();
    for fold in 0..FOLDS {
        let mut model = PriceModel::prior().expect("the model files load");
        for (observation, _) in observations.iter().filter(|(_, other)| *other != fold) {
            model.observe(observation);
        }
        for (observation, _) in observations.iter().filter(|(_, other)| *other == fold) {
            let rides: Vec<RideFacts<'_>> = observation
                .run
                .rides
                .iter()
                .map(RideFacts::of_description)
                .collect();
            if let Some(estimate) = model.estimate(&rides) {
                let mean = estimate.median.ln();
                let standard_deviation =
                    (estimate.high.ln() - estimate.low.ln()) / (2.0 * INTERVAL_HALF_WIDTH);
                score.add(observation.price.euro().ln(), mean, standard_deviation);
            }
        }
    }
    score
}

/// Fits the shipped terms with an operator slope on every price and prints the 25 prices
/// furthest from the fit, to show what the terms miss.
fn print_worst_prices(facts: &[Facts]) {
    let everything: Vec<&Facts> = facts.iter().collect();
    let build: TermBuilder = |facts| {
        let mut terms = base_terms(facts, 10.0);
        terms.extend(shape_terms(facts).into_iter().skip(2));
        terms
    };
    let fit = BatchFit::new(&everything, build);
    let mut ranked: Vec<(&Facts, f64)> = everything.into_iter().zip(fit.residuals).collect();
    ranked.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()));
    for (facts, residual) in ranked.into_iter().take(25) {
        let price = facts.log_price.exp();
        println!(
            "{residual:+.2}  {price:>7.2} EUR, fit {:>7.2}  {:>6.1} km  {:<16} {:<20} {} -> {}",
            (facts.log_price - residual).exp(),
            facts.kilometres,
            facts.operator,
            facts.category,
            facts.from_station,
            facts.to_station,
        );
    }
}

/// Groups prices by one of their facts, for the residual report.
type Grouping = (&'static str, fn(&Facts) -> String);

/// Fits the shipped terms on every price and prints the mean and spread of the residuals by
/// distance, days ahead, hour, category and operator, to show what the terms miss.
fn print_residuals(facts: &[Facts]) {
    let everything: Vec<&Facts> = facts.iter().collect();
    let fit = BatchFit::new(&everything, |facts| base_terms(facts, 10.0));
    let groupings: Vec<Grouping> = vec![
        ("distance", |facts| {
            match facts.kilometres {
                k if k < 5.0 => "0-5 km",
                k if k < 15.0 => "5-15 km",
                k if k < 40.0 => "15-40 km",
                k if k < 100.0 => "40-100 km",
                _ => "100+ km",
            }
            .to_owned()
        }),
        ("days ahead", |facts| {
            match facts.days_ahead {
                d if d < 3.0 => "0-2",
                d if d < 10.0 => "3-9",
                d if d < 25.0 => "10-24",
                _ => "25+",
            }
            .to_owned()
        }),
        ("hour", |facts| {
            format!("{:02}-{:02}", facts.hour / 3 * 3, facts.hour / 3 * 3 + 3)
        }),
        ("category", |facts| facts.category.clone()),
        ("operator", |facts| facts.operator.clone()),
    ];
    for (title, group_of) in groupings {
        println!("\nby {title}");
        let mut groups: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for (facts, residual) in everything.iter().zip(&fit.residuals) {
            groups.entry(group_of(facts)).or_default().push(*residual);
        }
        for (group, values) in groups {
            // Counts are small enough for `f64`.
            #[expect(clippy::cast_precision_loss)]
            let count = values.len() as f64;
            let mean = values.iter().sum::<f64>() / count;
            let spread = (values
                .iter()
                .map(|value| (value - mean).powi(2))
                .sum::<f64>()
                / count)
                .sqrt();
            println!(
                "  {group:<14} {:>4}  mean {mean:+.2}  spread {spread:.2}",
                values.len()
            );
        }
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("pass a file of live prices");
    let text = std::fs::read_to_string(&path).expect("the file reads");
    let observations: Vec<PriceObservation> = seed_observations(&text)
        .expect("the file parses")
        .into_iter()
        .filter(|observation| observation.price.euro() >= 0.5)
        .collect();
    let model = PriceModel::prior().expect("the model files load");
    let positions = station_positions();
    let mut facts = Vec::new();
    let mut folded = Vec::new();
    for observation in &observations {
        let run = &observation.run;
        let mut hasher = DefaultHasher::new();
        (&run.from_station, &run.to_station).hash(&mut hasher);
        let fold = hasher.finish() % FOLDS;
        let rides: Vec<RideFacts<'_>> = run.rides.iter().map(RideFacts::of_description).collect();
        let Some(estimate) = model.estimate(&rides) else {
            continue;
        };
        let longest = rides
            .iter()
            .max_by(|a, b| a.distance_kilometres.total_cmp(&b.distance_kilometres))
            .expect("a run has rides");
        // Day counts are small enough for `f64`.
        #[expect(clippy::cast_precision_loss)]
        let days_ahead = (run.local_departure.date() - observation.looked_up_on)
            .num_days()
            .max(0) as f64;
        facts.push(Facts {
            log_price: observation.price.euro().ln(),
            kilometres: rides.iter().map(|ride| ride.distance_kilometres).sum(),
            country: estimate.country.as_deref().unwrap_or("??").to_owned(),
            kind: format!("{:?}", estimate.kind),
            from_station: run.from_station.to_string(),
            to_station: run.to_station.to_string(),
            from_region: region_of(&positions, &run.from_station),
            to_region: region_of(&positions, &run.to_station),
            operator: longest.operator.to_owned(),
            category: longest.category.to_uppercase(),
            days_ahead,
            hour: run.local_departure.hour(),
            weekend: run.local_departure.weekday().number_from_monday() >= 6,
            fold,
        });
        folded.push((observation.clone(), fold));
    }
    println!("{} prices in {FOLDS} folds by route", facts.len());
    println!(
        "{:<28} {:>8} {:>8} {:>8}",
        "terms", "median", "mean", "inside"
    );
    if std::env::args().any(|argument| argument == "--worst") {
        print_worst_prices(&facts);
        return;
    }
    if std::env::args().any(|argument| argument == "--residuals") {
        print_residuals(&facts);
        return;
    }
    shipped_model_score(&folded).print("shipped model, online");
    for (name, build) in experiments() {
        cross_validated_score(&facts, build).print(name);
    }
}
