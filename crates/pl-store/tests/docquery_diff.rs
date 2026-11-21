//! L1 differential test: previous path (`pl_docfilter::apply` + `apply_ordering` +
//! `PermittedObjectsFilter`) against the AST path (`pl_docfilter::ast` → `pl_store::docsql`) on the
//! same DB – same IDs, same order, same count, same errors.
//!
//! Cases: (1) the requests from `crates/pl-docfilter/tests/parity.rs`, (2) all
//! `GET /api/documents/` cases of the parity catalogues `tests/parity/**/cases*.json`,
//! (3) randomly generated combinations (fixed seed, `PL_L1_RANDOM` count, default 3000)
//! incl. random `custom_field_query` trees, (4) `all=true` selection (lenient filters).
//! Each case for all users of the DB copy (superuser, group/individual permissions, inactive).
//!
//! Read-only, only against a DB **copy** (`paperless_snapshot` → `pl_l1`):
//!
//!   PL_L1_DB=postgres://paperless:paperless@127.0.0.1:54329/pl_l1 \
//!     cargo test -p pl-store -j 3 --test docquery_diff -- --ignored --nocapture
//!
//! `PL_L1_TIMING=1`: additionally per case the runtime of both SQL forms (page `LIMIT 50` and
//! `COUNT`, warm, minimum of 3) to `PL_L1_TIMING_OUT` (CSV); `PL_L1_QUERIES=<file>` (one
//! query string per line) instead of the fixed cases – for measuring on 100k/1M (`PL_L1_RANDOM=0`).

use std::time::Instant;

use pl_authz::models::DOCUMENT;
use pl_authz::objects::{FilterOpts, permitted_objects_filter};
use pl_core::sqlbind::SqlxBinder;
use pl_docfilter::{FilterContext, FilterError, QueryParams};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use sea_query::{Alias, Expr, Order, PostgresQueryBuilder, Query, SelectStatement};
use serde_json::Value;
use sqlx::Row;

const DOC: &str = "documents_document";

fn err_key(e: FilterError) -> String {
    match e {
        FilterError::Validation(v) => format!("400 {v}"),
        // text of the 500s is internal; only the class has to be the same (like the parity run)
        _ => "500".into(),
    }
}

async fn run(pool: &sqlx::PgPool, q: &SelectStatement) -> Result<Vec<i64>, String> {
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values)
        .fetch_all(pool)
        .await
        .map(|rows| rows.iter().map(|r| r.get::<i32, _>(0) as i64).collect())
        .map_err(|e| format!("{e}: {sql}"))
}

async fn count(pool: &sqlx::PgPool, q: &SelectStatement) -> i64 {
    let (sql, values) = pl_core::pagination::count_statement(q).build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).fetch_one(pool).await.map(|r| r.get::<i64, _>(0)).unwrap_or(-1)
}

/// `pl_documents::listing::filtered_documents_legacy` (previous path).
async fn legacy_list(
    pool: &sqlx::PgPool,
    ctx: &FilterContext,
    user: &pl_authz::User,
    p: &QueryParams,
) -> Result<SelectStatement, FilterError> {
    let mut q = Query::select();
    q.column((Alias::new(DOC), Alias::new("id")))
        .from(Alias::new(DOC))
        .and_where(Expr::col((Alias::new(DOC), Alias::new("root_document_id"))).is_null())
        .and_where(Expr::col((Alias::new(DOC), Alias::new("deleted_at"))).is_null());
    pl_docfilter::apply(&mut q, p, user, ctx, pool).await?;
    pl_docfilter::apply_ordering(&mut q, p.get("ordering"), pool).await?;
    q.cond_where(permitted_objects_filter(Some(user), &DOCUMENT, DOC, &FilterOpts::default()));
    Ok(q)
}

/// `pl_documents::selection_filter::resolve_all` without Tantivy (previous path).
async fn legacy_selection(
    pool: &sqlx::PgPool,
    ctx: &FilterContext,
    user: &pl_authz::User,
    p: &QueryParams,
) -> Result<SelectStatement, FilterError> {
    let mut q = Query::select();
    q.column((Alias::new(DOC), Alias::new("id")))
        .from(Alias::new(DOC))
        .and_where(Expr::col((Alias::new(DOC), Alias::new("root_document_id"))).is_null())
        .and_where(Expr::col((Alias::new(DOC), Alias::new("deleted_at"))).is_null())
        .and_where(Expr::col((Alias::new(DOC), Alias::new("id"))).in_subquery(pl_authz::objects::permitted_document_ids(
            Some(user),
            "view_document",
            false,
        )));
    pl_docfilter::filterset::apply_filterset_lenient(&mut q, p, user, ctx, pool).await?;
    Ok(q)
}

fn with_tiebreak(q: &SelectStatement) -> SelectStatement {
    let mut q = q.clone();
    q.order_by((Alias::new(DOC), Alias::new("id")), Order::Asc);
    q
}

fn ordered(q: &SelectStatement) -> SelectStatement {
    let mut q = q.clone();
    q.order_by((Alias::new(DOC), Alias::new("created")), Order::Desc).order_by((Alias::new(DOC), Alias::new("id")), Order::Desc);
    q
}

#[derive(Default)]
struct Stats {
    cases: usize,
    same_rows: usize,
    same_error: usize,
    tie_only: usize,
    nonempty: usize,
    different: Vec<String>,
}

/// Page (LIMIT 50) and count separately, each minimum of 3 (warm).
async fn time_min(pool: &sqlx::PgPool, q: &SelectStatement) -> (f64, f64) {
    let mut page = q.clone();
    page.limit(50);
    let (mut bp, mut bc) = (f64::MAX, f64::MAX);
    for _ in 0..3 {
        let t = Instant::now();
        let _ = run(pool, &page).await;
        bp = bp.min(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        let _ = count(pool, q).await;
        bc = bc.min(t.elapsed().as_secs_f64() * 1000.0);
    }
    (bp, bc)
}

#[allow(clippy::too_many_arguments)]
async fn compare_list(
    pool: &sqlx::PgPool,
    ctx: &FilterContext,
    user: &pl_authz::User,
    label: &str,
    pairs: &[(String, String)],
    st: &mut Stats,
    timing: &mut Option<Vec<String>>,
) {
    st.cases += 1;
    let p = QueryParams::from_pairs(pairs.iter().cloned());
    let legacy = legacy_list(pool, ctx, user, &p).await;
    let ast = pl_store::docsql::v1_list(pool, ctx, user, &p).await.map(|(q, _)| q);
    let desc = format!("{label} user={} {:?}", user.username, pairs);
    match (legacy, ast) {
        (Err(a), Err(b)) => {
            let (a, b) = (err_key(a), err_key(b));
            if a == b {
                st.same_error += 1;
            } else {
                st.different.push(format!("{desc}: error {a} / {b}"));
            }
        }
        (Ok(a), Ok(b)) => {
            let (ra, rb) = (run(pool, &a).await, run(pool, &b).await);
            match (ra, rb) {
                (Ok(x), Ok(y)) => {
                    let (ca, cb) = (count(pool, &a).await, count(pool, &b).await);
                    if ca != cb || ca != x.len() as i64 {
                        st.different.push(format!("{desc}: count {ca} / {cb} (rows {})", x.len()));
                        return;
                    }
                    if !x.is_empty() {
                        st.nonempty += 1;
                    }
                    if x == y {
                        st.same_rows += 1;
                    } else {
                        // same set; difference only on a tie of the sort keys?
                        let (ta, tb) = (run(pool, &with_tiebreak(&a)).await, run(pool, &with_tiebreak(&b)).await);
                        if ta.is_ok() && ta == tb {
                            st.tie_only += 1;
                        } else {
                            st.different.push(format!("{desc}: rows differ ({} / {})", x.len(), y.len()));
                        }
                    }
                    if let Some(t) = timing.as_mut() {
                        let ((pa, ca), (pb, cb)) = (time_min(pool, &a).await, time_min(pool, &b).await);
                        t.push(format!("{},{},{},{:.1},{:.1},{:.1},{:.1}", label, user.username, csv(pairs), pa, pb, ca, cb));
                    }
                }
                (Err(e), _) | (_, Err(e)) => st.different.push(format!("{desc}: sql error {e}")),
            }
        }
        (Ok(_), Err(e)) => st.different.push(format!("{desc}: only ast error {}", err_key(e))),
        (Err(e), Ok(_)) => st.different.push(format!("{desc}: only legacy error {}", err_key(e))),
    }
}

async fn compare_selection(pool: &sqlx::PgPool, ctx: &FilterContext, user: &pl_authz::User, pairs: &[(String, String)], st: &mut Stats) {
    st.cases += 1;
    let p = QueryParams::from_pairs(pairs.iter().cloned());
    let legacy = legacy_selection(pool, ctx, user, &p).await;
    let ast = pl_store::docsql::v1_selection(pool, ctx, user, &p).await;
    let desc = format!("selection user={} {:?}", user.username, pairs);
    match (legacy, ast) {
        (Err(a), Err(b)) => {
            let (a, b) = (err_key(a), err_key(b));
            if a == b {
                st.same_error += 1;
            } else {
                st.different.push(format!("{desc}: error {a} / {b}"));
            }
        }
        (Ok(a), Ok(b)) => match (run(pool, &ordered(&a)).await, run(pool, &ordered(&b)).await) {
            (Ok(x), Ok(y)) if x == y => {
                st.same_rows += 1;
                if !x.is_empty() {
                    st.nonempty += 1;
                }
            }
            (Ok(x), Ok(y)) => st.different.push(format!("{desc}: rows differ ({} / {})", x.len(), y.len())),
            (Err(e), _) | (_, Err(e)) => st.different.push(format!("{desc}: sql error {e}")),
        },
        (Ok(_), Err(e)) => st.different.push(format!("{desc}: only ast error {}", err_key(e))),
        (Err(e), Ok(_)) => st.different.push(format!("{desc}: only legacy error {}", err_key(e))),
    }
}

fn csv(pairs: &[(String, String)]) -> String {
    let s: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!("\"{}\"", s.join("&").replace('"', "\"\""))
}

fn qs(q: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(q.as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect()
}

fn cfq(json: &str) -> String {
    let enc: String = url::form_urlencoded::byte_serialize(json.as_bytes()).collect();
    format!("custom_field_query={enc}")
}

/// The requests from `crates/pl-docfilter/tests/parity.rs` (Django parity run).
fn parity_queries() -> Vec<String> {
    let mut q: Vec<String> = [
        "",
        "ordering=title",
        "ordering=-title",
        "ordering=-created",
        "ordering=archive_serial_number",
        "ordering=-num_notes",
        "ordering=owner",
        "ordering=correspondent__name",
        "ordering=-document_type__name",
        "ordering=storage_path__name",
        "ordering=page_count",
        "ordering=added",
        "ordering=-modified",
        "ordering=id",
        "ordering=bogus",
        "ordering=title,-id",
        "ordering=custom_field_999",
        "ordering=-num_notes,id",
        "title__icontains=tax",
        "title__istartswith=a",
        "title__iendswith=e",
        "title__iexact=x",
        "content__icontains=quarterly",
        "content__istartswith=d",
        "title_content=steuer",
        "title_content=strasse",
        "title_content=fi",
        "title_content=%C3%9F",
        "title_content=a_b%25",
        "content__iexact=x",
        "content__iendswith=n",
        "content__icontains=SS",
        "search=ss",
        "archive_serial_number__gt=1100",
        "archive_serial_number__lte=1050",
        "archive_serial_number=1005",
        "archive_serial_number__isnull=1",
        "archive_serial_number__gt=-1",
        "created__year=2024",
        "created__month=3",
        "created__day=15",
        "created__gt=2024-06-01",
        "created__date__lte=2024-12-31",
        "created__lt=10/25/2024",
        "added__date__gt=2025-01-10",
        "added__date__lte=2025-01-08",
        "added__gt=2025-01-10T00:00:00Z",
        "added__lt=2025-01-09",
        "modified__year=2025",
        "added__month=1",
        "added__day=7",
        "correspondent__isnull=1",
        "correspondent__isnull=0",
        "correspondent__id=1",
        "correspondent__id__in=1,2",
        "correspondent__id__none=1",
        "correspondent__name__icontains=a",
        "document_type__id__none=1",
        "document_type__name__istartswith=in",
        "storage_path__id__none=1",
        "storage_path__isnull=1",
        "tags__id__all=2,3",
        "tags__id__in=1,2",
        "tags__id__none=1",
        "tags__id=6",
        "tags__name__iexact=paid",
        "tags__name__icontains=tax",
        "is_tagged=0",
        "is_tagged=1",
        "is_in_inbox=true",
        "is_in_inbox=false",
        "owner__id=3",
        "owner__id__in=3,4",
        "owner__id__none=2",
        "owner__isnull=1",
        "shared_by__id=2",
        "shared_by__id=3",
        "shared_by__id=4",
        "mime_type=pdf",
        "mime_type=text",
        "has_custom_fields=1",
        "has_custom_fields=0",
        "custom_fields__id__all=1,3",
        "custom_fields__id__none=1",
        "custom_fields__id__in=1,2",
        "custom_fields__icontains=a",
        "custom_fields__icontains=medium",
        "custom_fields__icontains=CHF",
        "has_duplicates=1",
        "has_duplicates=0",
        "search=tax",
        "search=%22quarterly-tax%22",
        "search=steuer,rechnung",
        "id__in=1,2,3,500",
        "id=5",
        "id__in=,",
        "tags__id__all=1,x",
        "page_count=1",
        "archive_serial_number=abc",
        "created__gt=bad",
        "added__gt=bad",
        "id__in=1,x&title__icontains=a%00",
        "search=a%00",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for id in 1..=11 {
        q.push(format!("ordering=custom_field_{id}"));
        q.push(format!("ordering=-custom_field_{id}"));
        q.push(cfq(&format!("[{id}, \"exists\", true]")));
        q.push(cfq(&format!("[{id}, \"isnull\", true]")));
        q.push(cfq(&format!("[\"NOT\", [{id}, \"exists\", true]]")));
    }
    for j in [
        r#"["Text", "icontains", "a"]"#,
        r#"["Text", "exact", ""]"#,
        r#"["Website", "istartswith", "https"]"#,
        r#"["Due date", "gte", "2025-01-01"]"#,
        r#"["Due date", "year__exact", 2025]"#,
        r#"["Due date", "month__lt", 6]"#,
        r#"["Due date", "range", ["2024-01-01", "today"]]"#,
        r#"["Paid?", "exact", true]"#,
        r#"["Paid?", "exact", "false"]"#,
        r#"["Count", "gt", 5]"#,
        r#"["Count", "in", [1, 2, 3]]"#,
        r#"["Score", "range", [0, 50.5]]"#,
        r#"["Amount", "gt", "100"]"#,
        r#"["Amount", "exact", "CHF100.00"]"#,
        r#"["Amount (no default)", "lte", "EUR50"]"#,
        r#"["Amount", "icontains", "CHF"]"#,
        r#"["Related docs", "contains", [1]]"#,
        r#"["Related docs", "contains", []]"#,
        r#"["Related docs", "contains", [1, 2]]"#,
        r#"["Priority", "exact", "Medium"]"#,
        r#"["Priority", "in", ["Low", "pr3HighAAAAAAAAA"]]"#,
        r#"["Notes (long)", "icontains", "e"]"#,
        r#"["OR", [["Count", "gt", 5], ["Paid?", "exact", true]]]"#,
        r#"["AND", [["Count", "isnull", false], ["NOT", ["Score", "lt", 10]]]]"#,
        "not json",
        r#"["nope", "exact", 1]"#,
        r#"["Count", "icontains", 1]"#,
        r#"["Count", "exact", "x"]"#,
        r#"["Due date", "exact", "not a date"]"#,
        r#"["AND", []]"#,
        r#"["xor", [["Count", "gt", 1]]]"#,
    ] {
        q.push(cfq(j));
    }
    q
}

/// `GET /api/documents/` cases of the parity catalogues (variables resolved via the DB).
async fn catalog_queries(pool: &sqlx::PgPool) -> Vec<Vec<(String, String)>> {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/parity");
    let mut vars: Vec<(String, String)> = Vec::new();
    for (name, sql) in [
        ("correspondent_id", "SELECT min(id)::text FROM documents_correspondent"),
        ("correspondent_id_last", "SELECT max(id)::text FROM documents_correspondent"),
        ("custom_field_id", "SELECT min(id)::text FROM documents_customfield"),
        ("doc_id", "SELECT min(id)::text FROM documents_document WHERE root_document_id IS NULL AND deleted_at IS NULL"),
        ("doc_id_last", "SELECT max(id)::text FROM documents_document WHERE root_document_id IS NULL AND deleted_at IS NULL"),
        ("document_type_id", "SELECT min(id)::text FROM documents_documenttype"),
        ("storage_path_id", "SELECT min(id)::text FROM documents_storagepath"),
        ("tag_id", "SELECT min(id)::text FROM documents_tag"),
        ("tag_id_last", "SELECT max(id)::text FROM documents_tag"),
        ("user1_id", "SELECT COALESCE((SELECT id FROM auth_user WHERE username = 'user1'), 3)::text"),
    ] {
        let v: Option<String> = sqlx::query_scalar(sql).fetch_one(pool).await.ok().flatten();
        vars.push((format!("${{{name}}}"), v.unwrap_or_else(|| "0".into())));
    }
    let mut out = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(root)];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !(name.starts_with("cases") && name.ends_with(".json")) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
            let cases = v.get("cases").cloned().unwrap_or(v);
            for c in cases.as_array().into_iter().flatten() {
                if c["method"] != "GET" || c["path"] != "/api/documents/" {
                    continue;
                }
                let Some(q) = c["query"].as_object() else { continue };
                let pairs: Vec<(String, String)> = q
                    .iter()
                    .map(|(k, v)| {
                        let mut s = match v {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        for (n, val) in &vars {
                            s = s.replace(n, val);
                        }
                        (k.clone(), s)
                    })
                    .collect();
                out.push(pairs);
            }
        }
    }
    out
}

// --------------------------------------------------------------------- Random cases

fn pick<'a, T>(r: &mut StdRng, v: &'a [T]) -> &'a T {
    &v[r.gen_range(0..v.len())]
}

fn rand_ids(r: &mut StdRng, max: i64) -> String {
    let n = r.gen_range(1..=3);
    let mut ids: Vec<String> = (0..n).map(|_| r.gen_range(0..=max).to_string()).collect();
    if r.gen_bool(0.05) {
        ids.push(pick(r, &["x", "", "99999999999", "-1", " 2", "1e3"]).to_string());
    }
    ids.join(",")
}

const TEXTS: &[&str] = &[
    "a",
    "e",
    "tax",
    "TAX",
    "steuer",
    "rechnung",
    "ss",
    "ß",
    "fi",
    "a_b",
    "%",
    "\\",
    "quarterly",
    "invoice",
    " x ",
    "Ü",
    "ı",
    "2024",
    "pdf",
];
const DATES: &[&str] =
    &["2024-01-01", "2024-06-15", "2025-01-08", "2025-01-10", "2023-12-31", "9999-12-31", "0001-01-01", "10/25/2024", "bad", ""];
const DTS: &[&str] =
    &["2025-01-10T00:00:00Z", "2025-01-09", "2024-03-31T02:30:00", "2024-10-27 02:30", "2025-01-08T12:00:00+05:00", "bad", ""];
const NUMS: &[&str] = &["1", "3", "5", "1000", "1050", "1100", "-1", "0", "2147483648", "-2147483649", "1e3", "1.9", "abc", "", "1e51"];
const BOOLS: &[&str] = &["1", "0", "true", "false", "True", "x", ""];

fn rand_param(r: &mut StdRng) -> (String, String) {
    let lookups = ["icontains", "istartswith", "iendswith", "iexact"];
    let t = |r: &mut StdRng| pick(r, TEXTS).to_string();
    match r.gen_range(0..40) {
        0 => ("id__in".into(), rand_ids(r, 220)),
        1 => ("id".into(), pick(r, NUMS).to_string()),
        2 => (format!("title__{}", pick(r, &lookups)), t(r)),
        3 => (
            pick(
                r,
                &[
                    "archive_serial_number",
                    "archive_serial_number__gt",
                    "archive_serial_number__gte",
                    "archive_serial_number__lt",
                    "archive_serial_number__lte",
                ],
            )
            .to_string(),
            pick(r, NUMS).to_string(),
        ),
        4 => ("archive_serial_number__isnull".into(), pick(r, BOOLS).to_string()),
        5 => (
            pick(
                r,
                &[
                    "created__year",
                    "created__month",
                    "created__day",
                    "added__year",
                    "added__month",
                    "added__day",
                    "modified__year",
                    "modified__month",
                    "modified__day",
                ],
            )
            .to_string(),
            pick(r, &["2024", "2025", "3", "1", "15", "7", "13", "0", "10000", "x"]).to_string(),
        ),
        6 => (
            pick(
                r,
                &[
                    "created__gt",
                    "created__gte",
                    "created__lt",
                    "created__lte",
                    "created__date__gt",
                    "created__date__gte",
                    "created__date__lt",
                    "created__date__lte",
                ],
            )
            .to_string(),
            pick(r, DATES).to_string(),
        ),
        7 => (
            pick(
                r,
                &[
                    "added__date__gt",
                    "added__date__gte",
                    "added__date__lt",
                    "added__date__lte",
                    "modified__date__gt",
                    "modified__date__lte",
                ],
            )
            .to_string(),
            pick(r, DATES).to_string(),
        ),
        8 => (
            pick(r, &["added__gt", "added__gte", "added__lt", "added__lte", "modified__gt", "modified__lt"]).to_string(),
            pick(r, DTS).to_string(),
        ),
        9 => (format!("{}__{}", pick(r, &["original_filename", "checksum"]), pick(r, &lookups)), t(r)),
        10 => (format!("{}__isnull", pick(r, &["correspondent", "document_type", "storage_path", "owner"])), pick(r, BOOLS).to_string()),
        11 => (format!("{}__id__in", pick(r, &["correspondent", "document_type", "storage_path", "owner"])), rand_ids(r, 8)),
        12 => (
            format!("{}__id", pick(r, &["correspondent", "document_type", "storage_path", "owner"])),
            pick(r, &["1", "2", "3", "4", "0", "x"]).to_string(),
        ),
        13 => (format!("{}__name__{}", pick(r, &["correspondent", "document_type", "storage_path", "tags"]), pick(r, &lookups)), t(r)),
        14 => ("tags__id__in".into(), rand_ids(r, 12)),
        15 => ("tags__id".into(), pick(r, &["1", "2", "6", "x", ""]).to_string()),
        16 => ("tags__id__all".into(), rand_ids(r, 12)),
        17 => ("tags__id__none".into(), rand_ids(r, 12)),
        18 => (format!("{}__id__none", pick(r, &["correspondent", "document_type", "storage_path", "owner"])), rand_ids(r, 6)),
        19 => ("is_tagged".into(), pick(r, BOOLS).to_string()),
        20 => ("is_in_inbox".into(), pick(r, &["true", "false", "1", "True"]).to_string()),
        21 => ("title_content".into(), t(r)),
        22 => (format!("content__{}", pick(r, &lookups)), t(r)),
        23 => (format!("custom_fields__id__{}", pick(r, &["all", "none", "in"])), rand_ids(r, 12)),
        24 => ("has_custom_fields".into(), pick(r, BOOLS).to_string()),
        25 => ("custom_field_query".into(), rand_cfq(r, 0)),
        26 => ("shared_by__id".into(), pick(r, &["2", "3", "4", "5"]).to_string()),
        27 => ("mime_type".into(), pick(r, &["pdf", "text", "image", "PDF", " "]).to_string()),
        28 => ("has_duplicates".into(), pick(r, BOOLS).to_string()),
        29 => ("search".into(), pick(r, &["tax", "\"quarterly tax\"", "steuer,rechnung", "a b", "ss", "  "]).to_string()),
        30 => ("custom_fields__icontains".into(), pick(r, &["a", "medium", "CHF", "low", "x"]).to_string()),
        31..=34 => ("custom_field_query".into(), rand_cfq(r, 0)),
        _ => ("ordering".into(), rand_ordering(r)),
    }
}

fn rand_ordering(r: &mut StdRng) -> String {
    let fields = [
        "id",
        "title",
        "correspondent__name",
        "document_type__name",
        "storage_path__name",
        "created",
        "modified",
        "added",
        "archive_serial_number",
        "num_notes",
        "owner",
        "page_count",
        "bogus",
    ];
    if r.gen_bool(0.25) {
        let id = r.gen_range(1..=12);
        return format!("{}custom_field_{id}", if r.gen_bool(0.5) { "-" } else { "" });
    }
    let n = r.gen_range(1..=3);
    (0..n).map(|_| format!("{}{}", if r.gen_bool(0.5) { "-" } else { "" }, pick(r, &fields))).collect::<Vec<_>>().join(",")
}

fn rand_cfq(r: &mut StdRng, depth: usize) -> String {
    if depth < 3 && r.gen_bool(0.3) {
        let op = pick(r, &["AND", "OR", "and", "NOT"]).to_string();
        if op == "NOT" {
            return format!("[\"NOT\", {}]", rand_cfq(r, depth + 1));
        }
        let n = r.gen_range(1..=3);
        let items: Vec<String> = (0..n).map(|_| rand_cfq(r, depth + 1)).collect();
        return format!("[\"{op}\", [{}]]", items.join(", "));
    }
    // atom: field (ID or name), operator, value matching or deliberately wrong
    let fields: [(&str, &str); 11] = [
        ("Text", "s"),
        ("Website", "s"),
        ("Due date", "d"),
        ("Paid?", "b"),
        ("Count", "i"),
        ("Score", "f"),
        ("Amount", "m"),
        ("Amount (no default)", "m"),
        ("Related docs", "l"),
        ("Priority", "sel"),
        ("Notes (long)", "s"),
    ];
    let idx = r.gen_range(0..fields.len());
    let (name, kind) = fields[idx];
    let key = if r.gen_bool(0.5) { format!("\"{name}\"") } else { (idx + 1).to_string() };
    let (op, val): (String, String) = match kind {
        "s" => {
            let op = pick(r, &["exact", "icontains", "istartswith", "iendswith", "isnull", "exists", "in", "gt"]).to_string();
            let v = match op.as_str() {
                "isnull" | "exists" => pick(r, &["true", "false"]).to_string(),
                "in" => "[\"a\", \"b\", \"x\"]".into(),
                _ => format!("\"{}\"", pick(r, &["a", "e", "https", "", "x"])),
            };
            (op, v)
        }
        "d" => {
            let base = pick(r, &["exact", "gt", "gte", "lt", "lte", "range", "in", "isnull", "exists"]).to_string();
            if r.gen_bool(0.4) && base != "exists" {
                let comp = pick(r, &["year", "iso_year", "month", "day", "week", "week_day", "iso_week_day", "quarter"]);
                let v = match base.as_str() {
                    "range" => "[1, 6]".into(),
                    "in" => "[1, 2, 2025]".into(),
                    "isnull" => "false".into(),
                    _ => pick(r, &["2025", "6", "1", "2024", "0", "3000000000", "-3000000000", "12"]).to_string(),
                };
                (format!("{comp}__{base}"), v)
            } else {
                let v = match base.as_str() {
                    "range" => "[\"2024-01-01\", \"today\"]".into(),
                    "in" => "[\"2025-01-01\", \"2024-06-15\"]".into(),
                    "isnull" | "exists" => pick(r, &["true", "false"]).to_string(),
                    _ => format!("\"{}\"", pick(r, &["2025-01-01", "2024-06-15", "today", "in 30 days", "3 weeks ago", "bad"])),
                };
                (base, v)
            }
        }
        "b" => {
            (pick(r, &["exact", "isnull", "exists", "in"]).to_string(), pick(r, &["true", "false", "\"false\"", "1", "[true]"]).to_string())
        }
        "i" | "f" => {
            let op = pick(r, &["exact", "gt", "gte", "lt", "lte", "range", "in", "isnull"]).to_string();
            let v = match op.as_str() {
                "range" => "[0, 50.5]".into(),
                "in" => "[1, 2, 3]".into(),
                "isnull" => "false".into(),
                _ => pick(r, &["5", "10", "-1", "\"x\"", "2147483648", "1e3", "12.5"]).to_string(),
            };
            (op, v)
        }
        "m" => {
            let op =
                pick(r, &["exact", "gt", "gte", "lt", "lte", "range", "in", "icontains", "istartswith", "isnull", "exists"]).to_string();
            let v = match op.as_str() {
                "range" => "[\"10\", \"CHF100.00\"]".into(),
                "in" => "[\"CHF100.00\", \"EUR50\"]".into(),
                "isnull" | "exists" => "true".into(),
                "icontains" | "istartswith" => format!("\"{}\"", pick(r, &["CHF", "EUR", "1"])),
                _ => format!("\"{}\"", pick(r, &["100", "CHF100.00", "EUR50", "0", "-inf", "x"])),
            };
            (op, v)
        }
        "l" => {
            let op = pick(r, &["contains", "exact", "isnull", "exists", "in"]).to_string();
            let v = match op.as_str() {
                "contains" => pick(r, &["[1]", "[]", "[1, 2]", "[2]", "[999]"]).to_string(),
                "exact" => "[1, 2]".into(),
                "in" => "[[1]]".into(),
                _ => "true".into(),
            };
            (op, v)
        }
        _ => {
            let op = pick(r, &["exact", "in", "isnull", "exists"]).to_string();
            let v = match op.as_str() {
                "in" => "[\"Low\", \"pr3HighAAAAAAAAA\"]".into(),
                "isnull" | "exists" => "true".into(),
                _ => format!("\"{}\"", pick(r, &["Medium", "Low", "pr1LowAAAAAAAAAA", "nope"])),
            };
            (op, v)
        }
    };
    format!("[{key}, \"{op}\", {val}]")
}

#[tokio::test]
#[ignore = "braucht eine DB-Kopie (PL_L1_DB), siehe Modulkopf"]
async fn ast_equals_legacy() {
    let db = std::env::var("PL_L1_DB").unwrap_or_else(|_| "postgres://paperless:paperless@127.0.0.1:54329/pl_l1".into());
    assert!(!db.trim_end_matches('/').ends_with("/paperless"), "nie gegen die Referenz-DB `paperless`");
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&db).await.expect("db");
    let n_random: usize = std::env::var("PL_L1_RANDOM").ok().and_then(|s| s.parse().ok()).unwrap_or(3000);
    let mut timing = std::env::var("PL_L1_TIMING")
        .ok()
        .map(|_| vec!["case,user,query,legacy_page_ms,ast_page_ms,legacy_count_ms,ast_count_ms".to_string()]);
    let user_ids: Vec<i64> = std::env::var("PL_L1_USERS")
        .ok()
        .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![2, 3, 4, 5, 6]);
    let mut users = Vec::new();
    for id in user_ids {
        if let Some(u) = pl_authz::user::load_user_by_id(&pool, id).await.expect("load") {
            users.push(u);
        }
    }
    assert!(!users.is_empty());
    let mut ctx = FilterContext::new("Europe/Zurich".parse().unwrap());
    ctx.now = "2026-11-10T12:00:00Z".parse().unwrap();

    let mut st = Stats::default();
    // (0) only a request list (measurement, e.g. 1M data set): `PL_L1_QUERIES=<file>`, one query string per line
    if let Ok(path) = std::env::var("PL_L1_QUERIES") {
        let text = std::fs::read_to_string(&path).expect("PL_L1_QUERIES");
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let pairs = qs(line);
            for u in &users {
                compare_list(&pool, &ctx, u, "list", &pairs, &mut st, &mut timing).await;
            }
        }
    }
    // (1) parity requests
    if std::env::var("PL_L1_SKIP_FIXED").is_err() && std::env::var("PL_L1_QUERIES").is_err() {
        for q in parity_queries() {
            let pairs = qs(&q);
            for u in &users {
                compare_list(&pool, &ctx, u, "parity", &pairs, &mut st, &mut timing).await;
            }
        }
        // (2) Kataloge
        for pairs in catalog_queries(&pool).await {
            for u in &users {
                compare_list(&pool, &ctx, u, "catalog", &pairs, &mut st, &mut timing).await;
            }
        }
    }
    let fixed = st.cases;
    // (3) random (fixed seed)
    let mut r = StdRng::seed_from_u64(0x4c31);
    for i in 0..n_random {
        let n = r.gen_range(1..=4);
        let pairs: Vec<(String, String)> = (0..n).map(|_| rand_param(&mut r)).collect();
        let u = &users[i % users.len()];
        compare_list(&pool, &ctx, u, "random", &pairs, &mut st, &mut timing).await;
    }
    let random = st.cases - fixed;
    // (4) selection all=true (lenient)
    let mut r = StdRng::seed_from_u64(0x5e1);
    for i in 0..(n_random / 5) {
        let n = r.gen_range(1..=4);
        let pairs: Vec<(String, String)> = (0..n).map(|_| rand_param(&mut r)).filter(|(k, _)| k != "ordering").collect();
        let u = &users[i % users.len()];
        compare_selection(&pool, &ctx, u, &pairs, &mut st).await;
    }
    println!(
        "cases={} (fixed={fixed} random={random} selection={}) same_rows={} same_error={} tie_only={} nonempty={} different={}",
        st.cases,
        st.cases - fixed - random,
        st.same_rows,
        st.same_error,
        st.tie_only,
        st.nonempty,
        st.different.len()
    );
    for d in st.different.iter().take(50) {
        println!("DIFF  {d}");
    }
    if let Some(t) = timing {
        let out = std::env::var("PL_L1_TIMING_OUT").unwrap_or_else(|_| "docquery_timing.csv".into());
        std::fs::write(&out, t.join("\n")).expect("timing out");
    }
    assert!(st.different.is_empty());
}

/// Aid for the measurement: SQL of both paths for `PL_L1_DUMP=<query string>` and `PL_L1_USERS`.
#[tokio::test]
#[ignore = "Hilfswerkzeug (PL_L1_DUMP)"]
async fn dump_sql() {
    let db = std::env::var("PL_L1_DB").unwrap_or_else(|_| "postgres://paperless:paperless@127.0.0.1:54329/pl_l1".into());
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(&db).await.expect("db");
    let q = std::env::var("PL_L1_DUMP").unwrap_or_default();
    let uid: i64 = std::env::var("PL_L1_USERS").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
    let u = pl_authz::user::load_user_by_id(&pool, uid).await.expect("load").expect("user");
    let ctx = FilterContext::new("Europe/Zurich".parse().unwrap());
    let p = QueryParams::from_pairs(qs(&q));
    let a = legacy_list(&pool, &ctx, &u, &p).await.expect("legacy");
    let b = pl_store::docsql::v1_list(&pool, &ctx, &u, &p).await.expect("ast").0;
    println!("LEGACY: {};", a.to_string(PostgresQueryBuilder));
    println!("AST: {};", b.to_string(PostgresQueryBuilder));
}
