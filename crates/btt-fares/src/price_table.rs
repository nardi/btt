//! A table of prices per class of ride, built from the posterior of the price model, so a
//! search can price millions of rides without running the model.

use std::collections::HashMap;

use rustc_hash::FxBuildHasher;

use crate::PriceModel;
use smol_str::SmolStr;

/// A class of rides that share one price row: their operator, their category, whether their
/// trains have berths, and the country of the station they board at.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RouteClass {
    /// The operator as the app names it, such as `db-fernverkehr`. Empty when not known.
    pub operator: SmolStr,
    /// The category of the train, such as "ICE".
    pub category: SmolStr,
    /// Whether the trains have couchettes or sleepers, which makes them a night kind.
    pub offers_berths: bool,
    /// The two-letter code of the country of the boarding station, if known.
    pub country: Option<SmolStr>,
}

/// The log price of the rides of one class, as a search computes it. The price of a ride of
/// `kilometres` is `exp(constant + distance·u + curve·u² + base·b)`, where `u` is the log of
/// the kilometres over the reference distance and `b` fades with the distance the way the
/// model's base term does. The border surcharge is left out, because the search prices a ride
/// by the station it boards at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PriceRow {
    /// The log price at the reference distance, with every offset of the class.
    pub constant: f32,
    /// The coefficient of the log distance, the sum of every slope of the class.
    pub distance: f32,
    /// The coefficient of the squared log distance.
    pub curve: f32,
    /// The coefficient of the base term, which stands in for a base fare.
    pub base: f32,
    /// The kilometres the country's tariff shifts the distance by.
    pub shift: f32,
}

/// The price rows of route classes, built once from the posterior mean of the price model.
/// A row is a snapshot: prices the model learns later do not change it.
#[derive(Debug, Clone)]
pub struct PriceTable {
    /// The rows, in the order the classes were given.
    rows: Vec<PriceRow>,
    /// Where each class has its row, in the order the classes were given.
    row_by_class: HashMap<RouteClass, usize, FxBuildHasher>,
    /// The distance the log price divides by, in kilometres.
    reference_kilometres: f32,
}

impl PriceTable {
    /// Builds the table from the posterior mean of the model, one row per class.
    pub fn from_model(model: &PriceModel, classes: &[RouteClass]) -> Self {
        let mut row_by_class = HashMap::with_capacity_and_hasher(classes.len(), FxBuildHasher);
        let mut rows = Vec::with_capacity(classes.len());
        for (index, class) in classes.iter().enumerate() {
            row_by_class.insert(class.clone(), index);
            rows.push(model.route_row(
                &class.operator,
                &class.category,
                class.offers_berths,
                class.country.as_deref(),
            ));
        }
        Self {
            rows,
            row_by_class,
            reference_kilometres: model.reference_kilometres(),
        }
    }

    /// Returns the row of a class, or `None` for a class the table was not built with.
    pub fn row(&self, class: &RouteClass) -> Option<&PriceRow> {
        self.row_by_class.get(class).map(|&index| &self.rows[index])
    }

    /// Returns the rows, in the order the classes were given.
    pub fn rows(&self) -> &[PriceRow] {
        &self.rows
    }

    /// Returns the distance the log price divides by, in kilometres.
    pub fn reference_kilometres(&self) -> f32 {
        self.reference_kilometres
    }
}
