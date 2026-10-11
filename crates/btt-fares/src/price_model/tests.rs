use btt_model::{RideDescription, RunDescription};
use chrono::NaiveDateTime;

use super::*;

/// The model at its prior, without the seed observations.
fn prior_model() -> PriceModel {
    PriceModel::from_files(crate::PRICE_MODEL_TOML, crate::OPERATORS_TOML).unwrap()
}

/// Returns a day of 2026 by its number in the year.
fn day(ordinal: u32) -> NaiveDate {
    NaiveDate::from_yo_opt(2026, ordinal).unwrap()
}

/// A ride inside one country, or across a border when the countries differ.
fn ride<'a>(
    operator: &'a str,
    category: &'a str,
    from: &'a str,
    to: &'a str,
    kilometres: f64,
) -> RideFacts<'a> {
    RideFacts {
        from_country: Some(from),
        to_country: Some(to),
        operator,
        category,
        distance_kilometres: kilometres,
        offers_berths: false,
    }
}

/// A live price of a single ride.
fn observation(ride: &RideFacts<'_>, euro: f64, looked_up_on: NaiveDate) -> PriceObservation {
    // Test distances are whole kilometres well inside `u32` metres.
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let distance_metres = (ride.distance_kilometres * 1000.0) as u32;
    PriceObservation {
        run: RunDescription {
            from_station: "A".into(),
            to_station: "B".into(),
            local_departure: NaiveDateTime::new(looked_up_on, chrono::NaiveTime::MIN),
            rides: vec![RideDescription {
                operator: ride.operator.into(),
                category: ride.category.into(),
                from_country: ride.from_country.map(Into::into),
                to_country: ride.to_country.map(Into::into),
                distance_metres,
                offers_berths: ride.offers_berths,
            }],
        },
        price: EuroCents::from_euro(euro),
        original: None,
        looked_up_on,
        source: "Test".into(),
    }
}

/// Returns the median estimate of a single ride.
fn median(model: &PriceModel, ride: &RideFacts<'_>) -> f64 {
    model.estimate(&[*ride]).unwrap().median
}

#[test]
fn the_bundled_model_loads() {
    let model = PriceModel::bundled().unwrap();
    let price = model
        .price(&[ride("db-fernverkehr", "ICE", "DE", "DE", 300.0)])
        .unwrap();
    assert!(price.low < price.high);
    assert_eq!(price.kind, PriceKind::Range);
}

#[test]
fn the_czech_prior_reproduces_the_basic_fare() {
    let model = prior_model();
    // A long-distance kind has no offset, and a ride without an operator has no operator
    // terms, so the median is the country level.
    for (kilometres, crowns) in [(25.0, 68.0), (100.0, 221.0), (250.0, 527.0)] {
        let euro = median(&model, &ride("", "IC", "CZ", "CZ", kilometres));
        let tariff = crowns * 0.040_938;
        assert!(
            (euro / tariff - 1.0).abs() < 0.02,
            "{kilometres} km gives {euro} instead of {tariff}"
        );
    }
}

#[test]
fn an_observation_moves_the_estimate_towards_it() {
    let mut model = prior_model();
    let ice = ride("db-fernverkehr", "ICE", "DE", "DE", 400.0);
    let before = median(&model, &ice);
    let observed = before * 1.5;
    model.observe(&observation(&ice, observed, day(10)));
    let after = median(&model, &ice);
    assert!(after > before && after < observed, "{before} -> {after}");
    for _ in 0..30 {
        model.observe(&observation(&ice, observed, day(10)));
    }
    assert!((median(&model, &ice) / observed - 1.0).abs() < 0.05);
    assert_eq!(model.observation_count(), 31);
}

#[test]
fn an_operator_without_observations_takes_the_level_of_its_country() {
    let mut model = prior_model();
    let db = ride("db-fernverkehr", "IC", "DE", "DE", 300.0);
    let other = ride("some-new-operator", "IC", "DE", "DE", 300.0);
    let before = median(&model, &other);
    for _ in 0..20 {
        model.observe(&observation(&db, median(&model, &db) * 1.6, day(5)));
    }
    let after = median(&model, &other);
    assert!(after > before * 1.3, "{before} -> {after}");
    let estimate = model.estimate(&[other]).unwrap();
    assert_eq!(estimate.operator_observations, 0);
    assert_eq!(estimate.country_observations, 20);
}

#[test]
fn the_shape_carries_over_to_a_country_with_one_kind_observed() {
    let mut model = prior_model();
    // In Germany, high-speed trains cost twice as much as regional ones.
    for kilometres in [100.0, 200.0, 400.0] {
        let fast = ride("db-fernverkehr", "ICE", "DE", "DE", kilometres);
        let slow = ride("db-regio", "RE", "DE", "DE", kilometres);
        for _ in 0..10 {
            model.observe(&observation(&fast, 0.24 * kilometres, day(5)));
            model.observe(&observation(&slow, 0.12 * kilometres, day(5)));
        }
    }
    // Austria has only regional prices, at half the German level.
    for kilometres in [100.0, 200.0] {
        let slow = ride("oebb", "REX", "AT", "AT", kilometres);
        for _ in 0..10 {
            model.observe(&observation(&slow, 0.06 * kilometres, day(5)));
        }
    }
    // Austrian high-speed trains have no observations of their own.
    let fast = median(&model, &ride("oebb", "ICE", "AT", "AT", 200.0));
    let slow = median(&model, &ride("oebb", "REX", "AT", "AT", 200.0));
    let ratio = fast / slow;
    assert!(
        ratio > 1.5,
        "the ratio of high speed to regional is {ratio}"
    );
}

#[test]
fn many_observations_on_one_day_do_not_age_the_others() {
    let mut model = prior_model();
    let rare = ride("pkp-intercity", "TLK", "PL", "PL", 300.0);
    model.observe(&observation(&rare, median(&model, &rare) * 2.0, day(20)));
    let learned = median(&model, &rare);
    // Prices of another operator that match the model exactly change no mean, and on the same
    // day they make nothing older.
    let busy = ride("sbb", "IC", "CH", "CH", 150.0);
    for _ in 0..50 {
        let at_median = median(&model, &busy);
        model.observe(&observation(&busy, at_median, day(20)));
    }
    let after = median(&model, &rare);
    assert!((after / learned - 1.0).abs() < 0.01, "{learned} -> {after}");
}

#[test]
fn forgetting_follows_the_days_that_pass() {
    let mut model = prior_model();
    let rare = ride("pkp-intercity", "TLK", "PL", "PL", 300.0);
    let prior = median(&model, &rare);
    for _ in 0..10 {
        model.observe(&observation(&rare, prior * 2.0, day(1)));
    }
    let deviation = |model: &PriceModel| (median(model, &rare) / prior).ln();
    let fresh = deviation(&model);
    let mut half_life_later = model.clone();
    half_life_later.advance_to(day(1 + 182));
    let mut much_later = model.clone();
    much_later.advance_to(NaiveDate::from_ymd_opt(2036, 1, 1).unwrap());
    assert!(fresh > deviation(&half_life_later));
    assert!(deviation(&half_life_later) > 0.2);
    assert!(deviation(&much_later).abs() < 0.01);
}

#[test]
fn replaying_the_same_observations_gives_the_same_model() {
    let rides = [
        ride("sbb", "IC", "CH", "CH", 120.0),
        ride("trenitalia", "FR", "IT", "IT", 500.0),
        ride("oebb", "RJX", "AT", "DE", 300.0),
    ];
    let replay = || {
        let mut model = prior_model();
        for (index, ride) in rides.iter().enumerate() {
            let offset = u32::try_from(index).unwrap();
            model.observe(&observation(
                ride,
                40.0 + 10.0 * f64::from(offset),
                day(3 + offset),
            ));
        }
        model
    };
    let (first, second) = (replay(), replay());
    for ride in &rides {
        assert_eq!(first.estimate(&[*ride]), second.estimate(&[*ride]));
    }
}

#[test]
fn an_outlier_moves_the_estimate_less_than_its_size() {
    let mut model = prior_model();
    let ice = ride("db-fernverkehr", "ICE", "DE", "DE", 400.0);
    let before = median(&model, &ice);
    model.observe(&observation(&ice, before * 1000.0, day(4)));
    let after = median(&model, &ice);
    assert!(after < before * 5.0, "{before} -> {after}");
}

#[test]
fn free_prices_and_empty_runs_teach_nothing() {
    let mut model = prior_model();
    let ride = ride("cfl", "RE", "LU", "LU", 40.0);
    assert!(!model.observe(&observation(&ride, 0.0, day(4))));
    assert!(model.estimate(&[]).is_none());
    assert_eq!(model.observation_count(), 0);
}

#[test]
fn the_kind_of_a_train_follows_its_category_operator_and_berths() {
    let model = prior_model();
    let kind = |operator: &str, category: &str, berths: bool| {
        model.config.kind_of(operator, category, berths)
    };
    assert_eq!(kind("db-fernverkehr", "ICE", false), TrainKind::HighSpeed);
    assert_eq!(kind("sncf", "Intercités", false), TrainKind::LongDistance);
    assert_eq!(kind("regiojet", "RJ", false), TrainKind::LongDistance);
    assert_eq!(
        kind("ouigo", "OUIGO Train Classique", false),
        TrainKind::HighSpeed
    );
    assert_eq!(kind("oebb", "RJX", true), TrainKind::Night);
    assert_eq!(kind("sbb", "S", false), TrainKind::Regional);
}

#[test]
fn a_run_takes_its_fastest_kind_and_notices_a_border() {
    let model = prior_model();
    let rides = [
        ride("db-regio", "RE", "DE", "DE", 50.0),
        ride("sncf", "TGV INOUI", "DE", "FR", 450.0),
    ];
    let summary = model.run_summary(&rides).unwrap();
    assert_eq!(summary.kind, TrainKind::HighSpeed);
    assert_eq!(summary.operator, "sncf");
    assert_eq!(summary.country.as_deref(), Some("FR"));
    assert!(summary.crosses_border);
    assert!((summary.kilometres - 500.0).abs() < 1e-9);
}

#[test]
fn a_run_inside_one_country_takes_its_level_whoever_runs_it() {
    let model = prior_model();
    let summary = model
        .run_summary(&[ride("db-fernverkehr", "ICE", "CH", "CH", 40.0)])
        .unwrap();
    assert_eq!(summary.country.as_deref(), Some("CH"));
    assert!(!summary.crosses_border);
}

#[test]
fn the_range_holds_the_interval_share() {
    assert!((standard_normal_quantile(0.9) - 1.281_551_565_5).abs() < 1e-6);
    assert!((standard_normal_quantile(0.5)).abs() < 1e-9);
    assert!((standard_normal_quantile(0.01) + 2.326_347_874).abs() < 1e-6);
}

#[test]
fn seed_lines_parse_as_observations() {
    let line = serde_json::to_string(&observation(
        &ride("sbb", "IC", "CH", "CH", 100.0),
        30.0,
        day(2),
    ))
    .unwrap();
    let observations = seed_observations(&format!("{line}\n\n{line}\n")).unwrap();
    assert_eq!(observations.len(), 2);
    assert!(seed_observations("not json").is_err());
}

#[test]
fn the_note_of_a_price_names_its_basis() {
    let mut model = prior_model();
    let ice = ride("db-fernverkehr", "ICE", "DE", "DE", 400.0);
    let note = model.price(&[ice]).unwrap().source.note;
    assert!(note.contains("no live prices for this country"), "{note}");
    model.observe(&observation(&ice, 50.0, day(4)));
    let note = model.price(&[ice]).unwrap().source.note;
    assert!(note.contains("1 live price of db-fernverkehr"), "{note}");
}

#[test]
fn the_table_prices_single_rides_as_the_model_estimates_them() {
    let model = PriceModel::bundled().unwrap();
    // The classes the bundle prices, with countries the model knows and one it does not.
    let classes = [
        ("db-fernverkehr", "ICE", false, Some("DE")),
        ("oebb", "RJX", false, Some("AT")),
        ("oebb", "NJ", true, Some("AT")),
        ("ns", "IC", false, Some("NL")),
        ("cfl", "RE", false, Some("LU")),
        ("renfe", "ALVIA", false, Some("ES")),
        ("snalltaget", "NJ", true, Some("SE")),
        ("db-fernverkehr", "ICE", false, None),
    ];
    let table = crate::price_table::PriceTable::from_model(
        &model,
        &classes
            .iter()
            .map(
                |&(operator, category, offers_berths, country)| crate::price_table::RouteClass {
                    operator: operator.into(),
                    category: category.into(),
                    offers_berths,
                    country: country.map(Into::into),
                },
            )
            .collect::<Vec<_>>(),
    );
    // A small deterministic generator of distances, from 5 km to 1500 km.
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut distance = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        #[expect(clippy::cast_precision_loss)]
        let share = (state >> 33) as f64 / f64::from(u32::MAX);
        5.0 + share * 1495.0
    };
    for &(operator, category, offers_berths, country) in &classes {
        let row = *table
            .row(&crate::price_table::RouteClass {
                operator: operator.into(),
                category: category.into(),
                offers_berths,
                country: country.map(Into::into),
            })
            .unwrap();
        for _ in 0..125 {
            let kilometres = distance();
            let facts = [RideFacts {
                from_country: country,
                to_country: country,
                operator,
                category,
                distance_kilometres: kilometres,
                offers_berths,
            }];
            let estimate = model.estimate(&facts).unwrap().median;
            let shift = f64::from(row.shift);
            let u = ((kilometres + shift) / (f64::from(table.reference_kilometres()) + shift)).ln();
            let base = 10.0 / (kilometres + 5.0);
            let tabled = (f64::from(row.constant)
                + f64::from(row.distance) * u
                + f64::from(row.curve) * u * u
                + f64::from(row.base) * base)
                .exp();
            let share = (tabled - estimate) / estimate;
            assert!(
                share.abs() < 0.001,
                "{operator} {category} at {kilometres:.0} km: table {tabled:.4}, estimate {estimate:.4}"
            );
        }
    }
}
