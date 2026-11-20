//! Multinomial Naive Bayes (deterministic alternative to the MLP, see the contract's model
//! decision). Same definition as sklearn `MultinomialNB(alpha=1.0, fit_prior=True)`, with
//! optional sample weights (used for upstream's "balanced" weighting).

use serde::{Deserialize, Serialize};

use crate::b64;
use crate::vectorizer::Csr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nb {
    pub n_features: usize,
    pub n_classes: usize,
    #[serde(with = "b64")]
    pub class_log_prior: Vec<f32>,
    /// n_classes x n_features, row-major
    #[serde(with = "b64")]
    pub feature_log_prob: Vec<f32>,
}

impl Nb {
    pub fn fit(
        x: &Csr,
        class_of_row: &[usize],
        n_classes: usize,
        sample_weight: Option<&[f32]>,
        alpha: f64,
    ) -> Nb {
        let f = x.n_cols;
        let mut fc = vec![0f64; n_classes * f];
        let mut cc = vec![0f64; n_classes];
        for (r, &c) in class_of_row.iter().enumerate() {
            let w = sample_weight.map_or(1.0, |s| s[r] as f64);
            cc[c] += w;
            let (cols, vals) = x.row(r);
            for (&j, &v) in cols.iter().zip(vals) {
                fc[c * f + j as usize] += w * v as f64;
            }
        }
        let total: f64 = cc.iter().sum();
        let class_log_prior = cc
            .iter()
            .map(|&c| {
                if c > 0.0 {
                    (c / total).ln() as f32
                } else {
                    f32::NEG_INFINITY
                }
            })
            .collect();
        let mut feature_log_prob = vec![0f32; n_classes * f];
        for c in 0..n_classes {
            let row = &fc[c * f..(c + 1) * f];
            let denom: f64 = row.iter().sum::<f64>() + alpha * f as f64;
            for j in 0..f {
                feature_log_prob[c * f + j] = ((row[j] + alpha) / denom).ln() as f32;
            }
        }
        Nb {
            n_features: f,
            n_classes,
            class_log_prior,
            feature_log_prob,
        }
    }

    /// Class probabilities for one sparse row (softmax of the joint log likelihood).
    pub fn predict_proba(&self, row: &[(u32, f32)]) -> Vec<f64> {
        let f = self.n_features;
        let mut jll: Vec<f64> = self.class_log_prior.iter().map(|&v| v as f64).collect();
        for (c, j) in jll.iter_mut().enumerate() {
            for &(col, v) in row {
                *j += v as f64 * self.feature_log_prob[c * f + col as usize] as f64;
            }
        }
        let m = jll.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let mut s = 0.0;
        for v in jll.iter_mut() {
            *v = (*v - m).exp();
            s += *v;
        }
        jll.iter().map(|v| v / s).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separates_two_classes() {
        let x = Csr::from_rows(
            3,
            &[
                vec![(0, 3.0)],
                vec![(0, 2.0), (2, 1.0)],
                vec![(1, 3.0)],
                vec![(1, 1.0), (2, 1.0)],
            ],
        );
        let nb = Nb::fit(&x, &[0, 0, 1, 1], 2, None, 1.0);
        assert!(nb.predict_proba(&[(0, 1.0)])[0] > 0.5);
        assert!(nb.predict_proba(&[(1, 1.0)])[1] > 0.5);
        let p = nb.predict_proba(&[(2, 1.0)]);
        assert!((p[0] + p[1] - 1.0).abs() < 1e-9);
    }
}
