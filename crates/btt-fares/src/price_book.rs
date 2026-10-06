//! The sources that price a single ticket, tried from the most to the least exact.

use btt_model::{EuroCents, Price, PriceKind, PriceSource};

use crate::FareData;

/// What a price source needs to know about one ride.
#[derive(Debug, Clone, Copy)]
pub struct RideFacts<'a> {
    /// Country of the boarding station as a two-letter code, if known.
    pub from_country: Option<&'a str>,
    /// Country of the alighting station, if known.
    pub to_country: Option<&'a str>,
    /// Category of the train, such as "ICE".
    pub category: &'a str,
    /// Length of the ride along its line, in kilometres.
    pub distance_kilometres: f64,
}

/// Prices a run of consecutive rides that one ticket covers.
pub trait TicketPriceSource: Send + Sync {
    /// Returns the price of one ticket for the whole run, or `None` when the source does not
    /// know the run, and the next source gets to try.
    fn price_run(&self, run: &[RideFacts<'_>]) -> Option<Price>;
}

/// A country where trains inside the country cost nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreeTravel {
    /// The two-letter code of the country.
    pub country: &'static str,
    /// What the traveller should know, for the price note.
    pub note: &'static str,
}

impl FreeTravel {
    /// Public transport inside Luxembourg is free in second class.
    pub const LUXEMBOURG: Self = Self {
        country: "LU",
        note: "Public transport inside Luxembourg is free in second class. First class needs a \
               ticket.",
    };
}

impl TicketPriceSource for FreeTravel {
    fn price_run(&self, run: &[RideFacts<'_>]) -> Option<Price> {
        let inside = !run.is_empty()
            && run.iter().all(|ride| {
                ride.from_country == Some(self.country) && ride.to_country == Some(self.country)
            });
        inside.then(|| Price {
            low: EuroCents::ZERO,
            high: EuroCents::ZERO,
            kind: PriceKind::Formula,
            source: PriceSource {
                name: "Free public transport".into(),
                note: self.note.into(),
                as_of: None,
                page: None,
            },
        })
    }
}

/// The price sources in the order they are tried.
pub struct PriceBook {
    /// Sources from the most to the least exact. The last one prices every run.
    sources: Vec<Box<dyn TicketPriceSource>>,
}

impl PriceBook {
    /// Builds the book of the sources that the data files provide.
    pub fn from_data(data: &FareData) -> Self {
        let mut sources: Vec<Box<dyn TicketPriceSource>> = vec![Box::new(FreeTravel::LUXEMBOURG)];
        sources.extend(
            data.tariffs
                .iter()
                .map(|tariff| Box::new(tariff.clone()) as Box<dyn TicketPriceSource>),
        );
        sources.push(Box::new(data.distance_model));
        Self { sources }
    }

    /// Builds a book from the given sources. The last one has to price every run.
    pub fn from_sources(sources: Vec<Box<dyn TicketPriceSource>>) -> Self {
        Self { sources }
    }

    /// Returns the price of one ticket that covers the run, from the first source that knows it.
    ///
    /// # Panics
    ///
    /// Panics when no source prices the run, which the last source of a book never allows.
    pub fn price_run(&self, run: &[RideFacts<'_>]) -> Price {
        self.sources
            .iter()
            .find_map(|source| source.price_run(run))
            .expect("the last source of a price book prices every run")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ride<'a>(from: &'a str, to: &'a str) -> RideFacts<'a> {
        RideFacts {
            from_country: Some(from),
            to_country: Some(to),
            category: "RE",
            distance_kilometres: 30.0,
        }
    }

    #[test]
    fn trains_inside_luxembourg_are_free() {
        let free = FreeTravel::LUXEMBOURG;
        let price = free.price_run(&[ride("LU", "LU")]).unwrap();
        assert_eq!(price.high, EuroCents::ZERO);
        assert!(free.price_run(&[ride("LU", "BE")]).is_none());
        assert!(
            free.price_run(&[ride("LU", "LU"), ride("LU", "FR")])
                .is_none()
        );
    }
}
