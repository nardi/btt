//! The sources that price a single ticket, tried from the most to the least exact.

use btt_model::Price;

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

/// The price sources in the order they are tried.
pub struct PriceBook {
    /// Sources from the most to the least exact. The last one prices every run.
    sources: Vec<Box<dyn TicketPriceSource>>,
}

impl PriceBook {
    /// Builds the book of the sources that the data files provide.
    pub fn from_data(data: &FareData) -> Self {
        let mut sources: Vec<Box<dyn TicketPriceSource>> = data
            .tariffs
            .iter()
            .map(|tariff| Box::new(tariff.clone()) as Box<dyn TicketPriceSource>)
            .collect();
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
