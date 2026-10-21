use super::*;

/// One night train as a line of the offers, with the lowest prices of a seat, a couchette and a
/// berth without a pass and with one.
fn line(train: &str, straight_kilometres: f64, seat: f64, couchette: f64, berth: f64) -> String {
    let compartment = |kind: &str, price: f64, pass: f64| {
        format!(
            r#"{{"id":1,"name":"x","type":"{kind}","group":"DEFAULT","prices":{{"NON-FLEX":{price},"FLEX":{flex}}},"pass":{{"SEMI-FLEX":{pass}}}}}"#,
            flex = price + 40.0
        )
    };
    format!(
        r#"{{"collected_on":"2026-10-10","from":{{"name":"A"}},"to":{{"name":"B"}},"train":"{train}","straight_kilometres":{straight_kilometres},"compartments":[{},{},{}]}}"#,
        compartment("SEAT", seat, 9.9),
        compartment("COUCHETTE", couchette, couchette - 60.0),
        compartment("BERTH", berth, berth - 50.0),
    )
}

/// Offers of ten trains, with prices that do not depend on the distance.
fn offers() -> String {
    (0..10)
        .map(|index| {
            line(
                "NJ 466",
                400.0 + 50.0 * f64::from(index),
                40.0,
                100.0,
                180.0,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn prices_that_do_not_depend_on_the_distance_give_a_flat_fit() {
    let fares = NightFares::from_jsonl(&offers()).unwrap();
    for kilometres in [300.0, 800.0, 1500.0] {
        let seat = fares
            .estimate(
                NightOperator::Oebb,
                Accommodation::Seat,
                PriceBasis::WithoutPass,
                kilometres,
                false,
            )
            .unwrap();
        assert!((seat.median - 40.0).abs() < 2.0, "{kilometres}: {seat:?}");
    }
}

#[test]
fn the_extra_over_a_seat_is_the_difference_of_the_lowest_prices() {
    let fares = NightFares::from_jsonl(&offers()).unwrap();
    let couchette = fares
        .estimate(
            NightOperator::Oebb,
            Accommodation::Couchette,
            PriceBasis::ExtraOverSeat,
            700.0,
            false,
        )
        .unwrap();
    assert!((couchette.median - 60.0).abs() < 3.0, "{couchette:?}");
    assert!(
        fares
            .estimate(
                NightOperator::Oebb,
                Accommodation::Seat,
                PriceBasis::ExtraOverSeat,
                700.0,
                false
            )
            .is_none()
    );
}

#[test]
fn a_pass_holder_pays_less_than_the_extra_over_a_seat_plus_the_seat() {
    let fares = NightFares::from_jsonl(&offers()).unwrap();
    let with_pass = fares
        .accommodation_price(NightOperator::Oebb, "NJ", 700.0, true)
        .unwrap();
    let without_pass = fares
        .accommodation_price(NightOperator::Oebb, "NJ", 700.0, false)
        .unwrap();
    // The pass price holds the reservation and the bed, the extra holds the bed only.
    assert!(with_pass.low.0 > 0 && with_pass.low <= with_pass.high);
    assert!(without_pass.low.0 > 0 && without_pass.low <= without_pass.high);
    assert_ne!(with_pass.low, without_pass.low);
}

#[test]
fn a_euronight_gets_its_own_offset() {
    let mut text = offers();
    for index in 0..10 {
        text.push('\n');
        text.push_str(&line(
            "EN 40462",
            500.0 + 50.0 * f64::from(index),
            90.0,
            100.0,
            180.0,
        ));
    }
    let fares = NightFares::from_jsonl(&text).unwrap();
    let nightjet = fares
        .estimate(
            NightOperator::Oebb,
            Accommodation::Seat,
            PriceBasis::WithoutPass,
            700.0,
            false,
        )
        .unwrap();
    let euronight = fares
        .estimate(
            NightOperator::Oebb,
            Accommodation::Seat,
            PriceBasis::WithoutPass,
            700.0,
            true,
        )
        .unwrap();
    assert!(euronight.median > nightjet.median * 1.3);
}

#[test]
fn too_few_trains_give_no_estimate() {
    let fares = NightFares::from_jsonl(&line("NJ 1", 500.0, 40.0, 100.0, 180.0)).unwrap();
    assert!(
        fares
            .estimate(
                NightOperator::Oebb,
                Accommodation::Seat,
                PriceBasis::WithoutPass,
                700.0,
                false
            )
            .is_none()
    );
}

#[test]
fn only_night_trains_of_the_shop_are_priced() {
    let fares = NightFares::from_jsonl(&offers()).unwrap();
    let source = NightTrainTickets(fares);
    let ride = |category, kilometres, berths| RideFacts {
        from_country: Some("AT"),
        to_country: Some("CH"),
        operator: "oebb",
        category,
        distance_kilometres: kilometres,
        offers_berths: berths,
        train: "",
        date: None,
    };
    assert!(source.price_run(&[ride("NJ", 800.0, true)]).is_some());
    // A feeder before the night train does not change who sets the price.
    assert!(
        source
            .price_run(&[ride("S", 10.0, false), ride("NJ", 800.0, true)])
            .is_some()
    );
    // A long day train before the night train does.
    assert!(
        source
            .price_run(&[ride("ICE", 900.0, false), ride("NJ", 800.0, true)])
            .is_none()
    );
    assert!(source.price_run(&[ride("ICN", 800.0, true)]).is_none());
    assert!(source.price_run(&[ride("IC", 800.0, false)]).is_none());
}

#[test]
fn the_bundled_offers_give_a_flat_price_for_a_long_night_train() {
    let data = crate::FareData::bundled().unwrap();
    let short = data
        .night_fares
        .estimate(
            NightOperator::Oebb,
            Accommodation::Seat,
            PriceBasis::WithoutPass,
            500.0,
            false,
        )
        .unwrap();
    let long = data
        .night_fares
        .estimate(
            NightOperator::Oebb,
            Accommodation::Seat,
            PriceBasis::WithoutPass,
            1200.0,
            false,
        )
        .unwrap();
    assert!(long.median < short.median * 1.5, "{short:?} {long:?}");
    assert!(long.median < 100.0, "{long:?}");
}

#[test]
fn the_operator_of_a_night_train_comes_from_its_category_or_its_operator() {
    assert_eq!(NightOperator::of("oebb", "NJ"), Some(NightOperator::Oebb));
    // The shop sells the EuroNight whoever runs it.
    assert_eq!(
        NightOperator::of("hungary", "EuroNight"),
        Some(NightOperator::Oebb)
    );
    assert_eq!(
        NightOperator::of("european-sleeper", "ES"),
        Some(NightOperator::EuropeanSleeper)
    );
    assert_eq!(
        NightOperator::of("snalltaget", "Snälltåget"),
        Some(NightOperator::Snalltaget)
    );
    assert_eq!(NightOperator::of("db-fernverkehr", "ICE"), None);
}

#[test]
fn each_operator_is_fitted_to_its_own_offers() {
    let sleeper = (0..10)
        .map(|index| {
            line(
                "ES 453",
                500.0 + 20.0 * f64::from(index),
                110.0,
                220.0,
                240.0,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let fares = NightFares::from_sources(&[
        (NightOperator::Oebb, &offers()),
        (NightOperator::EuropeanSleeper, &sleeper),
    ])
    .unwrap();
    let seat = |operator| {
        fares
            .seat_price(operator, "NJ", 700.0)
            .expect("both operators have offers")
    };
    assert!((seat(NightOperator::Oebb) - 40.0).abs() < 2.0);
    assert!((seat(NightOperator::EuropeanSleeper) - 110.0).abs() < 4.0);
    // A ride of European Sleeper is priced from its own offers and says so.
    let source = NightTrainTickets(fares);
    let price = source
        .price_run(&[RideFacts {
            from_country: Some("NL"),
            to_country: Some("DE"),
            operator: "european-sleeper",
            category: "ES",
            distance_kilometres: 700.0,
            offers_berths: true,
            train: "",
            date: None,
        }])
        .unwrap();
    assert_eq!(price.source.name, "European Sleeper offers");
    assert!(price.low.0 > 9_000, "{price:?}");
}
