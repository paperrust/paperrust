//! Hit cache for frequent search words in title/content (docs/PERF-SCALE.md §12).
//!
//! **Problem.** `title_content=<frequent word>` requires, for the total count, checking every row with
//! `UPPER(..) LIKE` – at 1 million documents 2.2 GB of text (17 s when idle, 35–42 s under
//! foreign load). The trigram index does not help there (every candidate is re-checked, "rechnung"
//! is in > 50 % of the documents).
//!
//! **Solution, exact.** The ids of the rows whose *own* column matches are determined once and
//! stored with the snapshot of the database (`pg_current_snapshot()` in the same
//! `REPEATABLE READ` transaction as the query). Afterwards, for every row `T`:
//!
//! * `T` is **unchanged** since the snapshot – its row version (`xmin`) was already confirmed
//!   in the snapshot: its text is the same, so it matches exactly if its id
//!   is in the stored set.
//! * `T` is **new or changed** (every `UPDATE` writes a new row version with a new
//!   `xmin`): the condition is checked on the text as before.
//!
//! Deleted rows no longer appear in the query. The result is therefore the same
//! condition; only the text of unchanged rows is not read again.
//!
//! "Confirmed in the snapshot" means: `xmin` is before the snapshot's `xmax` and does not belong
//! to the transactions running at the time (`xip`). Checked cyclically via the age:
//! `age(xmin) > age(xmax)`. The system column `xmin` shows the original transaction even for frozen
//! rows (since PostgreSQL 9.4; verified with PostgreSQL 18 and `VACUUM FREEZE`),
//! so freezing after the snapshot does not fake an old row. Very old rows
//! whose age "wraps around" beyond 2^31 are at worst wrongly considered changed (then
//! re-checked – slower, but correct). The validity is limited to [`TTL`].
//!
//! **Form in SQL.** The set goes into the query as a bitmask (`bytea`, bit `i` = id `i`, least significant bit
//! first like `get_bit`): 125 kB for 1 million ids, check per row in O(1) and
//! independent of `work_mem` and the plan cache (an id list as `IN (…)` with 500,000
//! entries does not fit in `work_mem` and then runs linearly per row).
//!
//! **Own data model (stage 1).** There the document table ([`DOC`]) is a view without
//! system columns; the text lives in `paperrust_document_content`. The set then applies to this
//! table (id = `document_id`). The query joins the text rows that are new or changed since the snapshot
//! ([`HitSet::changed_rows`], reads the row versions of the text table once per query,
//! without text) and checks only those on the text ([`HitSet::cond_native`]). Titles are not
//! stored there: they live in the narrow table and are directly cheap to check.
//!
//! **Parallel build (PERF-SCALE §13).** The determination reads the whole text and is CPU-bound
//! (unpacking + folding + `LIKE`; the cluster allows only 2 parallel workers). It therefore runs
//! over up to [`WORKERS_VAR`] own connections, one block range of the table each (`ctid`,
//! TID range scan), all with the **same** snapshot (`pg_export_snapshot()` of the leading
//! transaction, `SET TRANSACTION SNAPSHOT` in the others). This makes the set exactly that of a
//! single query in this snapshot. If one part fails, the leading
//! transaction determines everything alone.
//!
//! Switch: [`VAR`] (`PAPERRUST_TEXT_HIT_CACHE=off` switches it off).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use pl_docfilter::sql::{DOC, TextLookup as SqlLookup, col, like_escape};
use pl_docfilter::textfold::{UpperFold, folded_lookup};
use sea_query::{Alias, Expr, Query, SimpleExpr, Value};

/// Environment variable: `off`/`false`/`0`/`no` switches the hit cache off.
pub const VAR: &str = "PAPERRUST_TEXT_HIT_CACHE";

/// Maximum age of an entry (afterwards it is re-determined at the next need).
pub const TTL: Duration = Duration::from_secs(15 * 60);

/// Maximum number of stored entries (each ≈ largest id / 8 bytes).
pub const MAX_ENTRIES: usize = 32;

/// Largest id for which a bitmask is created (8 MiB). Above it: no hit cache.
pub const MAX_ID: i64 = 1 << 26;

/// Environment variable: number of connections for the build (default: processor cores, at most 8;
/// `1` = one query).
pub const WORKERS_VAR: &str = "PAPERRUST_TEXT_HIT_CACHE_WORKERS";

/// Minimum size of a block range per connection (8 kB blocks; 32 MB). Smaller tables: one
/// query.
const MIN_BLOCKS_PER_WORKER: i64 = 4096;

/// Text table of the own data model.
pub const NATIVE_CONTENT_TABLE: &str = "paperrust_document_content";

/// Connections for the build ([`WORKERS_VAR`]).
fn workers() -> usize {
    let default = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(8);
    pl_core::config::var(WORKERS_VAR)
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(default)
        .clamp(1, 32)
}

/// Where text and row version live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layout {
    /// The document table ([`DOC`]) is a table (v3.4.2).
    Django,
    /// Own data model stage 1: text in `paperrust_document_content`.
    Native,
}

/// Layout of the database (cached for 60 s per database).
pub async fn layout(db: &sqlx::PgPool) -> Result<Layout, sqlx::Error> {
    type LayoutCache = Mutex<HashMap<String, (Instant, Layout)>>;
    static C: OnceLock<LayoutCache> = OnceLock::new();
    let o = db.connect_options();
    let key = format!("{}:{}/{}", o.get_host(), o.get_port(), o.get_database().unwrap_or(""));
    let cache = C.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, l)) = cache.lock().ok().and_then(|m| m.get(&key).copied())
        && at.elapsed() < Duration::from_secs(60)
    {
        return Ok(l);
    }
    let native: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
          WHERE n.nspname = current_schema() AND c.relname = 'paperrust_document_content' AND c.relkind = 'r')",
    )
    .fetch_one(db)
    .await?;
    let l = if native { Layout::Native } else { Layout::Django };
    if let Ok(mut m) = cache.lock() {
        m.insert(key, (Instant::now(), l));
    }
    Ok(l)
}

/// Hit cache switched on?
pub fn enabled() -> bool {
    !matches!(
        pl_core::config::var(VAR).map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("off" | "false" | "0" | "no")
    )
}

/// Column whose own text is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HitColumn {
    Title,
    Content,
}

impl HitColumn {
    fn name(self) -> &'static str {
        match self {
            HitColumn::Title => "title",
            HitColumn::Content => "content",
        }
    }
}

/// Stored hit set of a column for a search text.
#[derive(Debug)]
pub struct HitSet {
    /// Bit `i` (byte `i / 8`, bit `i % 8`, least significant first) = row with id `i` matches.
    bitmap: Vec<u8>,
    /// `xmax` of the snapshot (32-bit transaction number)
    xmax: u32,
    /// transactions running at the snapshot (32 bit)
    xip: Vec<u32>,
    built: Instant,
    /// Number of hits (log)
    pub hits: usize,
    /// Layout of the database at determination (decides where `xmin` is read)
    pub layout: Layout,
}

impl HitSet {
    /// From snapshot text (`xmin:xmax:xip,…`, 64-bit numbers) and hit ids.
    fn new(snapshot: &str, ids: &[i64]) -> Option<HitSet> {
        HitSet::with_layout(snapshot, ids, Layout::Django)
    }

    fn with_layout(snapshot: &str, ids: &[i64], layout: Layout) -> Option<HitSet> {
        let (xmax, xip) = parse_snapshot(snapshot)?;
        let max = ids.iter().copied().max().unwrap_or(0);
        if max >= MAX_ID || ids.iter().any(|&i| i < 0) {
            return None;
        }
        let mut bitmap = vec![0u8; usize::try_from(max / 8 + 1).ok()?];
        for &id in ids {
            let i = usize::try_from(id).ok()?;
            bitmap[i / 8] |= 1 << (i % 8);
        }
        Some(HitSet { bitmap, xmax, xip, built: Instant::now(), hits: ids.len(), layout })
    }

    /// For tests: set from snapshot text and ids.
    #[doc(hidden)]
    pub fn for_tests(snapshot: &str, ids: &[i64]) -> Option<HitSet> {
        HitSet::new(snapshot, ids)
    }

    /// For tests: like [`HitSet::for_tests`] for the own data model.
    #[doc(hidden)]
    pub fn for_tests_native(snapshot: &str, ids: &[i64]) -> Option<HitSet> {
        HitSet::with_layout(snapshot, ids, Layout::Native)
    }

    /// Bitmask (tests: comparison of two determinations).
    #[doc(hidden)]
    pub fn bitmap(&self) -> &[u8] {
        &self.bitmap
    }

    /// Set of the own data model (query: join [`HitSet::changed_rows`],
    /// [`HitSet::cond_native`])?
    pub fn native(&self) -> bool {
        self.layout == Layout::Native
    }

    /// Does the set contain `id`? (tests)
    pub fn contains(&self, id: i64) -> bool {
        usize::try_from(id)
            .ok()
            .and_then(|i| self.bitmap.get(i / 8).map(|b| b & (1 << (i % 8)) != 0))
            .unwrap_or(false)
    }

    /// "Row version `xmin` was confirmed in the snapshot".
    fn unchanged(&self, xmin: SimpleExpr) -> SimpleExpr {
        let mut unchanged =
            Expr::cust_with_exprs("age($1) > age(($2)::text::xid)", [xmin.clone(), Expr::val(self.xmax.to_string()).into()]);
        if !self.xip.is_empty() {
            let list = format!("{{{}}}", self.xip.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(","));
            unchanged = Expr::cust_with_exprs(
                "($1 AND NOT ($2 = ANY(($3)::text::xid[])))",
                [unchanged, xmin, Expr::val(list).into()],
            );
        }
        unchanged
    }

    /// "id of row `alias` is in the set".
    fn in_set(&self, alias: &str) -> SimpleExpr {
        let nbits = (self.bitmap.len() as i64) * 8;
        let bm: SimpleExpr = Expr::val(Value::Bytes(Some(Box::new(self.bitmap.clone())))).into();
        Expr::cust_with_exprs(
            "(CASE WHEN $1 < $2 THEN get_bit($3, ($1)::int4) = 1 ELSE FALSE END)",
            [col(alias, "id"), Expr::val(nbits).into(), bm],
        )
    }

    /// Condition "row `alias` matches" (layout [`Layout::Django`]): unchanged rows via
    /// the bitmask, new or changed ones via `recheck` (the same condition on the text).
    pub fn cond(&self, alias: &str, recheck: SimpleExpr) -> SimpleExpr {
        let xmin = Expr::cust(format!(r#""{alias}"."xmin""#));
        Expr::cust_with_exprs("(CASE WHEN $1 THEN $2 ELSE ($3) END)", [self.unchanged(xmin), self.in_set(alias), recheck])
    }

    /// Own data model: rows of the text table that are new or have been changed since the snapshot
    /// (`document_id`). The query joins them as `LEFT JOIN … AS <alias>`;
    /// this reads the row versions of the text table once per query (without text), the set is
    /// small, and the join stays linear – even if many rows were changed.
    ///
    /// Form `SELECT unnest(ARRAY(SELECT …)) AS document_id`: the inner query runs once beforehand
    /// (InitPlan). Joined directly, the planner estimates a third of all rows as changed and
    /// splits the hash join at 1 million documents into batches with temporary files
    /// (measured +1 s); this way it expects few rows, and the hash table grows as needed.
    pub fn changed_rows(&self) -> sea_query::SelectStatement {
        let t = "hit_chg";
        let inner = Query::select()
            .column((Alias::new(t), Alias::new("document_id")))
            .from_as(Alias::new(NATIVE_CONTENT_TABLE), Alias::new(t))
            .and_where(self.unchanged(Expr::cust(format!(r#""{t}"."xmin""#))).not())
            .to_owned();
        Query::select()
            .expr_as(
                Expr::cust_with_exprs(
                    "unnest(ARRAY $1)",
                    [SimpleExpr::SubQuery(None, Box::new(sea_query::SubQueryStatement::SelectStatement(inner)))],
                ),
                Alias::new("document_id"),
            )
            .to_owned()
    }

    /// Own data model: condition "row `alias` matches" when [`HitSet::changed_rows`] is joined as
    /// `changed` – changed rows via `recheck`, all others via the bitmask.
    ///
    /// Prerequisite: a row of the text table disappears only with its document (stage 1:
    /// `ON DELETE CASCADE`, no writer deletes it individually); a document without a text row counts
    /// as in the snapshot.
    pub fn cond_native(&self, alias: &str, changed: &str, recheck: SimpleExpr) -> SimpleExpr {
        Expr::cust_with_exprs(
            "(CASE WHEN $1 IS NOT NULL THEN ($2) ELSE $3 END)",
            [col(changed, "document_id"), recheck, self.in_set(alias)],
        )
    }
}

/// `xmin:xmax:xip1,xip2` (xid8) → (`xmax`, `xip`) as 32-bit numbers.
fn parse_snapshot(s: &str) -> Option<(u32, Vec<u32>)> {
    let mut it = s.trim().split(':');
    let _xmin: u64 = it.next()?.parse().ok()?;
    let xmax: u64 = it.next()?.parse().ok()?;
    let xip_s = it.next()?;
    if it.next().is_some() {
        return None;
    }
    let mut xip = Vec::new();
    for p in xip_s.split(',').filter(|p| !p.is_empty()) {
        xip.push(p.parse::<u64>().ok()? as u32);
    }
    Some((xmax as u32, xip))
}

type Cache = Mutex<HashMap<String, Arc<HitSet>>>;

fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Only one determination at a time (each reads the whole text; in parallel both would be
/// slower, and two identical requests should not compute twice).
fn build_lock() -> &'static tokio::sync::Mutex<()> {
    static L: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    L.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn key(db: &sqlx::PgPool, layout: Layout, c: HitColumn, lookup: SqlLookup, value: &str) -> String {
    let o = db.connect_options();
    format!(
        "{}:{}/{}|{layout:?}|{}|{lookup:?}|{value}",
        o.get_host(),
        o.get_port(),
        o.get_database().unwrap_or(""),
        c.name()
    )
}

fn cached(k: &str) -> Option<Arc<HitSet>> {
    let m = cache().lock().ok()?;
    m.get(k).filter(|h| h.built.elapsed() < TTL).cloned()
}

/// Hit set for `UPPER(<column>::text) LIKE …` (or the folded form `fold`) over **all**
/// rows of the table (also trash and versions; the outer query continues to filter).
/// `None`: switched off, ids too large, title in the own data model, or error (then the
/// previous path stays).
pub async fn hits(
    db: &sqlx::PgPool,
    fold: Option<&UpperFold>,
    c: HitColumn,
    lookup: SqlLookup,
    value: &str,
) -> Option<Arc<HitSet>> {
    if !enabled() {
        return None;
    }
    let layout = match layout(db).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(error = %e, "text hit cache: layout check failed, using full scan");
            return None;
        }
    };
    if layout == Layout::Native && c == HitColumn::Title {
        return None;
    }
    let k = key(db, layout, c, lookup, value);
    if let Some(h) = cached(&k) {
        return Some(h);
    }
    let _guard = build_lock().lock().await;
    if let Some(h) = cached(&k) {
        return Some(h);
    }
    let started = Instant::now();
    let h = match build(db, layout, fold, c, lookup, value).await {
        Ok(Some(h)) => Arc::new(h),
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(error = %e, "text hit cache: build failed, using full scan");
            return None;
        }
    };
    tracing::debug!(column = c.name(), hits = h.hits, ms = started.elapsed().as_millis() as u64, "text hit cache built");
    if let Ok(mut m) = cache().lock() {
        m.retain(|_, v| v.built.elapsed() < TTL);
        if m.len() >= MAX_ENTRIES
            && let Some(oldest) = m.iter().min_by_key(|(_, v)| v.built).map(|(k, _)| k.clone())
        {
            m.remove(&oldest);
        }
        m.insert(k, h.clone());
    }
    Some(h)
}

/// Table, id column and text column of the determination.
fn source(layout: Layout, c: HitColumn) -> (&'static str, &'static str, &'static str) {
    match layout {
        Layout::Django => (DOC, "id", c.name()),
        Layout::Native => (NATIVE_CONTENT_TABLE, "document_id", "content"),
    }
}

/// Query for the hit ids in a block range `[lo, hi)` (8 kB blocks; `None` = open).
fn range_query(
    layout: Layout,
    fold: Option<&UpperFold>,
    c: HitColumn,
    lookup: SqlLookup,
    value: &str,
    range: (Option<i64>, Option<i64>),
) -> (String, sea_query_binder::SqlxValues) {
    use pl_core::sqlbind::SqlxBinder;
    let (table, id, text) = source(layout, c);
    let col = |a: &str, c: &str| Expr::col((Alias::new(a), Alias::new(c)));
    let mut inner = Query::select();
    inner.from_as(Alias::new(table), Alias::new("t"));
    if let Some(lo) = range.0 {
        inner.and_where(Expr::cust(format!(r#""t"."ctid" >= '({lo},0)'::tid"#)));
    }
    if let Some(hi) = range.1 {
        inner.and_where(Expr::cust(format!(r#""t"."ctid" < '({hi},0)'::tid"#)));
    }
    let q = match upper_once_cond(fold, || col("s", "u").into(), lookup, value) {
        Some(cond) => {
            // unpack and uppercase the text once (`OFFSET 0`: the subquery is not
            // folded in, so `u` is not recomputed per condition)
            inner
                .expr_as(col("t", id), Alias::new("id"))
                .expr_as(Expr::cust_with_exprs(r#"UPPER(($1)::text COLLATE "C")"#, [col("t", text).into()]), Alias::new("u"))
                .offset(0);
            Query::select()
                .expr(col("s", "id").cast_as(Alias::new("int8")))
                .from_subquery(inner, Alias::new("s"))
                .and_where(cond)
                .to_owned()
        }
        None => {
            inner
                .expr(col("t", id).cast_as(Alias::new("int8")))
                .and_where(folded_lookup(fold, col("t", text).into(), lookup, value));
            inner
        }
    };
    q.build_sqlx(sea_query::PostgresQueryBuilder)
}

/// Text condition of the determination on `u = UPPER(x COLLATE "C")` (computed once per row),
/// result-equal to [`folded_lookup`] (PERF-SCALE §14). `None`: without folding or with a
/// non-ASCII search text (then the previous form).
///
/// The folding (`pl_docfilter::textfold`) is `replace(…replace(u, s1, U1)…, sk, Uk) op P` over
/// **all** special characters `s` of the database collation whose uppercase form `U` contains a character of the search
/// text; from more than `MAX_REPLACES` such characters `folded_lookup` instead takes
/// `UPPER(x)` of the collation – same result, but up to 7× slower here (measured:
/// "beitrag" 28 s instead of 8 s on 200,000 documents). If `u` contains none of the characters, the
/// `replace()` calls change nothing. Hence
///
/// `folding ≡ (u op P) OR (u contains an s_i AND replace-chain(u) op P)`
///
/// * "⇐": without special characters the folding is the left part, with special characters the right one.
/// * "⇒" (left part): a hit of the ASCII pattern `P` in `u` consists only of ASCII characters,
///   the `replace()` calls replace only non-ASCII characters – the hit stays (also at the start/end
///   and with `=`, because then `u` is entirely ASCII and there is nothing to replace).
///
/// `COLLATE "C"` leaves non-ASCII characters unchanged: "u contains s" = "x contains s". The
/// check `u LIKE '%s%'` runs bytewise in memory; the `replace()` chain only for the few
/// rows with one of the characters.
fn upper_once_cond(fold: Option<&UpperFold>, u: impl Fn() -> SimpleExpr, lookup: SqlLookup, value: &str) -> Option<SimpleExpr> {
    let f = fold?;
    if !value.is_ascii() {
        return None;
    }
    let (op, pattern) = match lookup {
        SqlLookup::IContains => ("LIKE", format!("%{}%", like_escape(value))),
        SqlLookup::IStartsWith => ("LIKE", format!("{}%", like_escape(value))),
        SqlLookup::IEndsWith => ("LIKE", format!("%{}", like_escape(value))),
        SqlLookup::IExact => ("=", value.to_string()),
    };
    let upper = pattern.to_ascii_uppercase();
    let relevant: Vec<&(char, String)> =
        f.specials().iter().filter(|(_, up)| up.chars().any(|ch| ch.is_ascii() && upper.contains(ch))).collect();
    let plain = Expr::cust_with_exprs(format!("$1 {op} $2"), [u(), Expr::val(upper.clone()).into()]);
    if relevant.is_empty() {
        return Some(plain);
    }
    let has_special = relevant
        .iter()
        .map(|(ch, _)| Expr::cust_with_exprs("$1 LIKE $2", [u(), Expr::val(format!("%{}%", like_escape(&ch.to_string()))).into()]))
        .reduce(|a, b| a.or(b))?;
    let mut folded = u();
    for (ch, up) in &relevant {
        folded = Expr::cust_with_exprs("replace($1, $2, $3)", [folded, Expr::val(ch.to_string()).into(), Expr::val(up.clone()).into()]);
    }
    let full = Expr::cust_with_exprs(format!("$1 {op} $2"), [folded, Expr::val(upper).into()]);
    Some(Expr::cust_with_exprs("(($1) OR (($2) AND ($3)))", [plain, has_special, full]))
}

/// Block ranges for `n` connections over `blocks` blocks; first without a lower bound, last without an
/// upper bound (rows in later-appended blocks are invisible in the snapshot anyway, so the
/// open bound does no harm).
fn ranges(blocks: i64, n: usize) -> Vec<(Option<i64>, Option<i64>)> {
    let n = n.max(1) as i64;
    (0..n)
        .map(|i| {
            let lo = (i > 0).then(|| blocks * i / n);
            let hi = (i < n - 1).then(|| blocks * (i + 1) / n);
            (lo, hi)
        })
        .collect()
}

/// Exported snapshot (`pg_export_snapshot()`, z. B. `00000003-0000001B-1`)?
fn valid_snapshot_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// One block range on its own connection in snapshot `snap`.
async fn worker_ids(
    opts: sqlx::postgres::PgConnectOptions,
    snap: String,
    sql: String,
    values: sea_query_binder::SqlxValues,
) -> Result<Vec<i64>, sqlx::Error> {
    use sqlx::Connection;
    let mut conn = sqlx::PgConnection::connect_with(&opts).await?;
    let mut tx = conn.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY").execute(&mut *tx).await?;
    sqlx::query(&format!("SET TRANSACTION SNAPSHOT '{snap}'")).execute(&mut *tx).await?;
    let ids: Vec<i64> = sqlx::query_scalar_with::<_, i64, _>(&sql, values).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    let _ = conn.close().await;
    Ok(ids)
}

/// For tests: hit set without cache, with exactly `workers` connections (without a
/// minimum size per range).
#[doc(hidden)]
pub async fn build_for_tests(
    db: &sqlx::PgPool,
    fold: Option<&UpperFold>,
    c: HitColumn,
    lookup: SqlLookup,
    value: &str,
    workers: usize,
) -> Result<Option<HitSet>, sqlx::Error> {
    let l = layout(db).await?;
    build_with(db, l, fold, c, lookup, value, Some(workers)).await
}

async fn build(
    db: &sqlx::PgPool,
    layout: Layout,
    fold: Option<&UpperFold>,
    c: HitColumn,
    lookup: SqlLookup,
    value: &str,
) -> Result<Option<HitSet>, sqlx::Error> {
    build_with(db, layout, fold, c, lookup, value, None).await
}

async fn build_with(
    db: &sqlx::PgPool,
    layout: Layout,
    fold: Option<&UpperFold>,
    c: HitColumn,
    lookup: SqlLookup,
    value: &str,
    fixed_workers: Option<usize>,
) -> Result<Option<HitSet>, sqlx::Error> {
    let (table, _, _) = source(layout, c);
    let mut tx = db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY").execute(&mut *tx).await?;
    // first query of the transaction = its snapshot; exported for the other connections
    let (snapshot, exported, blocks): (String, String, i64) = sqlx::query_as(
        "SELECT pg_current_snapshot()::text, pg_export_snapshot(), \
         (pg_relation_size($1::regclass) / current_setting('block_size')::int8)::int8",
    )
    .bind(table)
    .fetch_one(&mut *tx)
    .await?;
    let n = match fixed_workers {
        Some(n) => n.max(1),
        None => workers().min(usize::try_from(blocks / MIN_BLOCKS_PER_WORKER).unwrap_or(1).max(1)),
    };
    let mut ids: Option<Vec<i64>> = None;
    if n > 1 && valid_snapshot_id(&exported) {
        let opts = (*db.connect_options()).clone();
        let mut set = tokio::task::JoinSet::new();
        for (i, r) in ranges(blocks, n).into_iter().enumerate() {
            let (sql, values) = range_query(layout, fold, c, lookup, value, r);
            let fut = worker_ids(opts.clone(), exported.clone(), sql, values);
            set.spawn(async move { (i, fut.await) });
        }
        let mut parts: Vec<Option<Vec<i64>>> = vec![None; n];
        let mut failed = false;
        while let Some(res) = set.join_next().await {
            match res {
                Ok((i, Ok(v))) => {
                    if let Some(p) = parts.get_mut(i) {
                        *p = Some(v);
                    }
                }
                Ok((_, Err(e))) => {
                    tracing::warn!(error = %e, "text hit cache: parallel part failed, building in one query");
                    failed = true;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "text hit cache: parallel part panicked, building in one query");
                    failed = true;
                }
            }
        }
        if !failed && parts.iter().all(Option::is_some) {
            ids = Some(parts.into_iter().flatten().flatten().collect());
        }
    }
    let ids = match ids {
        Some(v) => v,
        None => {
            let (sql, values) = range_query(layout, fold, c, lookup, value, (None, None));
            sqlx::query_scalar_with::<_, i64, _>(&sql, values).fetch_all(&mut *tx).await?
        }
    };
    tx.commit().await?;
    Ok(HitSet::with_layout(&snapshot, &ids, layout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_query::PostgresQueryBuilder;

    #[test]
    fn snapshot_parsing_takes_low_32_bits() {
        assert_eq!(parse_snapshot("1983178:1983178:"), Some((1983178, vec![])));
        assert_eq!(parse_snapshot("10:20:12,15"), Some((20, vec![12, 15])));
        let epoch1 = (1u64 << 32) + 7;
        assert_eq!(parse_snapshot(&format!("{epoch1}:{epoch1}:")), Some((7, vec![])));
        assert_eq!(parse_snapshot("x:1:"), None);
        assert_eq!(parse_snapshot("1:2"), None);
    }

    #[test]
    fn bitmap_like_get_bit() {
        let h = HitSet::new("1:5:", &[0, 7, 8, 1000]).unwrap();
        assert!(h.contains(0) && h.contains(7) && h.contains(8) && h.contains(1000));
        assert!(!h.contains(1) && !h.contains(999) && !h.contains(1001) && !h.contains(-1));
        // get_bit: byte n/8, bit n%8 least significant first
        assert_eq!(h.bitmap[0], 0b1000_0001);
        assert_eq!(h.bitmap[1], 0b0000_0001);
        assert_eq!(h.bitmap.len(), 1000 / 8 + 1);
        assert_eq!(h.hits, 4);
        assert!(HitSet::new("1:5:", &[MAX_ID]).is_none());
        assert!(HitSet::new("1:5:", &[]).is_some());
    }

    #[test]
    fn cond_shape() {
        let h = HitSet::new("1:42:40,41", &[3]).unwrap();
        let e = h.cond(DOC, Expr::cust("TRUE"));
        let s = Query::select().expr(e).to_string(PostgresQueryBuilder);
        assert!(s.contains(r#"age("documents_document"."xmin") > age(('42')::text::xid)"#), "{s}");
        assert!(s.contains(r#"NOT ("documents_document"."xmin" = ANY(('{40,41}')::text::xid[]))"#), "{s}");
        assert!(s.contains("get_bit("), "{s}");
        assert!(s.contains(r#""documents_document"."id" < 8"#), "{s}");
        assert!(s.contains("ELSE (TRUE) END"), "{s}");
    }

    #[test]
    fn native_cond_reads_xmin_of_content_table() {
        let h = HitSet::for_tests_native("1:42:", &[3]).unwrap();
        assert!(h.native());
        let s = Query::select().expr(h.cond_native(DOC, "hit_ch0", Expr::cust("TRUE"))).to_string(PostgresQueryBuilder);
        assert!(s.contains(r#"CASE WHEN "hit_ch0"."document_id" IS NOT NULL THEN (TRUE) ELSE"#), "{s}");
        assert!(s.contains(&format!(r#"WHEN "{DOC}"."id" < 8"#)), "{s}");
        assert!(!s.contains("xmin"), "{s}");
        let c = h.changed_rows().to_string(PostgresQueryBuilder);
        assert!(c.contains(r#"FROM "paperrust_document_content" AS "hit_chg""#), "{c}");
        assert!(c.contains(r#"age("hit_chg"."xmin") > age(('42')::text::xid)"#) && c.contains("NOT "), "{c}");
    }

    #[test]
    fn block_ranges_cover_everything() {
        assert_eq!(ranges(100, 1), vec![(None, None)]);
        assert_eq!(ranges(100, 3), vec![(None, Some(33)), (Some(33), Some(66)), (Some(66), None)]);
        // each range joins onto the previous one
        let r = ranges(103_947, 8);
        assert_eq!(r.len(), 8);
        for w in r.windows(2) {
            assert_eq!(w[0].1, w[1].0);
        }
    }

    #[test]
    fn range_query_shape() {
        let (sql, _) = range_query(Layout::Native, None, HitColumn::Content, SqlLookup::IContains, "x", (Some(10), Some(20)));
        assert!(sql.contains(r#"FROM "paperrust_document_content" AS "t""#), "{sql}");
        assert!(sql.contains(r#""t"."document_id""#), "{sql}");
        assert!(sql.contains(r#""t"."ctid" >= '(10,0)'::tid"#) && sql.contains(r#""t"."ctid" < '(20,0)'::tid"#), "{sql}");
        let (sql, _) = range_query(Layout::Django, None, HitColumn::Title, SqlLookup::IContains, "x", (None, None));
        assert!(sql.contains(&format!(r#"FROM "{DOC}" AS "t""#)) && !sql.contains("ctid"), "{sql}");
    }

    #[test]
    fn upper_once_form() {
        let sql = |e: SimpleExpr| Query::select().expr(e).to_string(PostgresQueryBuilder);
        let u = || Expr::cust("u");
        let f = UpperFold::icu_default();
        // "rechnung": cheap form first, replace chain only for rows with a special character
        let s = sql(upper_once_cond(Some(&f), u, SqlLookup::IContains, "rechnung").unwrap());
        assert!(s.starts_with("SELECT ((u LIKE '%RECHNUNG%') OR (("), "{s}");
        assert!(s.contains("replace(") && s.contains("u LIKE '%"), "{s}");
        // many special characters: no collation UPPER, all replace() (only for rows with characters)
        let s = sql(upper_once_cond(Some(&f), u, SqlLookup::IContains, "beitrag").unwrap());
        assert!(!s.contains("UPPER("), "{s}");
        assert!(s.matches("replace(").count() > pl_docfilter::textfold::MAX_REPLACES, "{s}");
        // digits only: no special characters affected
        assert_eq!(sql(upper_once_cond(Some(&f), u, SqlLookup::IContains, "2024").unwrap()), "SELECT u LIKE '%2024%'");
        assert_eq!(sql(upper_once_cond(Some(&f), u, SqlLookup::IExact, "ab").unwrap()).matches("= 'AB'").count(), 2);
        // without folding / non-ASCII: previous form
        assert!(upper_once_cond(None, u, SqlLookup::IContains, "abc").is_none());
        assert!(upper_once_cond(Some(&f), u, SqlLookup::IContains, "größe").is_none());
        // build query: subquery with OFFSET 0, block range inside
        let (q, _) = range_query(Layout::Native, Some(&f), HitColumn::Content, SqlLookup::IContains, "rechnung", (Some(1), Some(2)));
        assert!(q.contains(r#"UPPER(("t"."content")::text COLLATE "C") AS "u""#) && q.contains("OFFSET"), "{q}");
        assert!(q.contains(r#""t"."ctid" >= '(1,0)'::tid"#), "{q}");
    }

    #[test]
    fn snapshot_ids() {
        assert!(valid_snapshot_id("00000003-0000001B-1"));
        assert!(!valid_snapshot_id(""));
        assert!(!valid_snapshot_id("1'; DROP"));
    }
}
