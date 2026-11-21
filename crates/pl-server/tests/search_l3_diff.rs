//! L3 differential test: full-text search of the document list `legacy` (upstream flow: rank all hits,
//! intersect with all visible IDs) against `native` (permissions in the index, top-N, hit count from
//! tantivy, SQL only for the page) – same hit count, same IDs in the same order on the
//! delivered page, same `__search_hit__` (score, highlights, rank), same errors,
//! with `need_all_ids` the same complete list.
//!
//! Only against a DB **copy** + index copy (the mutation phase writes to both):
//!
//!   PL_L3_DB=postgres://paperless:paperless@127.0.0.1:54329/pl_l3_snap \
//!   PL_L3_INDEX=<data>/index PL_L3_TZ=Europe/Zurich \
//!     cargo test -p pl-server -j 3 --test search_l3_diff -- --ignored --nocapture
//!
//! Environment: `PL_L3_USERS` (user IDs, comma; default all active), `PL_L3_TERMS` (terms per
//! initial letter, default 3), `PL_L3_MLT` (number of `more_like_id` documents, default 10),
//! `PL_L3_MUTATE=1` (mutation phase: revoke share, trash, owner change →
//! exact path until caught up, then fast again), `PL_L3_TIMING_OUT` (CSV per case),
//! `PL_L3_QUICK=1` (page 1 only, without random pages).
//!
//! Contract: docs/contract/pl-search.md "L3".

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use pl_docfilter::{FilterContext, QueryParams};
use pl_search::views::{BoxFut, DocumentSearchOutcome, Engine, FilteredDocuments, SearchOptions, ViewError, ViewResponse};
use pl_search::{SearchBackend, SearchSettings};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use sea_query::{Alias, Expr, PostgresQueryBuilder, SelectStatement};
use serde_json::Value;
use sqlx::{PgPool, Row};

const DOC: &str = "documents_document";

fn env_num(k: &str, d: u64) -> u64 {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

async fn run_ids(db: &PgPool, q: &SelectStatement) -> Result<Vec<i64>, ViewError> {
    let sql = q.to_string(PostgresQueryBuilder);
    let rows = sqlx::query(&sql).fetch_all(db).await?;
    Ok(rows.iter().map(|r| r.get::<i32, _>(0) as i64).collect())
}

/// `filtered_qs` like pl-documents `listing::Filtered` (AST path).
struct Filtered<'a> {
    db: &'a PgPool,
    q: SelectStatement,
}

impl FilteredDocuments for Filtered<'_> {
    fn ids_in<'a>(&'a self, ids: &'a [i64]) -> BoxFut<'a, Result<Vec<i64>, ViewError>> {
        Box::pin(async move {
            let mut q = self.q.clone();
            q.and_where(Expr::col((Alias::new(DOC), Alias::new("id"))).is_in(ids.iter().copied()));
            run_ids(self.db, &q).await
        })
    }

    fn all_ids(&self) -> BoxFut<'_, Result<Vec<i64>, ViewError>> {
        Box::pin(async move {
            let mut q = self.q.clone();
            q.clear_order_by();
            run_ids(self.db, &q).await
        })
    }
}

/// Result as the list delivers it (pagination like DRF / pl-documents `search_list`).
#[derive(Debug, PartialEq)]
enum Outcome {
    Page { count: usize, page: Vec<Value>, all: Option<Vec<i64>> },
    Invalid,
    Response(String),
    Error(String),
}

fn resolve_start(params: &[(String, String)], count: usize, size: usize) -> Option<usize> {
    let num_pages = count.max(1).div_ceil(size.max(1));
    let raw = params.iter().rev().find(|(k, _)| k == "page").map(|(_, v)| v.as_str());
    let page = match raw {
        None => 1,
        Some("last") => num_pages,
        Some(v) => match v.trim().parse::<usize>() {
            Ok(p) if p >= 1 => p,
            _ => return None,
        },
    };
    (page <= num_pages).then(|| (page - 1) * size)
}

#[allow(clippy::too_many_arguments)]
async fn search(
    db: &PgPool,
    backend: &Arc<SearchBackend>,
    tz: chrono_tz::Tz,
    user: &pl_authz::User,
    params: &[(String, String)],
    size: usize,
    engine: Engine,
    need_all: bool,
) -> Outcome {
    let qp = QueryParams::from_pairs(params.iter().cloned());
    let fctx = FilterContext::new(tz);
    let q = match pl_store::docsql::v1_list(db, &fctx, user, &qp).await {
        Ok((q, _)) => q,
        Err(e) => return Outcome::Error(format!("filter {e:?}")),
    };
    let filtered = Filtered { db, q };
    let ru = pl_search::routes::request_user(user);
    let opts = SearchOptions { need_all_ids: need_all, engine: Some(engine) };
    match pl_search::views::document_search_with(db, backend.clone(), &ru, params, size, &filtered, opts).await {
        Err(e) => Outcome::Error(e.to_string()),
        Ok(DocumentSearchOutcome::Response(r)) => Outcome::Response(match r {
            ViewResponse::Json(s, v) => format!("{s} {v}"),
            ViewResponse::Text(s, t) => format!("{s} {t}"),
        }),
        Ok(DocumentSearchOutcome::Page(p)) => {
            let count = p.count();
            let Some(start) = resolve_start(params, count, size) else {
                return Outcome::Invalid;
            };
            let stop = (start + size).min(count);
            let page = p
                .slice(start, stop)
                .iter()
                .map(|h| {
                    let mut v = h.to_json();
                    v["id"] = Value::from(h.id);
                    v
                })
                .collect();
            let all = need_all.then(|| p.ordered_ids.clone());
            Outcome::Page { count, page, all }
        }
    }
}

fn p(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

async fn users(db: &PgPool) -> Vec<pl_authz::User> {
    let ids: Vec<i64> = match std::env::var("PL_L3_USERS") {
        Ok(s) => s.split(',').filter_map(|x| x.trim().parse().ok()).collect(),
        Err(_) => sqlx::query_scalar("SELECT id::int8 FROM auth_user WHERE is_active ORDER BY id").fetch_all(db).await.unwrap(),
    };
    let mut out = Vec::new();
    for id in ids {
        if let Some(u) = pl_authz::user::load_user_by_id(db, id).await.unwrap() {
            out.push(u);
        }
    }
    out
}

fn index_language(dir: &std::path::Path) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join(".index_settings.json")).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    v.get("language").and_then(|l| l.as_str()).map(str::to_string)
}

/// Frequent terms of the index (autocomplete words per initial letter).
fn frequent_terms(backend: &SearchBackend, per_letter: usize) -> Vec<String> {
    let mut out = Vec::new();
    for c in "abcdefghiklmnoprstuwz".chars() {
        if let Ok(words) = backend.autocomplete(&c.to_string(), per_letter, None) {
            out.extend(words.into_iter().filter(|w| w.chars().count() >= 3));
        }
    }
    out
}

fn query_cases(terms: &[String], rng: &mut StdRng) -> Vec<Vec<(String, String)>> {
    let mut qs: Vec<String> = Vec::new();
    for t in terms {
        qs.push(t.clone());
    }
    for _ in 0..terms.len().min(30) {
        let a = terms.choose(rng).unwrap();
        let b = terms.choose(rng).unwrap();
        qs.push(format!("{a} {b}"));
        qs.push(format!("{a} OR {b}"));
        qs.push(format!("{a} NOT {b}"));
        qs.push(format!("\"{a} {b}\""));
        qs.push(format!("title:{a}"));
        qs.push(format!("content:{a} AND created:[2015 to 2022]"));
        let pre: String = a.chars().take(3).collect();
        qs.push(format!("{pre}*"));
    }
    qs.extend(
        ["zzqxnotfound", "created:[-1 year to now]", "asn:1", "tag:inbox", "notes:x", "created:foo", "title:", "a AND"]
            .iter()
            .map(|s| s.to_string()),
    );
    let mut cases = Vec::new();
    let orderings =
        ["", "-score", "score", "created", "-created", "-added", "modified", "archive_serial_number", "-page_count", "-num_notes", "title"];
    for q in &qs {
        let o = orderings[rng.gen_range(0..orderings.len())];
        let mut c = vec![("query".to_string(), q.clone())];
        if !o.is_empty() {
            c.push(("ordering".into(), o.into()));
        }
        cases.push(c);
    }
    for t in terms.iter().take(15) {
        let short: String = t.chars().take(4).collect();
        cases.push(p(&[("text", &short)]));
        cases.push(p(&[("title_search", &short)]));
        cases.push(p(&[("text", &short), ("query", "created:[2018 to 2026]")]));
        cases.push(p(&[("query", t), ("tags__id__all", "1")]));
        cases.push(p(&[("query", t), ("truncate_content", "true"), ("ordering", "-created")]));
    }
    cases
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "braucht DB- und Index-Kopie (PL_L3_DB, PL_L3_INDEX), siehe Modulkopf"]
async fn native_equals_legacy() {
    let url = std::env::var("PL_L3_DB").expect("PL_L3_DB (Kopie!)");
    let index_dir = std::path::PathBuf::from(std::env::var("PL_L3_INDEX").expect("PL_L3_INDEX"));
    let tz: chrono_tz::Tz = std::env::var("PL_L3_TZ").ok().and_then(|s| s.parse().ok()).unwrap_or(chrono_tz::UTC);
    let db = sqlx::postgres::PgPoolOptions::new().max_connections(6).connect(&url).await.unwrap();
    let backend = Arc::new(SearchBackend::new(SearchSettings {
        index_dir: index_dir.clone(),
        language: index_language(&index_dir),
        fuzzy_threshold: None,
        time_zone: tz,
    }));
    assert!(pl_search::db::search_index_clean(&db).await, "paperrust_search_dirty fehlt oder ist nicht leer");
    let mut rng = StdRng::seed_from_u64(env_num("PL_L3_SEED", 3));
    let terms = frequent_terms(&backend, env_num("PL_L3_TERMS", 3) as usize);
    println!("terms: {}", terms.len());
    let cases = query_cases(&terms, &mut rng);
    let users = users(&db).await;
    let quick = std::env::var("PL_L3_QUICK").is_ok();
    let mut timing: Vec<String> = vec!["user,params,size,page,legacy_ms,native_ms,count".into()];
    let mut stats: BTreeMap<&str, u64> = BTreeMap::new();
    let mut diffs: Vec<String> = Vec::new();
    let (mut t_legacy, mut t_native) = (0f64, 0f64);

    // more_like_id: random visible documents
    let mlt_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id::int8 FROM documents_document WHERE deleted_at IS NULL AND root_document_id IS NULL ORDER BY md5(id::text) LIMIT $1",
    )
    .bind(env_num("PL_L3_MLT", 10) as i64)
    .fetch_all(&db)
    .await
    .unwrap();
    let mut all_cases = cases.clone();
    for id in &mlt_ids {
        all_cases.push(p(&[("more_like_id", &id.to_string())]));
    }

    for user in &users {
        for base in &all_cases {
            let sizes: &[usize] = if quick { &[25] } else { &[25, 10] };
            for &size in sizes {
                // Page 1, then (without quick) page 2, last, random
                let mut pages: Vec<String> = vec!["1".into()];
                if !quick {
                    pages.push("2".into());
                    pages.push("last".into());
                }
                let mut i = 0;
                while i < pages.len() {
                    let page = pages[i].clone();
                    i += 1;
                    let mut params = base.clone();
                    params.push(("page".into(), page.clone()));
                    params.push(("page_size".into(), size.to_string()));
                    let t0 = Instant::now();
                    let l = search(&db, &backend, tz, user, &params, size, Engine::Legacy, true).await;
                    let tl = t0.elapsed().as_secs_f64() * 1000.0;
                    let t1 = Instant::now();
                    let n = search(&db, &backend, tz, user, &params, size, Engine::Native, false).await;
                    let tn = t1.elapsed().as_secs_f64() * 1000.0;
                    let n_all = search(&db, &backend, tz, user, &params, size, Engine::Native, true).await;
                    t_legacy += tl;
                    t_native += tn;
                    let cnt = match &l {
                        Outcome::Page { count, .. } => *count,
                        _ => 0,
                    };
                    timing.push(format!(
                        "{},{:?},{size},{page},{tl:.1},{tn:.1},{cnt}",
                        user.username,
                        base.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
                    ));
                    // Comparison without `all` (native without need_all has only the prefix)
                    let strip = |o: &Outcome| match o {
                        Outcome::Page { count, page, .. } => Outcome::Page { count: *count, page: page.clone(), all: None },
                        Outcome::Invalid => Outcome::Invalid,
                        Outcome::Response(r) => Outcome::Response(r.clone()),
                        Outcome::Error(e) => Outcome::Error(e.clone()),
                    };
                    *stats.entry("cases").or_default() += 1;
                    if let Outcome::Page { count, .. } = &l
                        && *count > 0
                    {
                        *stats.entry("with_hits").or_default() += 1;
                    }
                    if strip(&l) != strip(&n) {
                        diffs.push(format!("user={} params={params:?}\n  legacy={l:?}\n  native={n:?}", user.username));
                    } else if l != n_all {
                        diffs.push(format!("user={} params={params:?} (need_all)\n  legacy={l:?}\n  native={n_all:?}", user.username));
                    } else {
                        *stats.entry("identical").or_default() += 1;
                    }
                    if !quick
                        && page == "1"
                        && let Outcome::Page { count, .. } = &l
                    {
                        let num_pages = (*count).max(1).div_ceil(size);
                        if num_pages > 3 {
                            pages.push(rng.gen_range(3..=num_pages).to_string());
                            pages.push((num_pages + 1).to_string());
                        }
                    }
                }
            }
        }
        println!("user {} done: {:?} diffs={}", user.username, stats, diffs.len());
    }
    if let Ok(out) = std::env::var("PL_L3_TIMING_OUT") {
        std::fs::write(out, timing.join("\n")).unwrap();
    }
    for d in diffs.iter().take(20) {
        println!("DIFF {d}");
    }
    println!(
        "L3 RESULT {url}: users={} cases={:?} diffs={} legacy_ms={t_legacy:.0} native_ms={t_native:.0}",
        users.len(),
        stats,
        diffs.len()
    );
    assert!(diffs.is_empty(), "{} Abweichungen", diffs.len());
}

/// Mutation phase: changes that remove visibility → exact path until caught up,
/// then fast path again; both times equal to `legacy`. Writes to DB and index copy!
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "schreibt in DB- und Index-Kopie (PL_L3_DB, PL_L3_INDEX, PL_L3_MUTATE=1)"]
async fn dirty_tracking_and_drain() {
    assert_eq!(std::env::var("PL_L3_MUTATE").as_deref(), Ok("1"), "PL_L3_MUTATE=1 (schreibt!)");
    let url = std::env::var("PL_L3_DB").expect("PL_L3_DB (Kopie!)");
    let index_dir = std::path::PathBuf::from(std::env::var("PL_L3_INDEX").expect("PL_L3_INDEX"));
    let tz: chrono_tz::Tz = std::env::var("PL_L3_TZ").ok().and_then(|s| s.parse().ok()).unwrap_or(chrono_tz::UTC);
    let db = sqlx::postgres::PgPoolOptions::new().max_connections(6).connect(&url).await.unwrap();
    let lang = index_language(&index_dir);
    let backend = Arc::new(SearchBackend::new(SearchSettings {
        index_dir: index_dir.clone(),
        language: lang.clone(),
        fuzzy_threshold: None,
        time_zone: tz,
    }));
    let ix = pl_indexer::Indexer::new(pl_indexer::IndexerConfig { index_dir: index_dir.clone(), language: lang });
    let users = users(&db).await;
    let mut rng = StdRng::seed_from_u64(11);
    let terms = frequent_terms(&backend, 2);
    let mut checks = 0u64;
    let mut fast_after_drain = 0u64;
    for round in 0..env_num("PL_L3_ROUNDS", 12) {
        // a visibility-reducing change
        let kind = round % 4;
        match kind {
            0 => {
                // revoke user share
                let n = sqlx::query("DELETE FROM guardian_userobjectpermission WHERE id IN (SELECT id FROM guardian_userobjectpermission ORDER BY md5(id::text || $1) LIMIT 3)")
                    .bind(round.to_string()).execute(&db).await.unwrap().rows_affected();
                println!("round {round}: revoke user grants {n}");
            }
            1 => {
                let n = sqlx::query("DELETE FROM guardian_groupobjectpermission WHERE id IN (SELECT id FROM guardian_groupobjectpermission ORDER BY md5(id::text || $1) LIMIT 3)")
                    .bind(round.to_string()).execute(&db).await.unwrap().rows_affected();
                println!("round {round}: revoke group grants {n}");
            }
            2 => {
                let n = sqlx::query("UPDATE documents_document SET deleted_at = now() WHERE id IN (SELECT id FROM documents_document WHERE deleted_at IS NULL AND root_document_id IS NULL ORDER BY md5(id::text || $1) LIMIT 3)")
                    .bind(round.to_string()).execute(&db).await.unwrap().rows_affected();
                println!("round {round}: trash {n}");
            }
            _ => {
                let n = sqlx::query("UPDATE documents_document SET owner_id = (SELECT id FROM auth_user ORDER BY md5(id::text || $1) LIMIT 1) WHERE id IN (SELECT id FROM documents_document WHERE deleted_at IS NULL AND root_document_id IS NULL ORDER BY md5(id::text || $1) LIMIT 5)")
                    .bind(round.to_string()).execute(&db).await.unwrap().rows_affected();
                println!("round {round}: owner change {n}");
            }
        }
        let dirty_before: i64 = sqlx::query_scalar("SELECT count(*) FROM paperrust_search_dirty").fetch_one(&db).await.unwrap();
        assert!(dirty_before > 0 || kind < 2, "Trigger hat nichts gemerkt");
        for phase in ["dirty", "drained"] {
            if phase == "drained" {
                let d = pl_indexer::dirty::drain_once(&ix, &db, || backend.reload().unwrap()).await.unwrap();
                assert!(!d.deferred);
                assert!(pl_search::db::search_index_clean(&db).await, "nach dem Nachführen nicht leer");
            }
            for user in users.iter().take(8) {
                for _ in 0..4 {
                    let t = terms.choose(&mut rng).unwrap().clone();
                    let params = p(&[("query", &t), ("page", "1"), ("page_size", "25")]);
                    let l = search(&db, &backend, tz, user, &params, 25, Engine::Legacy, true).await;
                    let n = search(&db, &backend, tz, user, &params, 25, Engine::Native, false).await;
                    let strip = |o: Outcome| match o {
                        Outcome::Page { count, page, .. } => Outcome::Page { count, page, all: None },
                        x => x,
                    };
                    assert_eq!(strip(l), strip(n), "round {round} {phase} user {} query {t}", user.username);
                    checks += 1;
                    if phase == "drained" {
                        fast_after_drain += 1;
                    }
                }
            }
        }
    }
    println!("L3 MUTATE RESULT {url}: checks={checks} fast_after_drain={fast_after_drain} diffs=0");
}
