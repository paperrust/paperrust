#![allow(clippy::type_complexity, clippy::collapsible_if)]
//! Parity: index built by UNCHANGED upstream (tests/parity/pl-indexer/gen_reference.py) vs.
//! index built by pl-indexer from the same reference DB.
//!
//! Needs the reference PostgreSQL and a directory prepared by gen_reference.py:
//!   PAPERRUST_INDEXER_PARITY_DIR=<tmp>   (contains py_data/index from upstream)
//!   PAPERRUST_INDEXER_DB_URL=postgres://paperless:paperless@127.0.0.1:54329/paperless (default)
//! Run: cargo test -p pl-indexer --test parity -j 1 -- --ignored --nocapture
//!
//! Compared: .index_settings.json, every stored document, every indexed term with its
//! postings (doc id, term frequency, positions), field norms, fast-field values, and
//! through pl-search (read-only): search ids/scores/highlights, autocomplete, more-like-this.
//! Writes <dir>/report.json.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use pl_indexer::{Indexer, IndexerConfig, RebuildOptions};
use pl_search::{SearchBackend, SearchMode, SearchSettings, SearchUser};
use serde_json::{Value, json};
use sqlx::Row;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tantivy::schema::document::Document as _;
use tantivy::schema::{FieldType, IndexRecordOption};
use tantivy::postings::Postings;
use tantivy::{DocId, DocSet, Index, TERMINATED, TantivyDocument};

const LANG: &str = "en";

#[derive(Default, PartialEq, Debug)]
struct Snapshot {
    num_docs: u64,
    stored: BTreeMap<u64, Value>,
    /// (field, term bytes as lossy string + hex) -> [(id, tf, positions)]
    postings: BTreeMap<(String, String), Vec<(u64, u32, Vec<u32>)>>,
    fieldnorms: BTreeMap<(String, u64), u8>,
    fast: BTreeMap<(String, u64), Vec<String>>,
}

fn term_key(bytes: &[u8]) -> String {
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{} [{hex}]", String::from_utf8_lossy(bytes))
}

fn open(dir: &Path) -> Index {
    let index = Index::open_in_dir(dir).expect("open index");
    pl_search::tokenizer::register_tokenizers(&index, Some(LANG));
    index
}

fn snapshot(dir: &Path) -> Snapshot {
    let index = open(dir);
    let schema = index.schema();
    let reader = index.reader().expect("reader");
    let searcher = reader.searcher();
    let mut snap = Snapshot { num_docs: searcher.num_docs(), ..Default::default() };
    for (seg_ord, seg) in searcher.segment_readers().iter().enumerate() {
        let ids = seg.fast_fields().u64("id").expect("id column");
        let doc_id = |d: DocId| ids.first(d).expect("id value");
        let alive = |d: DocId| seg.alive_bitset().is_none_or(|b| b.is_alive(d));
        let store = seg.get_store_reader(0).expect("store");
        for d in 0..seg.max_doc() {
            if !alive(d) {
                continue;
            }
            let doc: TantivyDocument = store.get(d).expect("doc");
            let v: Value = serde_json::from_str(&doc.to_json(&schema)).expect("json");
            snap.stored.insert(doc_id(d), v);
        }
        for (field, entry) in schema.fields() {
            let name = entry.name().to_string();
            let inv = seg.inverted_index(field).expect("inverted index");
            let mut stream = inv.terms().stream().expect("terms");
            while stream.advance() {
                let key = term_key(stream.key());
                let ti = stream.value().clone();
                let mut postings = inv
                    .read_postings_from_terminfo(&ti, IndexRecordOption::WithFreqsAndPositions)
                    .expect("postings");
                let mut list = Vec::new();
                let mut doc = postings.doc();
                while doc != TERMINATED {
                    if alive(doc) {
                        let mut pos = Vec::new();
                        postings.positions(&mut pos);
                        list.push((doc_id(doc), postings.term_freq(), pos));
                    }
                    doc = postings.advance();
                }
                if !list.is_empty() {
                    let e = snap.postings.entry((name.clone(), key)).or_default();
                    e.extend(list);
                    // sorted by document id across all segments (segment layout independent)
                    e.sort();
                }
            }
            if matches!(entry.field_type(), FieldType::Str(_) | FieldType::JsonObject(_))
                && entry.is_indexed()
            {
                if let Ok(norms) = seg.get_fieldnorms_reader(field) {
                    for d in 0..seg.max_doc() {
                        if alive(d) {
                            snap.fieldnorms.insert((name.clone(), doc_id(d)), norms.fieldnorm_id(d));
                        }
                    }
                }
            }
            if entry.is_fast() {
                let ff = seg.fast_fields();
                for d in 0..seg.max_doc() {
                    if !alive(d) {
                        continue;
                    }
                    let vals: Vec<String> = match entry.field_type() {
                        FieldType::U64(_) => match ff.column_opt::<u64>(&name).expect("col") {
                            Some(c) => c.values_for_doc(d).map(|v| v.to_string()).collect(),
                            None => vec![],
                        },
                        FieldType::Date(_) => {
                            match ff.column_opt::<tantivy::DateTime>(&name).expect("col") {
                                Some(c) => c
                                    .values_for_doc(d)
                                    .map(|v| v.into_timestamp_nanos().to_string())
                                    .collect(),
                                None => vec![],
                            }
                        }
                        FieldType::Str(_) => match ff.str(&name).expect("col") {
                            Some(c) => c
                                .term_ords(d)
                                .map(|o| {
                                    let mut s = String::new();
                                    let _ = c.ord_to_str(o, &mut s);
                                    s
                                })
                                .collect(),
                            None => vec![],
                        },
                        _ => vec![],
                    };
                    snap.fast.insert((name.clone(), doc_id(d)), vals);
                }
            }
        }
        let _ = seg_ord;
    }
    snap
}

fn diff_maps<K: Ord + std::fmt::Debug, V: PartialEq + std::fmt::Debug>(
    what: &str,
    a: &BTreeMap<K, V>,
    b: &BTreeMap<K, V>,
    out: &mut Vec<Value>,
) {
    for (k, va) in a {
        match b.get(k) {
            None => out.push(json!({"what": what, "key": format!("{k:?}"), "only_in": "upstream", "upstream": format!("{va:?}")})),
            Some(vb) if vb != va => out.push(json!({"what": what, "key": format!("{k:?}"), "upstream": format!("{va:?}"), "rust": format!("{vb:?}")})),
            _ => {}
        }
    }
    for (k, vb) in b {
        if !a.contains_key(k) {
            out.push(json!({"what": what, "key": format!("{k:?}"), "only_in": "rust", "rust": format!("{vb:?}")}));
        }
    }
}

fn backend(dir: &Path) -> SearchBackend {
    SearchBackend::new(SearchSettings {
        index_dir: dir.to_path_buf(),
        language: Some(LANG.into()),
        fuzzy_threshold: None,
        time_zone: chrono_tz::Europe::Zurich,
    })
}

fn mode_of(s: &str) -> SearchMode {
    match s {
        "text" => SearchMode::Text,
        "title" => SearchMode::Title,
        _ => SearchMode::Query,
    }
}

/// Full search outcome as comparable JSON: ids in order + per id score/highlights.
fn run_search(b: &SearchBackend, case: &Value, user: Option<&SearchUser>) -> Value {
    let q = case["q"].as_str().unwrap_or_default();
    let mode = mode_of(case["mode"].as_str().unwrap_or("query"));
    let sort = case.get("sort").and_then(|s| s.as_str());
    let reverse = case.get("reverse").and_then(|r| r.as_bool()).unwrap_or(false);
    match b.search_ids(q, user, sort, reverse, mode, None, None) {
        Err(e) => json!({"error": e.to_string()}),
        Ok(ids) => {
            let hits = b.highlight_hits(q, &ids, mode, 1).unwrap_or_default();
            let hits: Vec<Value> = hits
                .iter()
                .map(|h| json!([h.id, h.score, h.content_highlight, h.notes_highlight]))
                .collect();
            json!({"ids": ids, "hits": hits})
        }
    }
}

/// `TantivyBackend.SORT_FIELD_MAP`.
fn sort_index_field(sort: &str) -> Option<&'static str> {
    Some(match sort {
        "title" => "title_sort",
        "correspondent__name" => "correspondent_sort",
        "document_type__name" => "type_sort",
        "created" => "created",
        "added" => "added",
        "modified" => "modified",
        "archive_serial_number" => "asn",
        "page_count" => "page_count",
        "num_notes" => "num_notes",
        _ => return None,
    })
}

/// Same result except for the order of hits that tie on the ranking key (the score, or
/// the sort field value). Tie order follows the internal doc order, which upstream
/// itself does not keep stable between two rebuilds (multi-threaded writer + merge).
fn same_up_to_ties(case: &Value, a: &Value, b: &Value, snap: &Snapshot) -> bool {
    let sort_field = case.get("sort").and_then(|s| s.as_str()).and_then(sort_index_field);
    let key = |v: &Value| {
        let mut hits: Vec<String> = v["hits"]
            .as_array()
            .map(|h| h.iter().map(|x| x.to_string()).collect())
            .unwrap_or_default();
        hits.sort();
        let ranking: Vec<String> = match sort_field {
            None => v["hits"]
                .as_array()
                .map(|h| h.iter().map(|x| x[1].to_string()).collect())
                .unwrap_or_default(),
            Some(f) => v["ids"]
                .as_array()
                .map(|ids| {
                    ids.iter()
                        .map(|id| {
                            let id = id.as_u64().unwrap_or_default();
                            // sorting reads the first fast value of the doc
                            format!("{:?}", snap.fast.get(&(f.to_string(), id)).and_then(|v| v.first()))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        };
        (hits, ranking)
    };
    a.get("error").is_none() && key(a) == key(b)
}

struct QueryCmp {
    n_search: usize,
    n_equal: usize,
    n_ties: usize,
    n_other: usize,
    n_mlt_order: usize,
    n_mlt_random: usize,
    search_diffs: Vec<Value>,
    other_diffs: Vec<Value>,
}

/// Run every case of queries.json through pl-search on both indices.
fn compare_queries(bp: &SearchBackend, br: &SearchBackend, cases: &Value, users: &[Option<SearchUser>], a: &Snapshot) -> QueryCmp {
    let (mut n_search, mut n_equal, mut n_ties) = (0, 0, 0);
    let mut search_diffs = Vec::new();
    for case in cases["search"].as_array().into_iter().flatten() {
        for user in users {
            n_search += 1;
            let rp = run_search(bp, case, user.as_ref());
            let rr = run_search(br, case, user.as_ref());
            if rp == rr {
                n_equal += 1;
            } else if same_up_to_ties(case, &rp, &rr, a) {
                n_ties += 1;
                search_diffs.push(json!({"case": case, "user": user.as_ref().map(|u| u.id), "kind": "tie-order", "upstream": rp["ids"], "rust": rr["ids"]}));
            } else {
                search_diffs.push(json!({"case": case, "user": user.as_ref().map(|u| u.id), "kind": "different", "upstream": rp, "rust": rr}));
            }
        }
    }
    let mut other_diffs = Vec::new();
    let mut n_other = 0;
    let mut n_mlt_order = 0;
    let mut n_mlt_random = 0;
    for term in cases["autocomplete"].as_array().into_iter().flatten() {
        let term = term.as_str().unwrap_or_default();
        for user in users {
            n_other += 1;
            let p = bp.autocomplete(term, 10, user.as_ref()).map_err(|e| e.to_string());
            let r = br.autocomplete(term, 10, user.as_ref()).map_err(|e| e.to_string());
            if p != r {
                other_diffs.push(json!({"autocomplete": term, "user": user.as_ref().map(|u| u.id), "upstream": format!("{p:?}"), "rust": format!("{r:?}")}));
            }
        }
    }
    for id in cases["more_like_this"].as_array().into_iter().flatten() {
        let id = id.as_i64().unwrap_or_default();
        for user in users {
            n_other += 1;
            // Full result (limit None): MLT returns ids only, so equal-score ties can only
            // be told apart by comparing the complete sets; order-only differences are
            // counted separately (tie order = internal doc order, unstable upstream too).
            let p = bp.more_like_this_ids(id, user.as_ref(), None).map_err(|e| e.to_string());
            let r = br.more_like_this_ids(id, user.as_ref(), None).map_err(|e| e.to_string());
            if p != r {
                let sorted = |x: &Result<Vec<i64>, String>| {
                    x.clone().map(|mut v| {
                        v.sort();
                        v
                    })
                };
                // tantivy's MoreLikeThis picks its 12 query terms from a HashMap (random
                // iteration order) through a heap ordered by score only: terms with equal
                // tf-idf are chosen at random, upstream as well. Accept a different set if
                // repeating the query on the UPSTREAM index produces exactly that set.
                let rust_set = sorted(&r);
                let upstream_also = (0..20).any(|_| {
                    sorted(&bp.more_like_this_ids(id, user.as_ref(), None).map_err(|e| e.to_string()))
                        == rust_set
                });
                if sorted(&p) == rust_set {
                    n_mlt_order += 1;
                } else if upstream_also {
                    n_mlt_random += 1;
                } else {
                    other_diffs.push(json!({"more_like_this": id, "user": user.as_ref().map(|u| u.id), "upstream": format!("{p:?}"), "rust": format!("{r:?}")}));
                }
            }
        }
    }
    QueryCmp { n_search, n_equal, n_ties, n_other, n_mlt_order, n_mlt_random, search_diffs, other_diffs }
}

async fn load_cases_and_users(pool: &sqlx::PgPool) -> (Value, Vec<Option<SearchUser>>) {
    let cases: Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity/pl-indexer/queries.json"),
        )
        .expect("queries.json"),
    )
    .expect("queries json");
    let mut users: Vec<Option<SearchUser>> = Vec::new();
    for u in cases["users"].as_array().into_iter().flatten() {
        match u.as_i64() {
            None => users.push(None),
            Some(id) => {
                let rows = sqlx::query("SELECT group_id FROM auth_user_groups WHERE user_id = $1 ORDER BY group_id")
                    .bind(id as i32)
                    .fetch_all(pool)
                    .await
                    .expect("groups");
                let group_ids = rows.iter().map(|r| i64::from(r.get::<i32, _>(0))).collect();
                users.push(Some(SearchUser { id, group_ids }));
            }
        }
    }
    (cases, users)
}

async fn reference_pool() -> sqlx::PgPool {
    let url = std::env::var("PAPERRUST_INDEXER_DB_URL")
        .unwrap_or_else(|_| "postgres://paperless:paperless@127.0.0.1:54329/paperless".into());
    // read-only session: nothing in the reference DB can be changed
    let opts = PgConnectOptions::from_str(&url)
        .expect("db url")
        .options([("default_transaction_read_only", "on")]);
    PgPoolOptions::new().max_connections(2).connect_with(opts).await.expect("connect")
}

fn index_diffs(py: &Path, rs: &Path) -> (Vec<Value>, Snapshot, Snapshot) {
    let mut diffs: Vec<Value> = Vec::new();
    let s_py = std::fs::read_to_string(py.join(".index_settings.json")).unwrap_or_default();
    let s_rs = std::fs::read_to_string(rs.join(".index_settings.json")).unwrap_or_default();
    if s_py != s_rs {
        diffs.push(json!({"what": "settings", "upstream": s_py, "rust": s_rs}));
    }
    if py.join(".rebuilding").exists() != rs.join(".rebuilding").exists() {
        diffs.push(json!({"what": "marker", "file": ".rebuilding"}));
    }
    let a = snapshot(py);
    let b = snapshot(rs);
    if a.num_docs != b.num_docs {
        diffs.push(json!({"what": "num_docs", "upstream": a.num_docs, "rust": b.num_docs}));
    }
    diff_maps("stored", &a.stored, &b.stored, &mut diffs);
    diff_maps("postings", &a.postings, &b.postings, &mut diffs);
    diff_maps("fieldnorm", &a.fieldnorms, &b.fieldnorms, &mut diffs);
    diff_maps("fast", &a.fast, &b.fast, &mut diffs);
    (diffs, a, b)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs reference PostgreSQL + PAPERRUST_INDEXER_PARITY_DIR from gen_reference.py"]
async fn parity_against_upstream_index() {
    let base = PathBuf::from(std::env::var("PAPERRUST_INDEXER_PARITY_DIR").expect("PAPERRUST_INDEXER_PARITY_DIR"));
    let py_index = base.join("py_data").join("index");
    assert!(py_index.join("meta.json").exists(), "upstream index missing: {}", py_index.display());
    let rust_index = base.join("rust_index");
    if rust_index.exists() {
        std::fs::remove_dir_all(&rust_index).expect("clean rust_index");
    }
    let pool = reference_pool().await;

    let ix = Indexer::new(IndexerConfig { index_dir: rust_index.clone(), language: Some(LANG.into()) });
    let started = std::time::Instant::now();
    let stats = ix.rebuild(&pool, &RebuildOptions::default()).await.expect("rebuild");
    let rebuild_ms = started.elapsed().as_millis();

    let (diffs, a, b) = index_diffs(&py_index, &rust_index);
    let n_index_diffs = diffs.len();

    // ---- searches through pl-search on both indices
    let (cases, users) = load_cases_and_users(&pool).await;
    let bp = backend(&py_index);
    let br = backend(&rust_index);
    let QueryCmp { n_search, n_equal, n_ties, n_other, n_mlt_order, n_mlt_random, search_diffs, other_diffs } =
        compare_queries(&bp, &br, &cases, &users, &a);
    let real_search_diffs = search_diffs.iter().filter(|d| d["kind"] == "different").count();

    // Control (optional): a SECOND upstream rebuild of the same DB. Shows that the
    // tie order also changes between two upstream runs (not a Rust difference).
    let control = std::env::var("PAPERRUST_INDEXER_PARITY_CONTROL_INDEX").ok().map(|c| {
        let bc = backend(Path::new(&c));
        let (mut exact, mut ties, mut different) = (0, 0, 0);
        for case in cases["search"].as_array().into_iter().flatten() {
            for user in &users {
                let rp = run_search(&bp, case, user.as_ref());
                let rc = run_search(&bc, case, user.as_ref());
                if rp == rc {
                    exact += 1;
                } else if same_up_to_ties(case, &rp, &rc, &a) {
                    ties += 1;
                } else {
                    different += 1;
                }
            }
        }
        json!({"index": c, "searches_identical": exact, "tie_order_only": ties, "different": different})
    });

    let report = json!({
        "rust_rebuild_documents": stats.documents,
        "rust_rebuild_ms": rebuild_ms,
        "upstream": {"num_docs": a.num_docs, "segments": open(&py_index).searchable_segment_ids().map(|s| s.len()).unwrap_or(0)},
        "rust": {"num_docs": b.num_docs, "segments": open(&rust_index).searchable_segment_ids().map(|s| s.len()).unwrap_or(0)},
        "compared": {
            "stored_docs": a.stored.len(),
            "terms": a.postings.len(),
            "fieldnorms": a.fieldnorms.len(),
            "fast_values": a.fast.len(),
            "searches": n_search,
            "searches_identical": n_equal,
            "searches_tie_order_only": n_ties,
            "autocomplete_and_mlt": n_other,
            "mlt_tie_order_only": n_mlt_order, "mlt_random_term_choice": n_mlt_random,
        },
        "control_upstream_vs_upstream": control,
        "index_diffs": diffs,
        "search_diffs": search_diffs,
        "other_diffs": other_diffs,
    });
    std::fs::write(base.join("report.json"), serde_json::to_string_pretty(&report).unwrap_or_default())
        .expect("write report");
    println!("{}", serde_json::to_string_pretty(&report["compared"]).unwrap_or_default());
    println!(
        "index_diffs={n_index_diffs} search_diffs={real_search_diffs} (tie-order {n_ties}) other_diffs={}",
        other_diffs.len()
    );
    assert_eq!(n_index_diffs, 0, "index differs, see {}", base.join("report.json").display());
    assert_eq!(real_search_diffs, 0, "search results differ");
    assert!(other_diffs.is_empty(), "autocomplete/mlt differ");
}

fn copy_dir(src: &Path, dst: &Path) {
    if dst.exists() {
        std::fs::remove_dir_all(dst).expect("clean");
    }
    std::fs::create_dir_all(dst).expect("mkdir");
    for e in std::fs::read_dir(src).expect("read_dir") {
        let e = e.expect("entry");
        if e.file_type().expect("type").is_file() {
            std::fs::copy(e.path(), dst.join(e.file_name())).expect("copy");
        }
    }
}

/// Same operations as gen_reference.py::run_upstream_incremental, on a copy of the
/// same fresh upstream index.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs reference PostgreSQL + gen_reference.py --incremental"]
async fn incremental_parity() {
    let base = PathBuf::from(std::env::var("PAPERRUST_INDEXER_PARITY_DIR").expect("PAPERRUST_INDEXER_PARITY_DIR"));
    let inc_base = base.join("inc_base");
    let py_inc = base.join("py_inc");
    assert!(py_inc.join("meta.json").exists(), "run gen_reference.py --incremental first");
    let rs_inc = base.join("rs_inc");
    copy_dir(&inc_base, &rs_inc);
    let pool = reference_pool().await;

    let (ix, outcome) = Indexer::open(IndexerConfig { index_dir: rs_inc.clone(), language: Some(LANG.into()) })
        .expect("open");
    assert_eq!(outcome, pl_indexer::settings::OpenOutcome::Opened);
    // 1) bulk path
    let mut batch = ix.batch_default().await.expect("batch");
    let n = batch.add_or_update_ids(&pool, &[1, 3, 201, 196, 999_999]).await.expect("ids");
    assert_eq!(n, 2, "roots 1 and 3; 196 trashed, 999999 unknown, 201 -> 1");
    batch.remove(5).expect("remove");
    batch.commit().expect("commit");
    // 2) single path with a version id
    let mut batch = ix.batch_default().await.expect("batch");
    assert!(batch.add_or_update(&pool, 202).await.expect("single"));
    batch.commit().expect("commit");
    // 3) convenience methods
    assert_eq!(ix.remove(7).await.expect("remove"), pl_indexer::WriteOutcome::Written);
    assert_eq!(ix.add_or_update(&pool, 8).await.expect("add"), pl_indexer::WriteOutcome::Written);

    let (diffs, a, b) = index_diffs(&py_inc, &rs_inc);
    let (cases, users) = load_cases_and_users(&pool).await;
    let bp = backend(&py_inc);
    let br = backend(&rs_inc);
    let QueryCmp { n_search, n_equal, n_ties, n_other, n_mlt_order, n_mlt_random, search_diffs, other_diffs } =
        compare_queries(&bp, &br, &cases, &users, &a);
    let real = search_diffs.iter().filter(|d| d["kind"] == "different").count();
    let report = json!({
        "upstream": {"num_docs": a.num_docs, "segments": open(&py_inc).searchable_segment_ids().map(|s| s.len()).unwrap_or(0)},
        "rust": {"num_docs": b.num_docs, "segments": open(&rs_inc).searchable_segment_ids().map(|s| s.len()).unwrap_or(0)},
        "compared": {
            "stored_docs": a.stored.len(), "terms": a.postings.len(), "fieldnorms": a.fieldnorms.len(),
            "fast_values": a.fast.len(), "searches": n_search, "searches_identical": n_equal,
            "searches_tie_order_only": n_ties, "autocomplete_and_mlt": n_other, "mlt_tie_order_only": n_mlt_order, "mlt_random_term_choice": n_mlt_random,
        },
        "index_diffs": diffs,
        "search_diffs": search_diffs,
        "other_diffs": other_diffs,
    });
    std::fs::write(base.join("report_incremental.json"), serde_json::to_string_pretty(&report).unwrap_or_default())
        .expect("write report");
    println!("incremental: {}", serde_json::to_string_pretty(&report["compared"]).unwrap_or_default());
    println!("incremental: index_diffs={} search_diffs={real} other_diffs={}", report["index_diffs"].as_array().map(|a| a.len()).unwrap_or(0), report["other_diffs"].as_array().map(|a| a.len()).unwrap_or(0));
    assert!(report["index_diffs"].as_array().is_some_and(|d| d.is_empty()), "index differs");
    assert_eq!(real, 0, "search results differ");
    assert!(report["other_diffs"].as_array().is_some_and(|d| d.is_empty()), "autocomplete/mlt differ");
}
