//! Quality measure used to compare the Rust classifier with upstream (scikit-learn).
//! Same definitions as `metrics()` in tests/parity/pl-classifier/gen_reference.py.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SingleMetrics {
    /// fraction of documents where prediction == truth (None counts as a value)
    pub accuracy: f64,
    /// mean F1 over the real classes (not None) occurring in truth or prediction
    pub macro_f1: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TagMetrics {
    pub micro_f1: f64,
    pub macro_f1: f64,
    /// fraction of documents whose predicted tag set equals the true set
    pub exact_match: f64,
}

fn f1(tp: usize, fp: usize, fn_: usize) -> f64 {
    let d = 2 * tp + fp + fn_;
    if d == 0 { 0.0 } else { 2.0 * tp as f64 / d as f64 }
}

pub fn single(pred: &[Option<i64>], truth: &[Option<i64>]) -> SingleMetrics {
    let n = truth.len().max(1) as f64;
    let acc = pred.iter().zip(truth).filter(|(p, t)| p == t).count() as f64 / n;
    let classes: BTreeSet<i64> = pred.iter().chain(truth).flatten().copied().collect();
    let f1s: Vec<f64> = classes
        .iter()
        .map(|&c| {
            let c = Some(c);
            let tp = pred.iter().zip(truth).filter(|(p, t)| **p == c && **t == c).count();
            let fp = pred.iter().zip(truth).filter(|(p, t)| **p == c && **t != c).count();
            let fn_ = pred.iter().zip(truth).filter(|(p, t)| **p != c && **t == c).count();
            f1(tp, fp, fn_)
        })
        .collect();
    let macro_f1 = if f1s.is_empty() { 0.0 } else { f1s.iter().sum::<f64>() / f1s.len() as f64 };
    SingleMetrics { accuracy: acc, macro_f1 }
}

pub fn tags(pred: &[Vec<i64>], truth: &[Vec<i64>]) -> TagMetrics {
    let classes: BTreeSet<i64> = pred.iter().chain(truth).flatten().copied().collect();
    let (mut tp_all, mut fp_all, mut fn_all) = (0, 0, 0);
    let mut f1s = Vec::new();
    for c in &classes {
        let (mut tp, mut fp, mut fn_) = (0, 0, 0);
        for (p, t) in pred.iter().zip(truth) {
            match (p.contains(c), t.contains(c)) {
                (true, true) => tp += 1,
                (true, false) => fp += 1,
                (false, true) => fn_ += 1,
                _ => {}
            }
        }
        tp_all += tp;
        fp_all += fp;
        fn_all += fn_;
        f1s.push(f1(tp, fp, fn_));
    }
    let exact = pred.iter().zip(truth).filter(|(p, t)| p.iter().collect::<BTreeSet<_>>() == t.iter().collect::<BTreeSet<_>>()).count()
        as f64
        / truth.len().max(1) as f64;
    TagMetrics {
        micro_f1: f1(tp_all, fp_all, fn_all),
        macro_f1: if f1s.is_empty() { 0.0 } else { f1s.iter().sum::<f64>() / f1s.len() as f64 },
        exact_match: exact,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_metrics() {
        let m = single(&[Some(1), None, Some(2), Some(1)], &[Some(1), Some(2), Some(2), None]);
        assert_eq!(m.accuracy, 0.5);
        // class 1: tp1 fp1 fn0 -> 2/3; class 2: tp1 fp0 fn1 -> 2/3
        assert!((m.macro_f1 - 2.0 / 3.0).abs() < 1e-12);
    }
}
