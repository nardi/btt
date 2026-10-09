//! Checks the night train fit against trains it was not fitted to.
//!
//! Every train of `data/fares/night_fares.jsonl` is predicted from the trains of the other folds,
//! where a fold holds all the days and both directions of a route. The report shows the median
//! error of the prediction as a factor, the same for a price that ignores distance and the kind
//! of train, and the share of prices inside the range of their estimate.
//!
//! Run `cargo run -p btt-fares --example night_fares_report -- [night_fares.jsonl]`.

use btt_fares::{Accommodation, NightFares, PriceBasis, PriceModel, RideFacts};
use serde_json::Value;

/// Returns the country of a station from the first two digits of its number, which are those of
/// its UIC number, or 12 for the Vienna Hbf of the shop.
fn country_of(number: u64) -> Option<&'static str> {
    Some(match number / 100_000 {
        80 => "DE",
        81 | 12 => "AT",
        85 => "CH",
        84 => "NL",
        83 => "IT",
        88 => "BE",
        54 => "CZ",
        55 => "HU",
        51 => "PL",
        78 => "HR",
        _ => return None,
    })
}

/// Returns the median error as a factor of the general price model on the cheapest seat of the
/// trains, which is what the model estimated for a night train before the fit to the offers.
fn general_model_error(text: &str) -> Option<f64> {
    let model = PriceModel::bundled().ok()?;
    let mut errors: Vec<f64> = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let train: Value = serde_json::from_str(line).ok()?;
        let seat = train["compartments"]
            .as_array()?
            .iter()
            .filter(|compartment| compartment["type"] == "SEAT")
            .flat_map(|compartment| compartment["prices"].as_object().into_iter().flatten())
            .filter_map(|(_, price)| price.as_f64())
            .reduce(f64::min);
        let Some(seat) = seat else { continue };
        let kilometres = train["straight_kilometres"].as_f64()? * 1.3;
        let category = if train["train"].as_str()?.starts_with("EN") {
            "EN"
        } else {
            "NJ"
        };
        let ride = RideFacts {
            from_country: country_of(train["from"]["number"].as_u64()?),
            to_country: country_of(train["to"]["number"].as_u64()?),
            operator: "oebb",
            category,
            distance_kilometres: kilometres,
            offers_berths: true,
        };
        let estimate = model.estimate(&[ride])?;
        errors.push((seat.ln() - estimate.median.ln()).abs());
    }
    errors.sort_by(f64::total_cmp);
    errors.get(errors.len() / 2).map(|error| error.exp())
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "data/fares/night_fares.jsonl".to_owned());
    let text = std::fs::read_to_string(path).expect("the file reads");
    if let Some(error) = general_model_error(&text) {
        println!("general price model on the cheapest seat: {error:.2}x\n");
    }
    let scores = NightFares::cross_validate(&text, 5).expect("the file parses");
    println!(
        "{:<10} {:<14} {:>6} {:>9} {:>9} {:>8}",
        "kind", "price", "trains", "error", "constant", "inside"
    );
    for score in scores {
        let kind = match score.accommodation {
            Accommodation::Seat => "seat",
            Accommodation::Couchette => "couchette",
            Accommodation::Berth => "berth",
        };
        let basis = match score.basis {
            PriceBasis::WithoutPass => "without pass",
            PriceBasis::WithPass => "with pass",
            PriceBasis::ExtraOverSeat => "over the seat",
        };
        println!(
            "{kind:<10} {basis:<14} {:>6} {:>8.3}x {:>8.3}x {:>7.0}%",
            score.count,
            score.median_error,
            score.baseline_median_error,
            score.inside_share * 100.0
        );
    }
}
