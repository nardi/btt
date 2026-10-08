//! Checks the price model against live prices it has not learned from.
//!
//! Every fifth observation of a seed file is held out. The model learns from the others and
//! predicts the held-out ones. The report shows, per country and per kind of train, the median
//! error of the prediction as a factor and how often the live price falls inside the range,
//! next to the same numbers for the model at its prior.
//!
//! Run `cargo run -p btt-fares --example price_seed_report -- [seed.jsonl]`. Without a path it
//! reads `data/fares/price_seed.jsonl`.

use std::collections::BTreeMap;

use btt_fares::{PriceModel, RideFacts, seed_observations};
use btt_model::PriceObservation;

/// How the predictions for one group of held-out prices did.
#[derive(Default)]
struct GroupScore {
    /// Absolute errors of the log price.
    errors: Vec<f64>,
    /// How many live prices fell inside the range.
    inside: usize,
}

impl GroupScore {
    /// Returns the median error as a factor, such as 1.25 for 25 percent off.
    fn median_factor(&self) -> f64 {
        let mut errors = self.errors.clone();
        errors.sort_by(f64::total_cmp);
        errors.get(errors.len() / 2).copied().unwrap_or(0.0).exp()
    }

    /// Returns the share of live prices inside the range.
    fn coverage(&self) -> f64 {
        // Counts of held-out prices are small.
        #[expect(clippy::cast_precision_loss)]
        let share = self.inside as f64 / self.errors.len().max(1) as f64;
        share
    }
}

/// Scores a model on held-out prices, grouped by country and by kind.
fn held_out_scores(
    model: &PriceModel,
    held_out: &[&PriceObservation],
) -> BTreeMap<String, GroupScore> {
    let mut groups: BTreeMap<String, GroupScore> = BTreeMap::new();
    for observation in held_out {
        let rides: Vec<RideFacts<'_>> = observation
            .run
            .rides
            .iter()
            .map(RideFacts::of_description)
            .collect();
        let Some(estimate) = model.estimate(&rides) else {
            continue;
        };
        let euro = observation.price.euro();
        if euro <= 0.0 {
            continue;
        }
        let error = (euro.ln() - estimate.median.ln()).abs();
        let inside = euro >= estimate.low && euro <= estimate.high;
        let country = estimate.country.as_deref().unwrap_or("??");
        for key in [
            "all".to_owned(),
            format!("country {country}"),
            format!("kind {:?}", estimate.kind),
        ] {
            let group = groups.entry(key).or_default();
            group.errors.push(error);
            group.inside += usize::from(inside);
        }
    }
    groups
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "data/fares/price_seed.jsonl".to_owned());
    let text = std::fs::read_to_string(&path).expect("the seed file reads");
    let observations = seed_observations(&text).expect("the seed file parses");
    let (mut learned, mut held_out) = (Vec::new(), Vec::new());
    for (index, observation) in observations.iter().enumerate() {
        if index % 5 == 4 {
            held_out.push(observation);
        } else {
            learned.push(observation);
        }
    }
    let prior = PriceModel::prior().expect("the model files load");
    let mut model = prior.clone();
    let started = std::time::Instant::now();
    for observation in &learned {
        model.observe(observation);
    }
    let learning_time = started.elapsed();
    let started = std::time::Instant::now();
    let predictions = held_out_scores(&model, &held_out);
    println!(
        "learning took {:.1} ms, and {} predictions took {:.1} ms",
        learning_time.as_secs_f64() * 1000.0,
        held_out.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    println!(
        "{} observations, {} learned, {} held out",
        observations.len(),
        learned.len(),
        held_out.len()
    );
    println!(
        "{:<24} {:>5}  {:>12} {:>9}  {:>12} {:>9}",
        "group", "count", "prior error", "inside", "model error", "inside"
    );
    let prior_scores = held_out_scores(&prior, &held_out);
    let model_scores = predictions;
    for (key, scored) in &model_scores {
        let before = &prior_scores[key];
        println!(
            "{key:<24} {:>5}  {:>11.2}x {:>8.0}%  {:>11.2}x {:>8.0}%",
            scored.errors.len(),
            before.median_factor(),
            before.coverage() * 100.0,
            scored.median_factor(),
            scored.coverage() * 100.0,
        );
    }
}
