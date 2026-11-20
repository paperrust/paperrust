//! Parity/quality tests against upstream (scikit-learn 1.9.1, tantivy-py 0.26.2), reference
//! data from tests/parity/pl-classifier/gen_reference.py (corpus.json, reference.json).
//!
//! - preprocessing: must be exactly equal (upstream samples, edge cases, every corpus document
//!   for every classifier language and for no language)
//! - CountVectorizer vocabulary / counts and the retrain hash: exactly equal
//! - quality per target: Rust MLP within QUALITY_TOLERANCE of sklearn's (absolute)

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use pl_classifier::metrics::{self, SingleMetrics, TagMetrics};
use pl_classifier::vectorizer::CountVectorizer;
use pl_classifier::{
    DocumentClassifier, Language, ModelKind, TrainOptions, TrainingDoc, preprocess_content,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Allowed absolute shortfall of a Rust metric against the sklearn one.
const QUALITY_TOLERANCE: f64 = 0.03;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load(name: &str) -> Value {
    let p = root().join("tests/parity/pl-classifier").join(name);
    serde_json::from_slice(&std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
        .unwrap()
}

fn lang_of(s: &str) -> Option<Language> {
    if s == "None" {
        None
    } else {
        Some(Language::from_name(s).unwrap_or_else(|| panic!("language {s}")))
    }
}

fn sha16(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn upstream_sample_files() {
    // upstream test_classifier.py::TestPreprocessContent (Python read_text(): universal newlines)
    let dir = root().join("upstream/src/documents/tests/samples");
    let read = |n: &str| {
        std::fs::read_to_string(dir.join(n))
            .unwrap()
            .replace("\r\n", "\n")
    };
    let content = read("content.txt");
    assert_eq!(
        preprocess_content(&content, Some(Language::English)),
        read("preprocessed_content_advanced.txt").trim_end()
    );
    assert_eq!(
        preprocess_content(&content, None),
        read("preprocessed_content.txt").trim_end()
    );
}

#[test]
fn preprocessing_edge_cases_exact() {
    let r = load("reference.json");
    let mut bad = Vec::new();
    for case in r["preprocess"]["edge_cases"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        for (lang, expect) in case["out"].as_object().unwrap() {
            let got = preprocess_content(text, lang_of(lang));
            if got != expect.as_str().unwrap() {
                bad.push(format!("{lang} {text:?}: got {got:?} expected {expect}"));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn preprocessing_corpus_exact_all_languages() {
    let r = load("reference.json");
    let c = load("corpus.json");
    let docs = c["documents"].as_array().unwrap();
    let mut bad = Vec::new();
    for (lang, hashes) in r["preprocess"]["doc_sha16"].as_object().unwrap() {
        let l = lang_of(lang);
        for (d, h) in docs.iter().zip(hashes.as_array().unwrap()) {
            let out = preprocess_content(d["content"].as_str().unwrap(), l);
            if sha16(&out) != h.as_str().unwrap() {
                bad.push(format!("{lang} pk {}", d["pk"]));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "{} mismatches: {:?}",
        bad.len(),
        &bad[..bad.len().min(20)]
    );
}

struct Corpus {
    train: Vec<TrainingDoc>,
    contents: HashMap<i64, String>,
    test: Vec<(String, Truth)>,
}

#[derive(Clone)]
struct Truth {
    correspondent: Option<i64>,
    document_type: Option<i64>,
    storage_path: Option<i64>,
    tags: Vec<i64>,
}

/// Builds training input exactly like upstream train(): inbox-tagged documents excluded,
/// labels only for MATCH_AUTO objects, tags = AUTO tags sorted by pk.
fn corpus() -> Corpus {
    let c = load("corpus.json");
    let ids = |k: &str, f: &str| -> Vec<i64> {
        c["objects"][k]
            .as_array()
            .unwrap()
            .iter()
            .filter(|o| o[f].as_bool().unwrap())
            .map(|o| o["id"].as_i64().unwrap())
            .collect()
    };
    let (auto_c, auto_t, auto_s, auto_tag, inbox) = (
        ids("correspondents", "auto"),
        ids("document_types", "auto"),
        ids("storage_paths", "auto"),
        ids("tags", "auto"),
        ids("tags", "inbox"),
    );
    let pick = |v: &Value, auto: &[i64]| v.as_i64().filter(|x| auto.contains(x));
    let mut out = Corpus {
        train: vec![],
        contents: HashMap::new(),
        test: vec![],
    };
    for d in c["documents"].as_array().unwrap() {
        let pk = d["pk"].as_i64().unwrap();
        let tags: Vec<i64> = d["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_i64().unwrap())
            .collect();
        let mut auto_tags: Vec<i64> = tags
            .iter()
            .copied()
            .filter(|t| auto_tag.contains(t))
            .collect();
        auto_tags.sort_unstable();
        let truth = Truth {
            correspondent: pick(&d["correspondent"], &auto_c),
            document_type: pick(&d["document_type"], &auto_t),
            storage_path: pick(&d["storage_path"], &auto_s),
            tags: auto_tags,
        };
        let content = d["content"].as_str().unwrap().to_string();
        if d["split"] == "train" {
            if tags.iter().any(|t| inbox.contains(t)) {
                continue;
            }
            out.train.push(TrainingDoc {
                pk,
                modified_us: d["modified_us"].as_i64().unwrap(),
                correspondent: truth.correspondent,
                document_type: truth.document_type,
                storage_path: truth.storage_path,
                tags: truth.tags.clone(),
            });
            out.contents.insert(pk, content);
        } else {
            out.test.push((content, truth));
        }
    }
    out
}

#[test]
fn vectorizer_and_hash_exact() {
    let r = load("reference.json");
    let c = corpus();
    for (lang, q) in r["quality"].as_object().unwrap() {
        let l = lang_of(lang);
        let mut v = CountVectorizer::default();
        let x = v
            .fit_transform(
                c.train
                    .iter()
                    .map(|d| preprocess_content(&c.contents[&d.pk], l)),
            )
            .unwrap();
        let vocab: Vec<&str> = q["vocabulary"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap())
            .collect();
        assert_eq!(v.vocabulary, vocab, "vocabulary {lang}");
        assert_eq!(x.n_rows(), q["n_train_docs"].as_u64().unwrap() as usize);
        for i in 0..x.n_rows() {
            let (cols, vals) = x.row(i);
            assert_eq!(
                cols.len() as u64,
                q["row_nnz"][i].as_u64().unwrap(),
                "nnz {lang} row {i}"
            );
            assert_eq!(
                vals.iter().sum::<f32>() as u64,
                q["row_sum"][i].as_u64().unwrap(),
                "sum {lang} row {i}"
            );
        }
        let h = pl_classifier::training_hash(&c.train).unwrap();
        let hex: String = h.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, q["train_hash_hex"].as_str().unwrap());
        assert_eq!(
            c.train.iter().map(|d| d.modified_us).max().unwrap(),
            q["last_doc_change_us"].as_i64().unwrap()
        );
    }
}

/// correspondent, document type, storage path, tags
type AllMetrics = (SingleMetrics, SingleMetrics, SingleMetrics, TagMetrics);

fn sk_metrics(v: &Value) -> AllMetrics {
    let s = |k: &str| SingleMetrics {
        accuracy: v[k]["accuracy"].as_f64().unwrap(),
        macro_f1: v[k]["macro_f1"].as_f64().unwrap(),
    };
    (
        s("correspondent"),
        s("document_type"),
        s("storage_path"),
        TagMetrics {
            micro_f1: v["tags"]["micro_f1"].as_f64().unwrap(),
            macro_f1: v["tags"]["macro_f1"].as_f64().unwrap(),
            exact_match: v["tags"]["exact_match"].as_f64().unwrap(),
        },
    )
}

#[test]
fn quality_against_sklearn() {
    // default model (MLP like upstream), default language (english); the other configuration
    // and model kinds run in the ignored `model_comparison` (slow in debug builds)
    run_quality(
        &["english"],
        &[("Mlp", ModelKind::Mlp, TrainOptions::default())],
    );
}

#[test]
#[ignore = "slow in debug builds (~12 min); run with --ignored to reproduce the model decision table"]
fn model_comparison() {
    use pl_classifier::mlp::MlpParams;
    let tuned = MlpParams {
        learning_rate_init: 0.01,
        tol: 1e-4,
        ..Default::default()
    };
    run_quality(
        &["english", "None"],
        &[
            ("Mlp", ModelKind::Mlp, TrainOptions::default()),
            (
                "Mlp seed 1",
                ModelKind::Mlp,
                TrainOptions {
                    mlp: MlpParams {
                        seed: 1,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ),
            (
                "Mlp seed 2",
                ModelKind::Mlp,
                TrainOptions {
                    mlp: MlpParams {
                        seed: 2,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ),
            (
                "Linear",
                ModelKind::Linear,
                TrainOptions {
                    kind: ModelKind::Linear,
                    ..Default::default()
                },
            ),
            (
                "Linear lr0.01 tol1e-4",
                ModelKind::Linear,
                TrainOptions {
                    kind: ModelKind::Linear,
                    mlp: tuned,
                },
            ),
            (
                "NaiveBayes",
                ModelKind::NaiveBayes,
                TrainOptions {
                    kind: ModelKind::NaiveBayes,
                    ..Default::default()
                },
            ),
        ],
    );
}

fn run_quality(langs: &[&str], variants: &[(&str, ModelKind, TrainOptions)]) {
    let r = load("reference.json");
    let threshold = r["threshold"].as_f64().unwrap();
    let c = corpus();
    let truth: Vec<Truth> = c.test.iter().map(|t| t.1.clone()).collect();
    let tc = |f: &dyn Fn(&Truth) -> Option<i64>| truth.iter().map(f).collect::<Vec<_>>();
    let (t_c, t_t, t_s) = (
        tc(&|t| t.correspondent),
        tc(&|t| t.document_type),
        tc(&|t| t.storage_path),
    );
    let t_tags: Vec<Vec<i64>> = truth.iter().map(|t| t.tags.clone()).collect();
    let mut failures = Vec::new();
    for (lang, q) in r["quality"].as_object().unwrap() {
        if !langs.contains(&lang.as_str()) {
            continue;
        }
        let l = lang_of(lang);
        // the metric code must reproduce the Python numbers from sklearn's predictions
        let sp = q["predictions"].as_array().unwrap();
        let col = |k: &str| sp.iter().map(|p| p[k].as_i64()).collect::<Vec<_>>();
        let sk_tags: Vec<Vec<i64>> = sp
            .iter()
            .map(|p| {
                p["tags"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x.as_i64().unwrap())
                    .collect()
            })
            .collect();
        let sk = sk_metrics(&q["metrics"]);
        assert!(
            (metrics::single(&col("correspondent"), &t_c).accuracy - sk.0.accuracy).abs() < 1e-12
        );
        assert!((metrics::tags(&sk_tags, &t_tags).micro_f1 - sk.3.micro_f1).abs() < 1e-12);
        // quality bar: mean of the sklearn runs (random_state 0 plus the seed-spread runs, if any)
        let mut runs = vec![sk];
        if let (true, Some(o)) = (
            lang == "english",
            r["sklearn_seed_spread_english"].as_object(),
        ) {
            runs.extend(o.values().map(sk_metrics));
        }
        let n = runs.len() as f64;
        let mean = |f: &dyn Fn(&AllMetrics) -> f64| runs.iter().map(f).sum::<f64>() / n;
        let bar = [
            mean(&|m| m.0.accuracy),
            mean(&|m| m.0.macro_f1),
            mean(&|m| m.1.accuracy),
            mean(&|m| m.1.macro_f1),
            mean(&|m| m.2.accuracy),
            mean(&|m| m.2.macro_f1),
            mean(&|m| m.3.micro_f1),
            mean(&|m| m.3.macro_f1),
        ];
        println!(
            "[{lang}] sklearn mean of {} run(s): {:.3?}",
            runs.len(),
            bar
        );
        println!(
            "[{lang}] sklearn MLP: corr acc {:.3} f1 {:.3} | type acc {:.3} f1 {:.3} | path acc {:.3} f1 {:.3} | tags micro {:.3} macro {:.3} exact {:.3} | train {}s, {} ms/doc",
            sk.0.accuracy,
            sk.0.macro_f1,
            sk.1.accuracy,
            sk.1.macro_f1,
            sk.2.accuracy,
            sk.2.macro_f1,
            sk.3.micro_f1,
            sk.3.macro_f1,
            sk.3.exact_match,
            q["train_seconds"],
            q["predict_ms_per_doc"]
        );
        for (name, kind, opts) in variants {
            let mut clf = DocumentClassifier::new();
            let t0 = Instant::now();
            let trained = clf
                .train(
                    c.train.clone(),
                    l,
                    |pks| pks.iter().map(|pk| c.contents.get(pk).cloned()).collect(),
                    opts,
                    None,
                )
                .unwrap();
            assert!(trained);
            let train_s = t0.elapsed().as_secs_f64();
            let t0 = Instant::now();
            let preds: Vec<_> = c
                .test
                .iter()
                .map(|(content, _)| clf.predict_all(content, threshold))
                .collect();
            let pred_ms = t0.elapsed().as_secs_f64() * 1000.0 / preds.len() as f64;
            let p_c: Vec<_> = preds.iter().map(|p| p.correspondent).collect();
            let p_t: Vec<_> = preds.iter().map(|p| p.document_type).collect();
            let p_s: Vec<_> = preds.iter().map(|p| p.storage_path).collect();
            let p_tags: Vec<Vec<i64>> = preds.iter().map(|p| p.tags.clone()).collect();
            let m = (
                metrics::single(&p_c, &t_c),
                metrics::single(&p_t, &t_t),
                metrics::single(&p_s, &t_s),
                metrics::tags(&p_tags, &t_tags),
            );
            let agree = p_c
                .iter()
                .zip(col("correspondent"))
                .filter(|(a, b)| **a == *b)
                .count() as f64
                / p_c.len() as f64;
            let size = clf.to_bytes(b"k").unwrap().len();
            println!(
                "[{lang}] rust {name}: corr acc {:.3} f1 {:.3} | type acc {:.3} f1 {:.3} | path acc {:.3} f1 {:.3} | tags micro {:.3} macro {:.3} exact {:.3} | corr agreement w/ sklearn {:.3} | train {:.2}s (debug), {:.3} ms/doc, model {} KiB",
                m.0.accuracy,
                m.0.macro_f1,
                m.1.accuracy,
                m.1.macro_f1,
                m.2.accuracy,
                m.2.macro_f1,
                m.3.micro_f1,
                m.3.macro_f1,
                m.3.exact_match,
                agree,
                train_s,
                pred_ms,
                size / 1024
            );
            if *kind == ModelKind::Mlp && *name == "Mlp" {
                let names = [
                    "correspondent accuracy",
                    "correspondent macro_f1",
                    "document_type accuracy",
                    "document_type macro_f1",
                    "storage_path accuracy",
                    "storage_path macro_f1",
                    "tags micro_f1",
                    "tags macro_f1",
                ];
                let ours = [
                    m.0.accuracy,
                    m.0.macro_f1,
                    m.1.accuracy,
                    m.1.macro_f1,
                    m.2.accuracy,
                    m.2.macro_f1,
                    m.3.micro_f1,
                    m.3.macro_f1,
                ];
                let pairs = names
                    .iter()
                    .zip(ours)
                    .zip(bar)
                    .map(|((n, a), b)| (*n, a, b));
                for (name, rust, skl) in pairs {
                    if rust < skl - QUALITY_TOLERANCE {
                        failures.push(format!("[{lang}] {name}: rust {rust:.3} < sklearn {skl:.3} - {QUALITY_TOLERANCE}"));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
