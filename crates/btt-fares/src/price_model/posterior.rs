//! A Bayesian linear regression with a normal prior that learns from one observation at a time
//! and forgets old observations by decaying towards its prior.

use nalgebra::{Cholesky, DMatrix, DVector};

/// A sparse feature vector as pairs of a feature index and its value.
pub(crate) type SparseFeatures = [(usize, f64)];

/// The posterior of the weights of a linear regression, with the prior it decays towards.
///
/// The precision matrix and the information vector are the exact state. The mean and the
/// covariance follow from them and are kept up to date, so a prediction needs no solve.
#[derive(Debug, Clone)]
pub(crate) struct Posterior {
    /// Prior mean of each weight.
    prior_mean: DVector<f64>,
    /// Prior precision of each weight. The prior has no correlations, so this is its diagonal.
    prior_precision: DVector<f64>,
    /// Precision matrix of the posterior, `Λ`.
    precision: DMatrix<f64>,
    /// Information vector of the posterior, `h = Λ μ`.
    information: DVector<f64>,
    /// Posterior mean `μ`.
    mean: DVector<f64>,
    /// Posterior covariance `Σ = Λ⁻¹`.
    covariance: DMatrix<f64>,
}

impl Posterior {
    /// Creates a posterior with no weights.
    pub(crate) fn new() -> Self {
        Self {
            prior_mean: DVector::zeros(0),
            prior_precision: DVector::zeros(0),
            precision: DMatrix::zeros(0, 0),
            information: DVector::zeros(0),
            mean: DVector::zeros(0),
            covariance: DMatrix::zeros(0, 0),
        }
    }

    /// Returns the number of weights.
    pub(crate) fn len(&self) -> usize {
        self.mean.len()
    }

    /// Adds a weight with its prior and returns its index.
    ///
    /// No observation has involved the new weight yet, so its posterior is its prior and it has
    /// no correlation with the other weights.
    pub(crate) fn add_weight(&mut self, mean: f64, standard_deviation: f64) -> usize {
        let index = self.len();
        let size = index + 1;
        let precision = 1.0 / (standard_deviation * standard_deviation);
        self.prior_mean.resize_vertically_mut(size, mean);
        self.prior_precision.resize_vertically_mut(size, precision);
        self.precision.resize_mut(size, size, 0.0);
        self.precision[(index, index)] = precision;
        self.information
            .resize_vertically_mut(size, precision * mean);
        self.mean.resize_vertically_mut(size, mean);
        self.covariance.resize_mut(size, size, 0.0);
        self.covariance[(index, index)] = 1.0 / precision;
        index
    }

    /// Returns the mean `xᵀ μ` and the variance `xᵀ Σ x` of the prediction for a feature vector.
    pub(crate) fn moments(&self, features: &SparseFeatures) -> (f64, f64) {
        let mean = features
            .iter()
            .map(|&(index, value)| value * self.mean[index])
            .sum();
        let variance = features
            .iter()
            .flat_map(|&(row, row_value)| {
                features.iter().map(move |&(column, column_value)| {
                    row_value * column_value * self.covariance[(row, column)]
                })
            })
            .sum();
        (mean, variance)
    }

    /// Learns from one observation `y` of a feature vector with a noise variance.
    ///
    /// The precision and the information gain the observation exactly. The mean and the
    /// covariance follow through the Sherman-Morrison formula, which costs d² and needs no
    /// factorization.
    pub(crate) fn observe(&mut self, features: &SparseFeatures, y: f64, noise_variance: f64) {
        for &(row, row_value) in features {
            self.information[row] += row_value * y / noise_variance;
            for &(column, column_value) in features {
                self.precision[(row, column)] += row_value * column_value / noise_variance;
            }
        }
        // The column `Σ x`, which the covariance loses a multiple of.
        let mut covariance_column = DVector::zeros(self.len());
        for &(index, value) in features {
            covariance_column.axpy(value, &self.covariance.column(index), 1.0);
        }
        let (predicted, variance) = self.moments(features);
        let denominator = noise_variance + variance;
        self.mean
            .axpy((y - predicted) / denominator, &covariance_column, 1.0);
        self.covariance.ger(
            -1.0 / denominator,
            &covariance_column,
            &covariance_column,
            1.0,
        );
    }

    /// Moves the posterior towards the prior, keeping `data_weight` of what the observations
    /// added, and recomputes the mean and the covariance from the precision.
    ///
    /// Moving towards the prior adds a full-rank term to the precision, so this takes a
    /// Cholesky factorization. It also clears the rounding errors that the rank-one updates of
    /// the covariance collect.
    pub(crate) fn decay(&mut self, data_weight: f64) {
        let prior_weight = 1.0 - data_weight;
        self.precision *= data_weight;
        for index in 0..self.len() {
            let precision = self.prior_precision[index];
            self.precision[(index, index)] += prior_weight * precision;
            self.information[index] = data_weight * self.information[index]
                + prior_weight * precision * self.prior_mean[index];
        }
        self.refactor();
    }

    /// Recomputes the mean and the covariance from the precision and the information.
    pub(crate) fn refactor(&mut self) {
        // The prior keeps the precision positive definite, so the factorization only fails on
        // a matrix that rounding has broken, and then the current mean and covariance stay.
        if let Some(factor) = Cholesky::new(self.precision.clone()) {
            self.mean = factor.solve(&self.information);
            self.covariance = factor.inverse();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fits `y = 2 + 3 t` with a wide prior and checks the mean against least squares.
    #[test]
    fn many_observations_find_the_least_squares_line() {
        let mut posterior = Posterior::new();
        let intercept = posterior.add_weight(0.0, 100.0);
        let slope = posterior.add_weight(0.0, 100.0);
        for step in 0..20 {
            let t = f64::from(step) / 4.0;
            posterior.observe(&[(intercept, 1.0), (slope, t)], 2.0 + 3.0 * t, 0.01);
        }
        assert!((posterior.mean[intercept] - 2.0).abs() < 1e-3);
        assert!((posterior.mean[slope] - 3.0).abs() < 1e-3);
    }

    #[test]
    fn the_updated_covariance_matches_the_inverse_of_the_precision() {
        let mut posterior = Posterior::new();
        let first = posterior.add_weight(1.0, 0.5);
        let second = posterior.add_weight(-1.0, 0.3);
        posterior.observe(&[(first, 1.0), (second, 2.0)], 0.4, 0.09);
        posterior.observe(&[(first, 1.0)], 1.5, 0.09);
        let updated_mean = posterior.mean.clone();
        let updated_covariance = posterior.covariance.clone();
        posterior.refactor();
        assert!((updated_mean - &posterior.mean).amax() < 1e-9);
        assert!((updated_covariance - &posterior.covariance).amax() < 1e-9);
    }

    #[test]
    fn a_full_decay_returns_to_the_prior() {
        let mut posterior = Posterior::new();
        let weight = posterior.add_weight(1.0, 0.5);
        posterior.observe(&[(weight, 1.0)], 3.0, 0.01);
        assert!(posterior.mean[weight] > 2.5);
        posterior.decay(0.0);
        assert!((posterior.mean[weight] - 1.0).abs() < 1e-12);
        assert!((posterior.covariance[(weight, weight)] - 0.25).abs() < 1e-12);
    }

    #[test]
    fn a_new_weight_keeps_the_others_as_they_are() {
        let mut posterior = Posterior::new();
        let first = posterior.add_weight(0.0, 1.0);
        posterior.observe(&[(first, 1.0)], 2.0, 0.5);
        let mean = posterior.mean[first];
        let second = posterior.add_weight(5.0, 2.0);
        assert!((posterior.mean[first] - mean).abs() < 1e-12);
        assert!((posterior.mean[second] - 5.0).abs() < 1e-12);
        assert!(posterior.covariance[(first, second)].abs() < 1e-12);
        assert!((posterior.covariance[(second, second)] - 4.0).abs() < 1e-12);
    }
}
