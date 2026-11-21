// upstream: src/documents/classifier.py@v3.4.2 (MLPClassifier(tol=0.01, random_state=0))
// sklearn 1.9.1 sklearn/neural_network/_multilayer_perceptron.py (BaseMultilayerPerceptron._fit_stochastic,
//   _backprop, _compute_loss_grad, _init_coef, _update_no_improvement_count, MLPClassifier defaults:
//   hidden_layer_sizes=(100,), activation="relu", solver="adam", alpha=1e-4, batch_size=min(200, n),
//   learning_rate_init=1e-3, max_iter=200, shuffle=True, n_iter_no_change=10, early_stopping=False)
// sklearn 1.9.1 sklearn/neural_network/_stochastic_optimizers.py (AdamOptimizer)
// sklearn 1.9.1 sklearn/neural_network/_base.py (log_loss, binary_log_loss)
//
// Same algorithm, own RNG and f32 weights: results are statistically equivalent, not bit-identical.

use serde::{Deserialize, Serialize};

use crate::b64;
use crate::rng::Rng;
use crate::vectorizer::Csr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutAct {
    /// multiclass (> 2 classes)
    Softmax,
    /// binary (one unit) or multilabel (one unit per label)
    Logistic,
}

#[derive(Debug, Clone)]
pub struct MlpParams {
    /// Hidden layer sizes; empty = linear model (multinomial/binary logistic regression).
    pub hidden: Vec<usize>,
    pub alpha: f64,
    pub learning_rate_init: f64,
    pub beta_1: f64,
    pub beta_2: f64,
    pub epsilon: f64,
    pub tol: f64,
    pub n_iter_no_change: usize,
    pub max_iter: usize,
    pub batch_size: usize,
    pub seed: u64,
}

impl Default for MlpParams {
    fn default() -> Self {
        MlpParams {
            hidden: vec![100],
            alpha: 1e-4,
            learning_rate_init: 1e-3,
            beta_1: 0.9,
            beta_2: 0.999,
            epsilon: 1e-8,
            tol: 0.01,
            n_iter_no_change: 10,
            max_iter: 200,
            batch_size: 200,
            seed: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layer {
    pub fan_in: usize,
    pub fan_out: usize,
    /// row-major fan_in x fan_out (sklearn coefs_[i])
    #[serde(with = "b64")]
    pub w: Vec<f32>,
    #[serde(with = "b64")]
    pub b: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mlp {
    pub layers: Vec<Layer>,
    pub out: OutAct,
    pub n_iter: usize,
    pub final_loss: f64,
}

fn sigmoid(x: f32) -> f32 {
    // scipy.special.expit, numerically stable
    if x >= 0.0 {
        1.0 / (1.0 + (-x).exp())
    } else {
        let e = x.exp();
        e / (1.0 + e)
    }
}

fn softmax_inplace(row: &mut [f32]) {
    let m = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut s = 0.0f32;
    for v in row.iter_mut() {
        *v = (*v - m).exp();
        s += *v;
    }
    for v in row.iter_mut() {
        *v /= s;
    }
}

/// Adam hyper-parameters of one step.
#[derive(Clone, Copy)]
struct AdamStep {
    /// L2 factor `alpha`
    decay: f32,
    /// `1 / sum(sample_weight)`
    inv: f32,
    lr: f32,
    b1: f32,
    b2: f32,
    eps: f32,
}

/// Adam step for one parameter vector (sklearn `AdamOptimizer._get_updates`), element-wise.
fn adam_step(w: &mut [f32], m: &mut [f32], v: &mut [f32], g: &[f32], a: AdamStep) {
    for i in 0..w.len() {
        let grad = (g[i] + a.decay * w[i]) * a.inv;
        m[i] = a.b1 * m[i] + (1.0 - a.b1) * grad;
        v[i] = a.b2 * v[i] + (1.0 - a.b2) * grad * grad;
        w[i] -= a.lr * m[i] / (v[i].sqrt() + a.eps);
    }
}

/// Input-layer gradient `gw[c*fo + j] += v * delta[r*fo + j]`, rows in batch order.
fn input_grad(x: &Csr, idx: &[usize], delta: &[f32], fo: usize, gw: &mut [f32]) {
    for (r, &row_i) in idx.iter().enumerate() {
        let (cols, vals) = x.row(row_i);
        let dr = &delta[r * fo..(r + 1) * fo];
        for (&c, &v) in cols.iter().zip(vals) {
            let base = c as usize * fo;
            let g = &mut gw[base..base + fo];
            for (gj, dj) in g.iter_mut().zip(dr) {
                *gj += v * dj;
            }
        }
    }
}

impl Mlp {
    pub fn n_out(&self) -> usize {
        self.layers.last().map_or(0, |l| l.fan_out)
    }

    /// Forward pass for a batch of sparse rows; returns activations per layer output
    /// (acts[k] = output of layer k, bs x fan_out).
    fn forward(&self, x: &Csr, idx: &[usize]) -> Vec<Vec<f32>> {
        let bs = idx.len();
        let mut acts: Vec<Vec<f32>> = Vec::with_capacity(self.layers.len());
        let last = self.layers.len() - 1;
        for (k, layer) in self.layers.iter().enumerate() {
            let h = layer.fan_out;
            let mut z = vec![0f32; bs * h];
            let prev_acts = acts.last();
            let row_fn = |r: usize, zr: &mut [f32]| {
                zr.copy_from_slice(&layer.b);
                if k == 0 {
                    let (cols, vals) = x.row(idx[r]);
                    for (&c, &v) in cols.iter().zip(vals) {
                        let wr = &layer.w[c as usize * h..(c as usize + 1) * h];
                        for (zj, wj) in zr.iter_mut().zip(wr) {
                            *zj += v * wj;
                        }
                    }
                } else {
                    let prev = prev_acts.expect("previous layer output");
                    let fi = layer.fan_in;
                    let ar = &prev[r * fi..(r + 1) * fi];
                    for (i, &a) in ar.iter().enumerate() {
                        if a != 0.0 {
                            let wr = &layer.w[i * h..(i + 1) * h];
                            for (zj, wj) in zr.iter_mut().zip(wr) {
                                *zj += a * wj;
                            }
                        }
                    }
                }
                if k < last {
                    for v in zr.iter_mut() {
                        if *v < 0.0 {
                            *v = 0.0;
                        }
                    }
                } else {
                    match self.out {
                        OutAct::Softmax => softmax_inplace(zr),
                        OutAct::Logistic => {
                            for v in zr.iter_mut() {
                                *v = sigmoid(*v);
                            }
                        }
                    }
                }
            };
            for (r, zr) in z.chunks_mut(h).enumerate() {
                row_fn(r, zr);
            }
            acts.push(z);
        }
        acts
    }

    /// Output activations for one sparse row (sklearn `_forward_pass_fast`).
    pub fn output(&self, row: &[(u32, f32)]) -> Vec<f32> {
        let x = Csr::from_rows(self.layers[0].fan_in, &[row.to_vec()]);
        self.forward(&x, &[0]).pop().unwrap_or_default()
    }

    /// Train like sklearn `MLPClassifier.fit` with solver adam. `y` is the binarized target
    /// (n x n_out, row-major), `sample_weight` optional.
    pub fn fit(x: &Csr, y: &[f32], n_out: usize, out: OutAct, sample_weight: Option<&[f32]>, p: &MlpParams) -> Mlp {
        let n = x.n_rows();
        let mut rng = Rng::new(p.seed);
        // layer sizes
        let mut sizes = vec![x.n_cols];
        sizes.extend(p.hidden.iter().copied());
        sizes.push(n_out);
        let mut layers = Vec::new();
        for k in 0..sizes.len() - 1 {
            let (fi, fo) = (sizes[k], sizes[k + 1]);
            // _init_coef: Glorot uniform, factor 6 for relu
            let bound = (6.0 / (fi + fo) as f64).sqrt();
            let w = (0..fi * fo).map(|_| rng.uniform(-bound, bound) as f32).collect();
            let b = (0..fo).map(|_| rng.uniform(-bound, bound) as f32).collect();
            layers.push(Layer { fan_in: fi, fan_out: fo, w, b });
        }
        let mut model = Mlp { layers, out, n_iter: 0, final_loss: f64::NAN };
        if n == 0 {
            return model;
        }
        let nl = model.layers.len();
        let mut mw: Vec<Vec<f32>> = model.layers.iter().map(|l| vec![0f32; l.w.len()]).collect();
        let mut vw = mw.clone();
        let mut mb: Vec<Vec<f32>> = model.layers.iter().map(|l| vec![0f32; l.b.len()]).collect();
        let mut vb = mb.clone();
        // gradient accumulators (sparse part for the input layer, dense for others)
        let mut gw: Vec<Vec<f32>> = model.layers.iter().map(|l| vec![0f32; l.w.len()]).collect();
        let mut touched = vec![false; x.n_cols];
        let mut touched_list: Vec<usize> = Vec::new();
        let batch_size = p.batch_size.clamp(1, n);
        let mut sample_idx: Vec<usize> = (0..n).collect();
        let mut t: i32 = 0;
        let mut best_loss = f64::INFINITY;
        let mut no_improve = 0usize;
        let eps64 = f64::EPSILON;
        let (b1, b2) = (p.beta_1 as f32, p.beta_2 as f32);

        for _epoch in 0..p.max_iter {
            rng.shuffle(&mut sample_idx);
            let mut accumulated = 0.0f64;
            for start in (0..n).step_by(batch_size) {
                let idx = &sample_idx[start..(start + batch_size).min(n)];
                let bs = idx.len();
                let acts = model.forward(x, idx);
                let sw: Vec<f32> = match sample_weight {
                    Some(w) => idx.iter().map(|&i| w[i]).collect(),
                    None => vec![1.0; bs],
                };
                let sw_sum: f64 = sw.iter().map(|&v| v as f64).sum();
                // loss (weighted average) + L2
                let outp = &acts[nl - 1];
                let mut loss = 0.0f64;
                for r in 0..bs {
                    let mut l = 0.0f64;
                    for k in 0..n_out {
                        let yv = y[idx[r] * n_out + k] as f64;
                        let pv = (outp[r * n_out + k] as f64).clamp(eps64, 1.0 - eps64);
                        match out {
                            OutAct::Softmax => {
                                if yv != 0.0 {
                                    l -= yv * pv.ln();
                                }
                            }
                            OutAct::Logistic => {
                                if yv != 0.0 {
                                    l -= yv * pv.ln();
                                }
                                if yv != 1.0 {
                                    l -= (1.0 - yv) * (1.0 - pv).ln();
                                }
                            }
                        }
                    }
                    loss += sw[r] as f64 * l;
                }
                loss /= sw_sum;
                let sq: f64 = model.layers.iter().map(|l| l.w.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>()).sum();
                loss += 0.5 * p.alpha * sq / sw_sum;
                accumulated += loss * bs as f64;

                // backward
                let mut delta: Vec<f32> = (0..bs * n_out)
                    .map(|i| {
                        let (r, k) = (i / n_out, i % n_out);
                        (outp[i] - y[idx[r] * n_out + k]) * sw[r]
                    })
                    .collect();
                let mut gb: Vec<Vec<f32>> = model.layers.iter().map(|l| vec![0f32; l.b.len()]).collect();
                for k in (0..nl).rev() {
                    let fo = model.layers[k].fan_out;
                    let fi = model.layers[k].fan_in;
                    for r in 0..bs {
                        for j in 0..fo {
                            gb[k][j] += delta[r * fo + j];
                        }
                    }
                    if k == 0 {
                        // touched columns first, then the gradient (same per-element order)
                        for &row_i in idx {
                            for &c in x.row(row_i).0 {
                                let c = c as usize;
                                if !touched[c] {
                                    touched[c] = true;
                                    touched_list.push(c);
                                }
                            }
                        }
                        input_grad(x, idx, &delta, fo, &mut gw[0]);
                    } else {
                        let a = &acts[k - 1];
                        let g = &mut gw[k];
                        for r in 0..bs {
                            let ar = &a[r * fi..(r + 1) * fi];
                            let dr = &delta[r * fo..(r + 1) * fo];
                            for (i, &av) in ar.iter().enumerate() {
                                if av != 0.0 {
                                    let gr = &mut g[i * fo..(i + 1) * fo];
                                    for (gj, dj) in gr.iter_mut().zip(dr) {
                                        *gj += av * dj;
                                    }
                                }
                            }
                        }
                        // delta for previous layer: delta W^T * relu'(a)
                        let w = &model.layers[k].w;
                        let mut nd = vec![0f32; bs * fi];
                        for r in 0..bs {
                            let dr = &delta[r * fo..(r + 1) * fo];
                            for i in 0..fi {
                                if a[r * fi + i] > 0.0 {
                                    let wr = &w[i * fo..(i + 1) * fo];
                                    nd[r * fi + i] = wr.iter().zip(dr).map(|(a, b)| a * b).sum();
                                }
                            }
                        }
                        delta = nd;
                    }
                }

                // Adam update (all parameters, like sklearn)
                t += 1;
                let lr = (p.learning_rate_init * (1.0 - p.beta_2.powi(t)).sqrt() / (1.0 - p.beta_1.powi(t))) as f32;
                let inv = (1.0 / sw_sum) as f32;
                let alpha = p.alpha as f32;
                let eps = p.epsilon as f32;
                let step = AdamStep { decay: alpha, inv, lr, b1, b2, eps };
                for k in 0..nl {
                    let layer = &mut model.layers[k];
                    let (m, v, g) = (&mut mw[k], &mut vw[k], &mut gw[k]);
                    adam_step(&mut layer.w, m, v, g, step);
                    for i in 0..layer.b.len() {
                        let grad = gb[k][i] * inv;
                        mb[k][i] = b1 * mb[k][i] + (1.0 - b1) * grad;
                        vb[k][i] = b2 * vb[k][i] + (1.0 - b2) * grad * grad;
                        layer.b[i] -= lr * mb[k][i] / (vb[k][i].sqrt() + eps);
                    }
                    if k == 0 {
                        let fo = layer.fan_out;
                        for &c in &touched_list {
                            g[c * fo..(c + 1) * fo].fill(0.0);
                            touched[c] = false;
                        }
                        touched_list.clear();
                    } else {
                        g.fill(0.0);
                    }
                }
            }
            model.n_iter += 1;
            let epoch_loss = accumulated / n as f64;
            model.final_loss = epoch_loss;
            if epoch_loss > best_loss - p.tol {
                no_improve += 1;
            } else {
                no_improve = 0;
            }
            if epoch_loss < best_loss {
                best_loss = epoch_loss;
            }
            if no_improve > p.n_iter_no_change {
                break; // adam: trigger_stopping always stops
            }
        }
        model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toy() -> (Csr, Vec<usize>) {
        // 3 classes, each with its own feature (+ a shared noise feature)
        let mut rows = Vec::new();
        let mut cls = Vec::new();
        for i in 0..60usize {
            let c = i % 3;
            rows.push(vec![(c as u32, 2.0), (3, 1.0)]);
            cls.push(c);
        }
        (Csr::from_rows(4, &rows), cls)
    }

    #[test]
    fn learns_separable_multiclass() {
        let (x, cls) = toy();
        let mut y = vec![0f32; 60 * 3];
        for (i, &c) in cls.iter().enumerate() {
            y[i * 3 + c] = 1.0;
        }
        let p = MlpParams { tol: 1e-4, ..Default::default() };
        let m = Mlp::fit(&x, &y, 3, OutAct::Softmax, None, &p);
        for c in 0..3u32 {
            let o = m.output(&[(c, 2.0), (3, 1.0)]);
            let best = o.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|x| x.0);
            assert_eq!(best, Some(c as usize), "{o:?}");
        }
        // deterministic
        let m2 = Mlp::fit(&x, &y, 3, OutAct::Softmax, None, &p);
        assert_eq!(m.layers[0].w, m2.layers[0].w);
    }

    /// Measurement + bit equality (results/bench/worker/NOTES.md, T5): synthetic data at the size
    /// of the 100k benchmark (V=7034, ~150 features per document), 2 epochs.
    /// `cargo test --release -p pl-classifier -- --ignored --nocapture fit_timing`
    #[test]
    #[ignore = "measurement"]
    fn fit_timing_and_checksum() {
        let n: usize = std::env::var("PL_MLP_BENCH_ROWS").ok().and_then(|v| v.parse().ok()).unwrap_or(20_000);
        let (v, nnz) = (7034u32, 150usize);
        let mut rng = Rng::new(42);
        let rows: Vec<Vec<(u32, f32)>> = (0..n)
            .map(|_| {
                let mut r: Vec<u32> = (0..nnz).map(|_| (rng.uniform(0.0, 1.0).powi(2) * f64::from(v)) as u32 % v).collect();
                r.sort_unstable();
                r.dedup();
                r.into_iter().map(|c| (c, 1.0 + (c % 3) as f32)).collect()
            })
            .collect();
        let y: Vec<f32> = rows.iter().map(|r| if r.iter().any(|(c, _)| *c < 40) { 1.0 } else { 0.0 }).collect();
        let x = Csr::from_rows(v as usize, &rows);
        let sw: Vec<f32> = y.iter().map(|&t| if t > 0.5 { 2.0 } else { 0.7 }).collect();
        let p = MlpParams { max_iter: 2, ..Default::default() };
        let t = std::time::Instant::now();
        let m = Mlp::fit(&x, &y, 1, OutAct::Logistic, Some(&sw), &p);
        let el = t.elapsed();
        let sum: f64 =
            m.layers.iter().flat_map(|l| l.w.iter().chain(&l.b)).enumerate().map(|(i, w)| f64::from(*w) * ((i % 97) as f64 + 1.0)).sum();
        let bits: u64 = m.layers.iter().flat_map(|l| l.w.iter().chain(&l.b)).fold(0u64, |h, w| h.rotate_left(5) ^ u64::from(w.to_bits()));
        println!(
            "rows {n}: 2 epochs {:.2} s ({:.2} ms/batch), loss {:.12}, checksum {sum:.9}, bits {bits:016x}",
            el.as_secs_f64(),
            el.as_secs_f64() * 1e3 / (2.0 * (n as f64 / 200.0).ceil()),
            m.final_loss
        );
    }

    #[test]
    fn linear_binary() {
        let (x, cls) = toy();
        let y: Vec<f32> = cls.iter().map(|&c| if c == 1 { 1.0 } else { 0.0 }).collect();
        let p = MlpParams { hidden: vec![], tol: 1e-5, learning_rate_init: 0.05, ..Default::default() };
        let m = Mlp::fit(&x, &y, 1, OutAct::Logistic, None, &p);
        assert!(m.output(&[(1, 2.0), (3, 1.0)])[0] > 0.5);
        assert!(m.output(&[(0, 2.0), (3, 1.0)])[0] < 0.5);
    }
}
