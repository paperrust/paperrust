// upstream: src/documents/classifier.py@v3.4.2 (DocumentClassifier.train/predict_*/save/load,
//           _predict_with_threshold, load_classifier)
// upstream: src/documents/tasks.py@v3.4.2 (train_classifier) - DB part is the caller's job

use std::path::{Path, PathBuf};

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ClassifierError;
use crate::mlp::{Mlp, MlpParams, OutAct};
use crate::nb::Nb;
use crate::text::{Language, preprocess_content};
use crate::vectorizer::{CountVectorizer, Csr};

/// Own model format version (independent of upstream's pickle FORMAT_VERSION 11, which this
/// port mirrors in behaviour). Bump on any incompatible change; old files are then deleted and
/// re-trained like upstream does on `IncompatibleClassifierVersionError`.
pub const FORMAT_VERSION: u32 = 1;
/// upstream `DocumentClassifier.FORMAT_VERSION` whose behaviour this port follows.
pub const UPSTREAM_FORMAT_VERSION: u32 = 11;
const MAGIC: &[u8] = b"PAPERRUST-CLASSIFIER\n";
const HMAC_SIZE: usize = 32;
/// upstream `_CONTENT_CHUNK_SIZE`
pub const CONTENT_CHUNK_SIZE: usize = 1000;
/// upstream default of `PAPERLESS_CLASSIFIER_MATCH_THRESHOLD`
pub const DEFAULT_MATCH_THRESHOLD: f64 = 0.3;

/// One training document. The caller (worker) builds these exactly like upstream `train()`:
/// `Document.objects.exclude(tags__is_inbox_tag=True).order_by("pk")` (soft-deleted documents are
/// excluded by the default manager, versions are included), and labels only for objects with
/// `matching_algorithm == MATCH_AUTO` (6), otherwise `None`; `tags` = the document's AUTO tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainingDoc {
    pub pk: i64,
    /// `Document.modified` as microseconds since the Unix epoch (UTC)
    pub modified_us: i64,
    pub correspondent: Option<i64>,
    pub document_type: Option<i64>,
    pub storage_path: Option<i64>,
    pub tags: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    /// MLP with one hidden layer of 100 units like upstream (default)
    Mlp,
    /// MLP without hidden layer (multinomial / binary logistic regression)
    Linear,
    /// Multinomial Naive Bayes
    NaiveBayes,
}

#[derive(Debug, Clone)]
pub struct TrainOptions {
    pub kind: ModelKind,
    pub mlp: MlpParams,
}

impl Default for TrainOptions {
    fn default() -> Self {
        TrainOptions {
            kind: ModelKind::Mlp,
            mlp: MlpParams::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Estimator {
    Mlp(Mlp),
    /// one multiclass NB (single-label) or one binary NB per label (multilabel)
    NaiveBayes {
        models: Vec<Nb>,
    },
    /// fitted on a single class: always that class (NB only; the MLP is trained like sklearn)
    Constant,
}

/// Single-label classifier (correspondent / document type / storage path); classes sorted,
/// `-1` = no (AUTO) object, like upstream labels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SingleLabel {
    pub classes: Vec<i64>,
    pub est: Estimator,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagBinarizer {
    /// upstream num_tags == 1: LabelBinarizer over [tag or -1]; classes sorted
    Label { classes: Vec<i64> },
    /// upstream num_tags > 1: MultiLabelBinarizer; classes sorted
    MultiLabel { classes: Vec<i64> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagModel {
    pub binarizer: TagBinarizer,
    pub est: Estimator,
}

/// Everything `predict_*` needs, plus upstream's retrain bookkeeping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentClassifier {
    pub format_version: u32,
    pub kind: ModelKind,
    /// language the vocabulary was built with; prediction preprocesses with it
    pub language: Option<Language>,
    /// upstream `last_doc_change_time`
    pub last_doc_change_us: Option<i64>,
    /// upstream `last_auto_type_hash` (hex)
    pub last_auto_type_hash: Option<String>,
    pub vectorizer: Option<CountVectorizer>,
    pub tags: Option<TagModel>,
    pub correspondent: Option<SingleLabel>,
    pub document_type: Option<SingleLabel>,
    pub storage_path: Option<SingleLabel>,
}

impl Default for DocumentClassifier {
    fn default() -> Self {
        DocumentClassifier {
            format_version: FORMAT_VERSION,
            kind: ModelKind::Mlp,
            language: None,
            last_doc_change_us: None,
            last_auto_type_hash: None,
            vectorizer: None,
            tags: None,
            correspondent: None,
            document_type: None,
            storage_path: None,
        }
    }
}

/// Predictions for one document (upstream `predict_*` results).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prediction {
    pub correspondent: Option<i64>,
    pub document_type: Option<i64>,
    pub storage_path: Option<i64>,
    pub tags: Vec<i64>,
}

/// Status messages like upstream's `status_callback` texts.
pub type Notify<'a> = &'a mut dyn FnMut(&str);

fn le_i32(v: i64) -> Result<[u8; 4], ClassifierError> {
    i32::try_from(v)
        .map(i32::to_le_bytes)
        .map_err(|_| ClassifierError::LabelOutOfRange(v))
}

/// upstream train(): sha256 over every label as 4-byte little-endian signed int, per document in
/// pk order: document type, correspondent, each AUTO tag, storage path (-1 for none).
pub fn training_hash(docs: &[TrainingDoc]) -> Result<[u8; 32], ClassifierError> {
    let mut h = Sha256::new();
    for d in docs {
        h.update(le_i32(d.document_type.unwrap_or(-1))?);
        h.update(le_i32(d.correspondent.unwrap_or(-1))?);
        for &t in &d.tags {
            h.update(le_i32(t)?);
        }
        h.update(le_i32(d.storage_path.unwrap_or(-1))?);
    }
    Ok(h.finalize().into())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// sklearn `compute_sample_weight("balanced", y)`: n_samples / (n_classes * count(class)).
fn balanced_weights(class_of_row: &[usize], n_classes: usize) -> Vec<f32> {
    let mut counts = vec![0usize; n_classes];
    for &c in class_of_row {
        counts[c] += 1;
    }
    let n = class_of_row.len() as f64;
    let present = counts.iter().filter(|&&c| c > 0).count() as f64;
    class_of_row
        .iter()
        .map(|&c| (n / (present * counts[c] as f64)) as f32)
        .collect()
}

fn fit_single(x: &Csr, labels: &[i64], balanced: bool, opts: &TrainOptions) -> SingleLabel {
    let mut classes: Vec<i64> = labels.to_vec();
    classes.sort_unstable();
    classes.dedup();
    let cls: Vec<usize> = labels
        .iter()
        .map(|l| classes.binary_search(l).unwrap_or(0))
        .collect();
    let k = classes.len();
    let sw = if balanced {
        Some(balanced_weights(&cls, k))
    } else {
        None
    };
    let est = match opts.kind {
        ModelKind::NaiveBayes => {
            if k == 1 {
                Estimator::Constant
            } else {
                Estimator::NaiveBayes {
                    models: vec![Nb::fit(x, &cls, k, sw.as_deref(), 1.0)],
                }
            }
        }
        ModelKind::Mlp | ModelKind::Linear => {
            let mut p = opts.mlp.clone();
            if opts.kind == ModelKind::Linear {
                p.hidden.clear();
            }
            // sklearn LabelBinarizer: 1 class -> one zero column, 2 classes -> one 0/1 column,
            // more -> one-hot with softmax output
            let (n_out, out, y): (usize, OutAct, Vec<f32>) = if k <= 2 {
                (
                    1,
                    OutAct::Logistic,
                    cls.iter()
                        .map(|&c| if k == 2 && c == 1 { 1.0 } else { 0.0 })
                        .collect(),
                )
            } else {
                let mut y = vec![0f32; cls.len() * k];
                for (i, &c) in cls.iter().enumerate() {
                    y[i * k + c] = 1.0;
                }
                (k, OutAct::Softmax, y)
            };
            Estimator::Mlp(Mlp::fit(x, &y, n_out, out, sw.as_deref(), &p))
        }
    };
    SingleLabel { classes, est }
}

/// Join a fit thread; a panic inside it is re-raised on the caller (like a sequential fit).
fn join<T>(h: std::thread::ScopedJoinHandle<'_, T>) -> T {
    h.join().unwrap_or_else(|e| std::panic::resume_unwind(e))
}

/// multilabel indicator (n x k) -> estimator
fn fit_multilabel(x: &Csr, y: &[f32], k: usize, opts: &TrainOptions) -> Estimator {
    match opts.kind {
        ModelKind::NaiveBayes => {
            let n = x.n_rows();
            let models = (0..k)
                .map(|j| {
                    let cls: Vec<usize> = (0..n)
                        .map(|i| if y[i * k + j] > 0.5 { 1 } else { 0 })
                        .collect();
                    Nb::fit(x, &cls, 2, None, 1.0)
                })
                .collect();
            Estimator::NaiveBayes { models }
        }
        ModelKind::Mlp | ModelKind::Linear => {
            let mut p = opts.mlp.clone();
            if opts.kind == ModelKind::Linear {
                p.hidden.clear();
            }
            Estimator::Mlp(Mlp::fit(x, y, k, OutAct::Logistic, None, &p))
        }
    }
}

impl SingleLabel {
    /// sklearn `predict_proba(X)[0]` in class order; for a single-class MLP only column 0
    /// (= 1 - p), like upstream `_predict_with_threshold`.
    pub fn predict_proba(&self, row: &[(u32, f32)]) -> Vec<f64> {
        match &self.est {
            Estimator::Constant => vec![1.0],
            Estimator::NaiveBayes { models } => models
                .first()
                .map_or_else(|| vec![1.0], |m| m.predict_proba(row)),
            Estimator::Mlp(m) => {
                let o = m.output(row);
                if m.out == OutAct::Logistic {
                    let p = o.first().copied().unwrap_or(0.0) as f64;
                    if self.classes.len() == 1 {
                        vec![1.0 - p]
                    } else {
                        vec![1.0 - p, p]
                    }
                } else {
                    o.into_iter().map(f64::from).collect()
                }
            }
        }
    }

    /// upstream `_predict_with_threshold`: None for class -1 or best probability < threshold
    /// (threshold 0.0 disables the check; ties -> first class like numpy argmax).
    pub fn predict(&self, row: &[(u32, f32)], threshold: f64) -> Option<i64> {
        let probas = self.predict_proba(row);
        let mut best = 0usize;
        for (i, &p) in probas.iter().enumerate() {
            if p > probas[best] {
                best = i;
            }
        }
        let class = *self.classes.get(best)?;
        if class == -1 {
            return None;
        }
        if threshold > 0.0 && probas.get(best).copied().unwrap_or(0.0) < threshold {
            return None;
        }
        Some(class)
    }
}

impl TagModel {
    /// Per-output probabilities (multilabel: per tag; binary: P(classes[1])).
    fn outputs(&self, row: &[(u32, f32)]) -> Vec<f64> {
        match &self.est {
            Estimator::Constant => vec![],
            Estimator::NaiveBayes { models } => models
                .iter()
                .map(|m| m.predict_proba(row).get(1).copied().unwrap_or(0.0))
                .collect(),
            Estimator::Mlp(m) => m.output(row).into_iter().map(f64::from).collect(),
        }
    }

    /// upstream `predict_tags`: `predict()` (> 0.5), inverse-binarized.
    pub fn predict(&self, row: &[(u32, f32)]) -> Vec<i64> {
        match &self.binarizer {
            TagBinarizer::MultiLabel { classes } => {
                let o = self.outputs(row);
                classes
                    .iter()
                    .zip(o)
                    .filter(|(_, p)| *p > 0.5)
                    .map(|(c, _)| *c)
                    .collect()
            }
            TagBinarizer::Label { classes } => {
                if classes.len() == 1 {
                    // single class: always predicted (sklearn single-class inverse_transform)
                    return if classes[0] != -1 {
                        vec![classes[0]]
                    } else {
                        vec![]
                    };
                }
                let p = self.outputs(row).first().copied().unwrap_or(0.0);
                let c = if p > 0.5 { classes[1] } else { classes[0] };
                if c != -1 { vec![c] } else { vec![] }
            }
        }
    }
}

impl DocumentClassifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// upstream `train()`. `docs` as described at [`TrainingDoc`] (sorted by pk here);
    /// `content` is called with chunks of up to 1000 pks (in order) and must return one entry
    /// per pk (`None` = deleted meanwhile → empty content, like upstream).
    /// Returns `Ok(false)` if nothing changed since the last training (no retrain).
    pub fn train(
        &mut self,
        mut docs: Vec<TrainingDoc>,
        language: Option<Language>,
        mut content: impl FnMut(&[i64]) -> Vec<Option<String>>,
        opts: &TrainOptions,
        notify: Option<Notify<'_>>,
    ) -> Result<bool, ClassifierError> {
        let mut noop = |_: &str| {};
        let notify: Notify<'_> = match notify {
            Some(n) => n,
            None => &mut noop,
        };
        docs.sort_by_key(|d| d.pk);
        if docs.is_empty() {
            return Err(ClassifierError::NoTrainingData);
        }
        notify(&format!(
            "Gathering data from {} document(s)...",
            docs.len()
        ));
        let hash = training_hash(&docs)?;
        let hash_hex = hex(&hash);
        let latest = docs.iter().map(|d| d.modified_us).max().unwrap_or(i64::MIN);
        if self.last_doc_change_us.is_some_and(|t| t >= latest)
            && self.last_auto_type_hash.as_deref() == Some(&hash_hex)
        {
            return Ok(false);
        }

        let mut tag_set: Vec<i64> = docs.iter().flat_map(|d| d.tags.iter().copied()).collect();
        tag_set.sort_unstable();
        tag_set.dedup();
        let num_tags = tag_set.len();
        let count = |f: &dyn Fn(&TrainingDoc) -> Option<i64>| {
            let mut v: Vec<i64> = docs.iter().filter_map(f).collect();
            v.sort_unstable();
            v.dedup();
            v.len()
        };
        let num_correspondents = count(&|d| d.correspondent);
        let num_document_types = count(&|d| d.document_type);
        let num_storage_paths = count(&|d| d.storage_path);

        // Step 2: vectorize (content fetched in chunks, preprocessed while streaming)
        notify("Vectorizing document content...");
        let pks: Vec<i64> = docs.iter().map(|d| d.pk).collect();
        let mut vectorizer = CountVectorizer::default();
        let mut failed: Option<ClassifierError> = None;
        let stream = pks.chunks(CONTENT_CHUNK_SIZE).flat_map(|chunk| {
            let mut got = content(chunk);
            if got.len() != chunk.len() {
                failed = Some(ClassifierError::ContentMismatch);
                got.resize(chunk.len(), None);
            }
            got.into_iter()
                .map(|c| preprocess_content(c.as_deref().unwrap_or(""), language))
                .collect::<Vec<_>>()
        });
        let x = vectorizer.fit_transform(stream)?;
        if let Some(e) = failed {
            return Err(e);
        }

        // Step 3: train. The four estimators are independent (each its own seeded RNG, read-only
        // `x`), so they are fitted in parallel threads: identical models, wall time = slowest fit
        // instead of the sum (results/bench/worker/NOTES.md, T5). Status messages keep upstream's
        // order and text; they are emitted before the fits start.
        enum TagPlan {
            Constant(Vec<i64>),
            Single(Vec<i64>, Vec<i64>),
            Multi(Vec<f32>),
        }
        let tag_plan = if num_tags > 0 {
            notify(&format!("Training tags classifier ({num_tags} tag(s))..."));
            Some(if num_tags == 1 {
                let labels: Vec<i64> = docs
                    .iter()
                    .map(|d| if d.tags.len() == 1 { d.tags[0] } else { -1 })
                    .collect();
                let mut classes = labels.clone();
                classes.sort_unstable();
                classes.dedup();
                if classes.len() == 1 {
                    TagPlan::Constant(classes)
                } else {
                    TagPlan::Single(classes, labels)
                }
            } else {
                let k = num_tags;
                let mut y = vec![0f32; docs.len() * k];
                for (i, d) in docs.iter().enumerate() {
                    for t in &d.tags {
                        if let Ok(j) = tag_set.binary_search(t) {
                            y[i * k + j] = 1.0;
                        }
                    }
                }
                TagPlan::Multi(y)
            })
        } else {
            None
        };
        let mut single_labels = |n: usize, msg: String, f: &dyn Fn(&TrainingDoc) -> Option<i64>| {
            if n == 0 {
                return None;
            }
            notify(&msg);
            Some(docs.iter().map(|d| f(d).unwrap_or(-1)).collect::<Vec<i64>>())
        };
        let corr_labels = single_labels(
            num_correspondents,
            format!("Training correspondent classifier ({num_correspondents} correspondent(s))..."),
            &|d| d.correspondent,
        );
        let type_labels = single_labels(
            num_document_types,
            format!("Training document type classifier ({num_document_types} type(s))..."),
            &|d| d.document_type,
        );
        let path_labels = single_labels(
            num_storage_paths,
            format!("Training storage path classifier ({num_storage_paths} path(s))..."),
            &|d| d.storage_path,
        );
        let x = &x;
        let (tags, correspondent, document_type, storage_path) = std::thread::scope(|s| {
            let tags = s.spawn(move || {
                tag_plan.map(|plan| match plan {
                    TagPlan::Constant(classes) => TagModel {
                        binarizer: TagBinarizer::Label { classes },
                        est: Estimator::Constant,
                    },
                    TagPlan::Single(classes, labels) => TagModel {
                        binarizer: TagBinarizer::Label { classes },
                        est: fit_single(x, &labels, false, opts).est,
                    },
                    TagPlan::Multi(y) => TagModel {
                        binarizer: TagBinarizer::MultiLabel {
                            classes: tag_set.clone(),
                        },
                        est: fit_multilabel(x, &y, num_tags, opts),
                    },
                })
            });
            let fit = |labels: Option<Vec<i64>>| {
                s.spawn(move || labels.map(|l| fit_single(x, &l, true, opts)))
            };
            let (c, t, p) = (fit(corr_labels), fit(type_labels), fit(path_labels));
            (join(tags), join(c), join(t), join(p))
        });

        self.format_version = FORMAT_VERSION;
        self.kind = opts.kind;
        self.language = language;
        self.vectorizer = Some(vectorizer);
        self.tags = tags;
        self.correspondent = correspondent;
        self.document_type = document_type;
        self.storage_path = storage_path;
        self.last_doc_change_us = Some(latest);
        self.last_auto_type_hash = Some(hash_hex);
        Ok(true)
    }

    /// upstream `_vectorize` (without the read-cache): preprocess + transform.
    pub fn vectorize(&self, content: &str) -> Option<Vec<(u32, f32)>> {
        let v = self.vectorizer.as_ref()?;
        Some(v.transform_one(&preprocess_content(content, self.language)))
    }

    pub fn predict_correspondent(&self, content: &str, threshold: f64) -> Option<i64> {
        let m = self.correspondent.as_ref()?;
        m.predict(&self.vectorize(content)?, threshold)
    }

    pub fn predict_document_type(&self, content: &str, threshold: f64) -> Option<i64> {
        let m = self.document_type.as_ref()?;
        m.predict(&self.vectorize(content)?, threshold)
    }

    pub fn predict_storage_path(&self, content: &str, threshold: f64) -> Option<i64> {
        let m = self.storage_path.as_ref()?;
        m.predict(&self.vectorize(content)?, threshold)
    }

    pub fn predict_tags(&self, content: &str) -> Vec<i64> {
        match (&self.tags, self.vectorize(content)) {
            (Some(m), Some(row)) => m.predict(&row),
            _ => Vec::new(),
        }
    }

    /// All four predictions with a single preprocessing/vectorization pass.
    pub fn predict_all(&self, content: &str, threshold: f64) -> Prediction {
        let Some(row) = self.vectorize(content) else {
            return Prediction::default();
        };
        Prediction {
            correspondent: self
                .correspondent
                .as_ref()
                .and_then(|m| m.predict(&row, threshold)),
            document_type: self
                .document_type
                .as_ref()
                .and_then(|m| m.predict(&row, threshold)),
            storage_path: self
                .storage_path
                .as_ref()
                .and_then(|m| m.predict(&row, threshold)),
            tags: self
                .tags
                .as_ref()
                .map(|m| m.predict(&row))
                .unwrap_or_default(),
        }
    }

    fn mac(key: &[u8]) -> Hmac<Sha256> {
        // HMAC accepts keys of any length
        <Hmac<Sha256> as Mac>::new_from_slice(key).unwrap_or_else(|_| unreachable!())
    }

    /// Serialized file content: HMAC-SHA256(key, payload) || payload, payload = MAGIC || JSON.
    pub fn to_bytes(&self, key: &[u8]) -> Result<Vec<u8>, ClassifierError> {
        let mut payload = MAGIC.to_vec();
        serde_json::to_writer(&mut payload, self)
            .map_err(|e| ClassifierError::Corrupt(e.to_string()))?;
        let mut mac = Self::mac(key);
        mac.update(&payload);
        let mut out = mac.finalize().into_bytes().to_vec();
        out.extend_from_slice(&payload);
        Ok(out)
    }

    pub fn from_bytes(raw: &[u8], key: &[u8]) -> Result<Self, ClassifierError> {
        if raw.len() <= HMAC_SIZE {
            return Err(ClassifierError::Corrupt("file too short".into()));
        }
        let (sig, payload) = raw.split_at(HMAC_SIZE);
        let mut mac = Self::mac(key);
        mac.update(payload);
        mac.verify_slice(sig)
            .map_err(|_| ClassifierError::Corrupt("signature mismatch".into()))?;
        let json = payload
            .strip_prefix(MAGIC)
            .ok_or_else(|| ClassifierError::Corrupt("bad magic".into()))?;
        // read the version first so a newer/older layout is reported as incompatible
        #[derive(Deserialize)]
        struct V {
            format_version: u32,
        }
        let v: V =
            serde_json::from_slice(json).map_err(|e| ClassifierError::Corrupt(e.to_string()))?;
        if v.format_version != FORMAT_VERSION {
            return Err(ClassifierError::IncompatibleVersion(v.format_version));
        }
        let mut c: DocumentClassifier =
            serde_json::from_slice(json).map_err(|e| ClassifierError::Corrupt(e.to_string()))?;
        if let Some(v) = c.vectorizer.as_mut() {
            v.rebuild_index();
        }
        Ok(c)
    }

    /// upstream `save()`: write `<file>.part`, then rename over the target; partial file removed
    /// on error.
    pub fn save(&self, path: &Path, key: &[u8]) -> Result<(), ClassifierError> {
        let bytes = self.to_bytes(key)?;
        let mut tmp: PathBuf = path.to_path_buf();
        let name = format!(
            "{}.part",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("model")
        );
        tmp.set_file_name(name);
        let res = std::fs::write(&tmp, &bytes).and_then(|_| std::fs::rename(&tmp, path));
        if res.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        res.map_err(ClassifierError::Io)
    }

    pub fn load(path: &Path, key: &[u8]) -> Result<Self, ClassifierError> {
        let raw = std::fs::read(path).map_err(ClassifierError::Io)?;
        Self::from_bytes(&raw, key)
    }
}

/// Classifier bound to `settings.CLASSIFIER_MATCH_THRESHOLD`, with the method signatures of
/// `pl_matching::matching::Classifier` (the worker implements that trait by delegating here).
#[derive(Debug, Clone, Copy)]
pub struct Predictor<'a> {
    pub classifier: &'a DocumentClassifier,
    pub threshold: f64,
}

impl Predictor<'_> {
    pub fn predict_correspondent(&self, content: &str) -> Option<i64> {
        self.classifier
            .predict_correspondent(content, self.threshold)
    }
    pub fn predict_document_type(&self, content: &str) -> Option<i64> {
        self.classifier
            .predict_document_type(content, self.threshold)
    }
    pub fn predict_tags(&self, content: &str) -> Vec<i64> {
        self.classifier.predict_tags(content)
    }
    pub fn predict_storage_path(&self, content: &str) -> Option<i64> {
        self.classifier
            .predict_storage_path(content, self.threshold)
    }
}

/// upstream `load_classifier(raise_exception=...)`: `Ok(None)` if the file does not exist; on an
/// incompatible version or a corrupt file the file is deleted (so it gets re-trained) and
/// `Ok(None)` returned, or the error if `raise_exception`. IO errors keep the file.
pub fn load_classifier(
    path: &Path,
    key: &[u8],
    raise_exception: bool,
) -> Result<Option<DocumentClassifier>, ClassifierError> {
    if !path.is_file() {
        return Ok(None);
    }
    match DocumentClassifier::load(path, key) {
        Ok(c) => Ok(Some(c)),
        Err(e) => {
            if matches!(
                e,
                ClassifierError::IncompatibleVersion(_) | ClassifierError::Corrupt(_)
            ) {
                let _ = std::fs::remove_file(path);
            }
            if raise_exception { Err(e) } else { Ok(None) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(pk: i64, c: Option<i64>, t: Option<i64>, tags: &[i64]) -> TrainingDoc {
        TrainingDoc {
            pk,
            modified_us: pk * 1000,
            correspondent: c,
            document_type: t,
            storage_path: None,
            tags: tags.to_vec(),
        }
    }

    fn contents(pks: &[i64]) -> Vec<Option<String>> {
        pks.iter()
            .map(|pk| {
                Some(match pk % 3 {
                    0 => "electricity bill kilowatt meter reading tariff".to_string(),
                    1 => "salary payslip employer income tax pension".to_string(),
                    _ => "insurance policy premium coverage claim".to_string(),
                })
            })
            .collect()
    }

    fn corpus() -> Vec<TrainingDoc> {
        (1..=60)
            .map(|pk| {
                let k = pk % 3;
                doc(
                    pk,
                    Some(10 + k),
                    if k == 0 { None } else { Some(20 + k) },
                    if k == 1 { &[5, 6] } else { &[6] },
                )
            })
            .collect()
    }

    /// The estimators are fitted in parallel threads (T5): the model must still be identical
    /// run to run, and the status messages keep upstream's order.
    #[test]
    fn parallel_fits_are_deterministic_and_messages_ordered() {
        let mut bytes = Vec::new();
        let mut msgs = Vec::new();
        for _ in 0..2 {
            let mut c = DocumentClassifier::new();
            let mut seen: Vec<String> = Vec::new();
            let mut note = |m: &str| seen.push(m.to_string());
            assert!(c
                .train(corpus(), Some(Language::English), contents, &TrainOptions::default(), Some(&mut note))
                .unwrap());
            bytes.push(c.to_bytes(b"k").unwrap());
            msgs.push(seen);
        }
        assert_eq!(bytes[0], bytes[1]);
        assert_eq!(msgs[0], msgs[1]);
        let order: Vec<&str> = msgs[0].iter().map(|m| m.split(' ').nth(1).unwrap_or("")).collect();
        assert_eq!(order, ["data", "document", "tags", "correspondent", "document"], "{:?}", msgs[0]);
    }

    #[test]
    fn train_predict_save_load_and_no_retrain() {
        for kind in [ModelKind::Mlp, ModelKind::Linear, ModelKind::NaiveBayes] {
            let mut c = DocumentClassifier::new();
            let opts = TrainOptions {
                kind,
                ..Default::default()
            };
            assert!(
                c.train(corpus(), Some(Language::English), contents, &opts, None)
                    .unwrap()
            );
            let p = c.predict_all("payslip salary income", 0.3);
            assert_eq!(p.correspondent, Some(11), "{kind:?}");
            assert_eq!(p.document_type, Some(21), "{kind:?}");
            assert_eq!(p.tags, vec![5, 6], "{kind:?}");
            assert_eq!(
                c.predict_document_type("electricity meter tariff", 0.3),
                None,
                "{kind:?}"
            );
            assert_eq!(c.predict_storage_path("x", 0.3), None);
            // unchanged -> no retrain
            assert!(
                !c.train(corpus(), Some(Language::English), contents, &opts, None)
                    .unwrap()
            );
            let mut changed = corpus();
            changed[0].modified_us += 10_000_000;
            assert!(
                c.train(changed, Some(Language::English), contents, &opts, None)
                    .unwrap()
            );

            let dir = std::env::temp_dir().join(format!(
                "pl-classifier-test-{kind:?}-{}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let f = dir.join("model.bin");
            c.save(&f, b"secret").unwrap();
            let l = load_classifier(&f, b"secret", true).unwrap().unwrap();
            assert_eq!(l.predict_all("payslip salary income", 0.3), p);
            // wrong key -> corrupt -> deleted
            assert!(matches!(
                load_classifier(&f, b"other", true),
                Err(ClassifierError::Corrupt(_))
            ));
            assert!(!f.exists());
            assert!(load_classifier(&f, b"secret", false).unwrap().is_none());
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn incompatible_version_and_truncation() {
        let mut c = DocumentClassifier::new();
        c.format_version = FORMAT_VERSION + 1;
        let b = c.to_bytes(b"k").unwrap();
        assert!(matches!(
            DocumentClassifier::from_bytes(&b, b"k"),
            Err(ClassifierError::IncompatibleVersion(_))
        ));
        assert!(matches!(
            DocumentClassifier::from_bytes(&b[..20], b"k"),
            Err(ClassifierError::Corrupt(_))
        ));
        let mut t = b.clone();
        let last = t.len() - 1;
        t[last] ^= 1;
        assert!(matches!(
            DocumentClassifier::from_bytes(&t, b"k"),
            Err(ClassifierError::Corrupt(_))
        ));
    }

    #[test]
    fn no_training_data() {
        let mut c = DocumentClassifier::new();
        assert!(matches!(
            c.train(vec![], None, contents, &TrainOptions::default(), None),
            Err(ClassifierError::NoTrainingData)
        ));
    }

    #[test]
    fn single_class_and_single_tag_cases() {
        // upstream test_one_correspondent_predict / test_one_tag_predict(_unassigned)
        for kind in [ModelKind::Mlp, ModelKind::NaiveBayes] {
            let opts = TrainOptions {
                kind,
                ..Default::default()
            };
            let docs: Vec<TrainingDoc> = (1..=4).map(|pk| doc(pk, Some(7), None, &[3])).collect();
            let mut c = DocumentClassifier::new();
            c.train(docs, None, contents, &opts, None).unwrap();
            assert_eq!(
                c.predict_correspondent("anything at all", 0.3),
                Some(7),
                "{kind:?}"
            );
            assert_eq!(c.predict_tags("anything at all"), vec![3], "{kind:?}");
            assert!(c.document_type.is_none());
            // one tag on some documents: binary
            let docs: Vec<TrainingDoc> = (1..=30)
                .map(|pk| doc(pk, None, None, if pk % 3 == 1 { &[3] } else { &[] }))
                .collect();
            let mut c = DocumentClassifier::new();
            c.train(docs, None, contents, &opts, None).unwrap();
            assert!(matches!(
                c.tags.as_ref().unwrap().binarizer,
                TagBinarizer::Label { .. }
            ));
            assert_eq!(
                c.predict_tags("salary payslip employer"),
                vec![3],
                "{kind:?}"
            );
            assert_eq!(
                c.predict_tags("insurance policy premium"),
                Vec::<i64>::new(),
                "{kind:?}"
            );
            assert!(c.correspondent.is_none());
        }
    }

    #[test]
    fn threshold_semantics() {
        // upstream test_predict_with_threshold
        let m = SingleLabel {
            classes: vec![-1, 3],
            est: Estimator::Constant,
        };
        assert_eq!(m.predict(&[], 0.0), None); // constant -> class index 0 = -1
        let nb = |p0: f32| Nb {
            n_features: 1,
            n_classes: 2,
            class_log_prior: vec![p0.ln(), (1.0 - p0).ln()],
            feature_log_prob: vec![0.0, 0.0],
        };
        let mk = |p0| SingleLabel {
            classes: vec![-1, 3],
            est: Estimator::NaiveBayes {
                models: vec![nb(p0)],
            },
        };
        assert_eq!(mk(0.1).predict(&[], 0.6), Some(3));
        assert_eq!(mk(0.45).predict(&[], 0.6), None);
        assert_eq!(mk(0.99).predict(&[], 0.0), None);
        assert_eq!(mk(0.45).predict(&[], 0.0), Some(3));
    }

    #[test]
    fn hash_matches_upstream_layout() {
        let d = vec![TrainingDoc {
            pk: 1,
            modified_us: 0,
            correspondent: Some(2),
            document_type: None,
            storage_path: Some(1),
            tags: vec![4, 5],
        }];
        let mut h = Sha256::new();
        for v in [-1i32, 2, 4, 5, 1] {
            h.update(v.to_le_bytes());
        }
        let expect: [u8; 32] = h.finalize().into();
        assert_eq!(training_hash(&d).unwrap(), expect);
        let big = vec![TrainingDoc {
            pk: 1,
            modified_us: 0,
            correspondent: Some(1 << 40),
            document_type: None,
            storage_path: None,
            tags: vec![],
        }];
        assert!(matches!(
            training_hash(&big),
            Err(ClassifierError::LabelOutOfRange(_))
        ));
    }
}
