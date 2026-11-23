//! SQL builder for the document filter AST (ARCHITECTURE-V2 §5 **L1**).
//!
//! From a [`DocFilter`] (v1 via `pl_docfilter::ast`, v2 via [`crate::documents`]) **one**
//! query arises: `SELECT "documents_document"."id" FROM "documents_document" WHERE <base>
//! AND <predicate> AND <visibility> ORDER BY …`:
//!
//! * Base like `DocumentViewSet.get_queryset`: root documents outside the trash.
//! * Multi-valued relations (tags, custom field instances) as correlated `EXISTS` – no
//!   JOINs, no `DISTINCT`, `COUNT(*)` stays correct.
//! * Visibility ([`Visibility`]) **set-based directly on the outer row** instead of
//!   `id IN (permitted_document_ids …)` with a self-JOIN over `root_document`: for a
//!   root document `authorizing_owner = owner_id` and `authorizing_id = id`, the permission
//!   subquery (guardian) is uncorrelated and is hashed once. Same set as
//!   `PermittedObjectsFilter` (proof in the contract). For L5 (permissions → `paperrust_grant`) only
//!   [`Visibility::root_row`] is replaced.
//! * Sorting like `DocumentsOrderingFilter` (LEFT JOIN for names/note count, no tiebreaker on
//!   explicit sorting, PostgreSQL NULL order = Django).
//! * Substring search over title/content in the result-equal folded form, when the DB
//!   is calibrated (`pl_docfilter::textfold`).
//!
//! The only query path of the document list (the former switch `PAPERRUST_QUERY_ENGINE=legacy`
//! has been removed since data model stage 7, docs/DATAMODEL-V2.md §6). Contract:
//! docs/contract/pl-docfilter.md, section "L1".

use std::sync::Arc;

use pl_authz::User;
use pl_authz::models::DOCUMENT;
use pl_docfilter::sql::{
    CF, CFI, CORRESPONDENT, Cond, DOC, DOC_TAGS, DOCTYPE, NOTE, STORAGEPATH, TAG, TextLookup as SqlLookup, col, doc_col, effective_content,
    exists, scalar, select_one_from, text_lookup,
};
use pl_docfilter::textfold::{UpperFold, folded_lookup};
use pl_domain::filter::{
    CfColumn, CfCond, CfSortValue, CfTest, CfValue, CmpOp, DatePart, DocFilter, InstanceCond, NamedRel, NumCol, PartTest, Pred, Sort,
    SortKey, TagCond, TextCol, TextLookup, TsCol,
};
use sea_query::{Alias, Expr, ExprTrait, JoinType, Order, Query, SelectStatement, SimpleExpr, Value};

// ------------------------------------------------------------------------- Visibility

/// Who sees which root documents (`view_document`). Exchange point for L5.
#[derive(Clone)]
pub enum Visibility {
    /// active superuser (or no restriction)
    All,
    /// inactive user
    Nothing,
    /// own, ownerless and documents shared via object permission (user/group)
    Grants(Box<User>),
}

impl Visibility {
    /// `PermittedObjectsFilter` (documents, `view_document`) for `user`.
    pub fn for_user(user: &User) -> Self {
        if !user.is_active {
            Visibility::Nothing
        } else if user.is_superuser {
            Visibility::All
        } else {
            Visibility::Grants(Box::new(user.clone()))
        }
    }

    /// Condition on a **root document** row with alias `alias` (`None` = all visible).
    ///
    /// Equivalent to `alias.id IN permitted_document_ids(user, view_document)` as long as
    /// `alias.root_document_id IS NULL` (then `authorizing_owner = owner_id`,
    /// `authorizing_id = id`); the manager's trash filter is the caller's business.
    pub fn root_row(&self, alias: &str) -> Option<SimpleExpr> {
        match self {
            Visibility::All => None,
            Visibility::Nothing => Some(Expr::cust("FALSE")),
            Visibility::Grants(user) => {
                // L5: `granted_key_cond` chooses guardian (`legacy`) or `paperrust_grant` (`grants`).
                let granted = pl_authz::objects::granted_key_cond(user, &DOCUMENT, "view_document", col(alias, "id"));
                Some(col(alias, "owner_id").eq(user.id).or(col(alias, "owner_id").is_null()).or(granted))
            }
        }
    }

    /// SQL text of a subquery of the visible root documents outside the trash
    /// (for side-table queries, `pl_store::visible_ids_sql`).
    pub fn visible_ids_sql(&self) -> String {
        let mut q = base_select();
        if let Some(v) = self.root_row(DOC) {
            q.and_where(v);
        }
        q.to_string(sea_query::PostgresQueryBuilder)
    }
}

// ------------------------------------------------------------------------- Builder

/// Base like `Document.objects.filter(root_document__isnull=True)` (manager: not deleted).
pub fn base_select() -> SelectStatement {
    let mut q = Query::select();
    q.column((Alias::new(DOC), Alias::new("id")))
        .from(Alias::new(DOC))
        .and_where(doc_col("root_document_id").is_null())
        .and_where(doc_col("deleted_at").is_null());
    q
}

/// `true` if the predicate contains a text search over title/content (then the
/// folding calibration is worthwhile).
pub fn wants_fold(p: &Pred) -> bool {
    match p {
        Pred::Text { col: TextCol::Title | TextCol::Content, .. } => true,
        Pred::And(v) | Pred::Or(v) => v.iter().any(wants_fold),
        Pred::Not(p) | Pred::Complement(p) => wants_fold(p),
        _ => false,
    }
}

/// Calibrated folding of the DB, if the predicate needs it.
pub async fn fold_for(db: &sqlx::PgPool, p: &Pred) -> Option<Arc<UpperFold>> {
    if wants_fold(p) { pl_docfilter::FilterDb::upper_fold(db).await } else { None }
}

/// Names of the trigram indexes (migration `pl_store.0002_paperrust_doc_trgm`).
pub const TRGM_INDEXES: [&str; 2] = ["paperrust_doc_title_trgm", "paperrust_doc_content_trgm"];

/// Minimum length of a contiguous word part (letters/digits) in the search text from which
/// pg_trgm can extract trigrams from the `LIKE` pattern. Shorter search texts: scan as before
/// (a GIN run without trigrams would read the whole index).
pub const TRGM_MIN_RUN: usize = 3;

/// Trigram indexes present and valid? Cached per database (60 s).
pub async fn trigram_ready(db: &sqlx::PgPool) -> bool {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, bool)>>> = OnceLock::new();
    let o = db.connect_options();
    let key = format!("{}:{}/{}", o.get_host(), o.get_port(), o.get_database().unwrap_or(""));
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, v)) = cache.lock().ok().and_then(|m| m.get(&key).copied())
        && at.elapsed() < Duration::from_secs(60)
    {
        return v;
    }
    let names: Vec<String> = TRGM_INDEXES.iter().map(|s| s.to_string()).collect();
    let v = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid \
          WHERE c.relname = ANY($1) AND i.indisvalid AND i.indisready",
    )
    .bind(&names)
    .fetch_one(db)
    .await
    .map(|n| n == TRGM_INDEXES.len() as i64)
    .unwrap_or(false);
    if let Ok(mut m) = cache.lock() {
        m.insert(key, (Instant::now(), v));
    }
    v
}

/// Highest actual hit count per title/content leaf up to which the trigram form is used.
/// Above it, it is slower than the scan: every candidate is re-checked with the collation `UPPER()`
/// (~80 µs per row at 1 million; "rechnung" 39 s → 80 s), and the `IN (…)` candidate
/// set prevents the early abort with `LIMIT` and the use of more selective filters (v2 text
/// 59 ms → 96 s; dashboard widget with correspondent + "Beitrag" 0.5 s → 12 s).
///
/// Counting is real (`LIMIT` + 1, via the trigram index: rare words ~20–60 ms, frequent
/// 0.1–0.6 s): the planner estimate is useless, ANALYZE leaves wide values (> 1 kB, i.e. almost
/// every content) out of the statistics ("Beitrag" estimated 100, actually 70,310).
pub const TRGM_MAX_CANDIDATES: i64 = 1000;

/// Validity of a counted hit count in the cache.
const TRGM_PROBE_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// Trigram form sensible for the predicate (contains a title/content search), indexes ready and
/// every index-capable leaf rare enough ([`TRGM_MAX_CANDIDATES`])?
pub async fn trigram_for(db: &sqlx::PgPool, p: &Pred) -> bool {
    if !(wants_fold(p) && trigram_ready(db).await) {
        return false;
    }
    let mut leaves = Vec::new();
    trigram_leaves(p, &mut leaves);
    for (c, lookup, value) in leaves {
        match trigram_probe(db, c, lookup, value).await {
            Some(n) if n <= TRGM_MAX_CANDIDATES => {}
            _ => return false,
        }
    }
    true
}

/// Title/content leaves that could run via the trigram index.
fn trigram_leaves<'p>(p: &'p Pred, out: &mut Vec<(TextCol, TextLookup, &'p str)>) {
    match p {
        Pred::Text { col: c @ (TextCol::Title | TextCol::Content), lookup, value } if trigram_eligible(*lookup, value) => {
            out.push((*c, *lookup, value.as_str()));
        }
        Pred::And(v) | Pred::Or(v) => v.iter().for_each(|x| trigram_leaves(x, out)),
        Pred::Not(x) | Pred::Complement(x) => trigram_leaves(x, out),
        _ => {}
    }
}

/// Rows with `UPPER(<column>::text) LIKE UPPER(p)`, counted up to [`TRGM_MAX_CANDIDATES`] + 1
/// (all rows of the table, like the candidate set). Cached per database, column and pattern for
/// [`TRGM_PROBE_TTL`].
async fn trigram_probe(db: &sqlx::PgPool, c: TextCol, lookup: TextLookup, value: &str) -> Option<i64> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;
    type Cache = Mutex<HashMap<String, (Instant, i64)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let column = if matches!(c, TextCol::Title) { "title" } else { "content" };
    let o = db.connect_options();
    let key = format!("{}:{}/{}|{column}|{lookup:?}|{value}", o.get_host(), o.get_port(), o.get_database().unwrap_or(""));
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, n)) = cache.lock().ok().and_then(|m| m.get(&key).copied())
        && at.elapsed() < TRGM_PROBE_TTL
    {
        return Some(n);
    }
    let inner = Query::select()
        .expr(Expr::val(1))
        .from(Alias::new(DOC))
        .and_where(text_lookup(doc_col(column), sql_lookup(lookup), value))
        .limit((TRGM_MAX_CANDIDATES + 1) as u64)
        .to_string(sea_query::PostgresQueryBuilder);
    let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM ({inner}) AS pl_trgm_probe")).fetch_one(db).await.ok()?;
    if let Ok(mut m) = cache.lock() {
        if m.len() > 1000 {
            m.retain(|_, (at, _)| at.elapsed() < TRGM_PROBE_TTL);
            if m.len() > 1000 {
                m.clear();
            }
        }
        m.insert(key, (Instant::now(), n));
    }
    Some(n)
}

/// Can `lookup` on `value` run via the trigram index? (`LIKE` lookups with at least one
/// word part of [`TRGM_MIN_RUN`] letters/digits.)
pub fn trigram_eligible(lookup: TextLookup, value: &str) -> bool {
    if matches!(lookup, TextLookup::IExact) {
        return false;
    }
    let mut run = 0usize;
    for c in value.chars() {
        if c.is_alphanumeric() {
            run += 1;
            if run >= TRGM_MIN_RUN {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

/// Stored hit set of a title/content leaf ([`crate::textcache`]).
#[derive(Clone)]
pub struct TextHit {
    pub col: TextCol,
    pub lookup: TextLookup,
    pub value: String,
    pub set: Arc<crate::textcache::HitSet>,
}

/// Does the predicate consist only of title/content searches (AND/OR/NOT)? Then for the total count
/// every visible row has to be checked anyway – exactly there the hit cache helps. With
/// further conditions (correspondent, tags …) the previous path stays, because the database
/// then often has to check only a few rows and a determination over all rows would be more expensive.
pub fn text_only(p: &Pred) -> bool {
    match p {
        Pred::Text { col: TextCol::Title | TextCol::Content, .. } => true,
        Pred::And(v) | Pred::Or(v) => !v.is_empty() && v.iter().all(text_only),
        Pred::Not(x) | Pred::Complement(x) => text_only(x),
        _ => false,
    }
}

/// Hit sets for all title/content leaves of `p` if [`text_only`] applies and the
/// hit cache is switched on (otherwise empty). For queries that count all hits anyway
/// (v1 list, selection "all") – not for the v2 list without a count, which can abort
/// after the first hits.
pub async fn text_hits_for(db: &sqlx::PgPool, fold: Option<&UpperFold>, p: &Pred) -> Vec<TextHit> {
    if !text_only(p) || !crate::textcache::enabled() {
        return Vec::new();
    }
    let mut leaves = Vec::new();
    text_leaves(p, &mut leaves);
    let mut out: Vec<TextHit> = Vec::new();
    for (c, lookup, value) in leaves {
        if out.iter().any(|h| h.col == c && h.lookup == lookup && h.value == value) {
            continue;
        }
        let hc = match c {
            TextCol::Title => crate::textcache::HitColumn::Title,
            _ => crate::textcache::HitColumn::Content,
        };
        if let Some(set) = crate::textcache::hits(db, fold, hc, sql_lookup(lookup), value).await {
            out.push(TextHit { col: c, lookup, value: value.to_string(), set });
        }
    }
    out
}

fn text_leaves<'p>(p: &'p Pred, out: &mut Vec<(TextCol, TextLookup, &'p str)>) {
    match p {
        Pred::Text { col: c @ (TextCol::Title | TextCol::Content), lookup, value } => {
            out.push((*c, *lookup, value.as_str()));
        }
        Pred::And(v) | Pred::Or(v) => v.iter().for_each(|x| text_leaves(x, out)),
        Pred::Not(x) | Pred::Complement(x) => text_leaves(x, out),
        _ => {}
    }
}

/// Builds SQL from the AST.
pub struct Builder<'a> {
    pub vis: &'a Visibility,
    pub fold: Option<&'a UpperFold>,
    /// Use trigram indexes for title/content (see [`Builder::with_trigram`]).
    pub trgm: bool,
    /// Hit sets for title/content leaves (see [`Builder::with_hits`]).
    pub hits: Vec<TextHit>,
}

impl<'a> Builder<'a> {
    pub fn new(vis: &'a Visibility, fold: Option<&'a UpperFold>) -> Self {
        Builder { vis, fold, trgm: false, hits: Vec::new() }
    }

    /// Check title/content leaves with a stored hit set via the bitmask, result-
    /// equal to the text condition ([`crate::textcache`]): unchanged rows from the set, new/
    /// changed ones on the text.
    pub fn with_hits(mut self, hits: Vec<TextHit>) -> Self {
        self.hits = hits;
        self
    }

    fn hit_for(&self, c: TextCol, lookup: TextLookup, value: &str) -> Option<(usize, &crate::textcache::HitSet)> {
        self.hits.iter().position(|h| h.col == c && h.lookup == lookup && h.value == value).map(|i| (i, self.hits[i].set.as_ref()))
    }

    /// Alias under which the changed rows of hit set `i` are joined (own data model).
    fn changed_alias(i: usize) -> String {
        format!("hit_ch{i}")
    }

    /// Title/content leaf via the hit set (outer row `"documents_document"`):
    /// * Title: own row → set or re-check on the title.
    /// * Content = `effective_content`: roots with a live version (rare, hashed subquery)
    ///   check the effective content directly; all others their own content → set or
    ///   re-check on the own content.
    ///
    /// Own data model (set via `paperrust_document_content`): changed rows come from
    /// the join in [`Builder::select_unordered`]; the text is read only for them (and for
    /// roots with a version) individually from the text table – the query does not touch
    /// the view's text column, its join is dropped.
    fn text_hit(&self, i: usize, h: &crate::textcache::HitSet, c: TextCol, lookup: TextLookup, value: &str) -> SimpleExpr {
        let l = sql_lookup(lookup);
        match c {
            TextCol::Title => h.cond(DOC, folded_lookup(self.fold, doc_col("title"), l, value)),
            _ => {
                let (own, eff) = if h.native() {
                    let own_text = || {
                        scalar(
                            Query::select()
                                .column((Alias::new("hit_t"), Alias::new("content")))
                                .from_as(Alias::new(crate::textcache::NATIVE_CONTENT_TABLE), Alias::new("hit_t"))
                                .and_where(col("hit_t", "document_id").eq(doc_col("id")))
                                .to_owned(),
                        )
                    };
                    (
                        h.cond_native(DOC, &Self::changed_alias(i), folded_lookup(self.fold, own_text(), l, value)),
                        folded_lookup(self.fold, effective_content_with(DOC, own_text()), l, value),
                    )
                } else {
                    (
                        h.cond(DOC, folded_lookup(self.fold, doc_col("content"), l, value)),
                        folded_lookup(self.fold, effective_content(), l, value),
                    )
                };
                Expr::cust_with_exprs(
                    format!(
                        r#"(CASE WHEN "{DOC}"."id" IN (SELECT "hit_v"."root_document_id" FROM "{DOC}" AS "hit_v" WHERE "hit_v"."root_document_id" IS NOT NULL AND "hit_v"."deleted_at" IS NULL) THEN ($1) ELSE ($2) END)"#
                    ),
                    [eff, own],
                )
            }
        }
    }

    /// Substring search over title/content as a candidate set from the trigram indexes
    /// (`pl_store.0002_paperrust_doc_trgm`), result-equal to the `UPPER(..) LIKE` form (see
    /// [`Builder::text_candidates`]). Only with `trigram_ready`.
    pub fn with_trigram(mut self, on: bool) -> Self {
        self.trgm = on;
        self
    }

    /// Complete query: base, predicate, visibility, sorting.
    pub fn select(&self, f: &DocFilter) -> SelectStatement {
        let mut q = self.select_unordered(&f.pred);
        apply_sort(&mut q, &f.sort);
        q
    }

    /// Base, predicate and visibility, without ORDER BY.
    pub fn select_unordered(&self, p: &Pred) -> SelectStatement {
        let mut q = base_select();
        for (i, h) in self.hits.iter().enumerate().filter(|(_, h)| h.set.native()) {
            // Hit set in the own data model: text rows changed since the snapshot
            // (at most one per document, the join does not multiply any row)
            let alias = Self::changed_alias(i);
            q.join_subquery(
                JoinType::LeftJoin,
                h.set.changed_rows(),
                Alias::new(alias.as_str()),
                Expr::col((Alias::new(alias.as_str()), Alias::new("document_id"))).equals((Alias::new(DOC), Alias::new("id"))),
            );
        }
        self.pred(p).apply_to(&mut q);
        if let Some(v) = self.vis.root_row(DOC) {
            q.and_where(v);
        }
        q
    }

    /// Condition on the outer row `"documents_document"`.
    pub fn pred(&self, p: &Pred) -> Cond {
        match p {
            Pred::True => Cond::True,
            Pred::False => Cond::False,
            Pred::And(v) => v.iter().fold(Cond::True, |acc, x| acc.and(self.pred(x))),
            Pred::Or(v) if self.trgm && v.len() > 1 && v.iter().all(|x| self.trgm_arm(x).is_some()) => {
                // `title_content` = title OR content → one candidate set (UNION ALL)
                let arms: Vec<(TextCol, TextLookup, &str)> = v.iter().filter_map(|x| self.trgm_arm(x)).collect();
                Cond::Expr(self.text_candidates(&arms))
            }
            Pred::Or(v) if !self.hits.is_empty() => {
                // Leaves with a hit set first (OR is commutative): the bitmask check is
                // cheap and, on a hit, saves the text comparison of the remaining branches
                // (`title_content`: title comparison only for rows without a content hit).
                let cached = |x: &Pred| matches!(x, Pred::Text { col, lookup, value } if self.hit_for(*col, *lookup, value).is_some());
                let (first, rest): (Vec<&Pred>, Vec<&Pred>) = v.iter().partition(|x| cached(x));
                first.into_iter().chain(rest).fold(Cond::False, |acc, x| acc.or(self.pred(x)))
            }
            Pred::Or(v) => v.iter().fold(Cond::False, |acc, x| acc.or(self.pred(x))),
            Pred::Not(x) => self.pred(x).negate(),
            Pred::Complement(x) => match self.pred(x) {
                Cond::True => Cond::False,
                Cond::False => Cond::True,
                Cond::Expr(e) => Cond::Expr(Expr::cust_with_exprs("($1) IS NOT TRUE", [e])),
            },
            Pred::Num { col, op, value } => Cond::Expr(cmp(doc_col(num_col(*col)), *op, Expr::val(*value).into())),
            Pred::NumIn { col, values } => {
                if values.is_empty() {
                    Cond::False
                } else {
                    Cond::Expr(doc_col(num_col(*col)).is_in(values.iter().copied()))
                }
            }
            Pred::IsNull { col, is_null } => {
                let c = doc_col(num_col(*col));
                Cond::Expr(if *is_null { c.is_null() } else { c.is_not_null() })
            }
            Pred::FkNot { col, id } => {
                let c = num_col(*col);
                Cond::Expr(doc_col(c).eq(*id).and(doc_col(c).is_not_null()).not())
            }
            Pred::Text { col, lookup, value } => Cond::Expr(self.text(*col, *lookup, value)),
            Pred::RelName { rel, lookup, value } => {
                let (table, fk) = rel_table(*rel);
                let sub = Query::select()
                    .column((Alias::new("fkn"), Alias::new("id")))
                    .from_as(Alias::new(table), Alias::new("fkn"))
                    .and_where(text_lookup(col("fkn", "name"), sql_lookup(*lookup), value))
                    .to_owned();
                Cond::Expr(doc_col(fk).in_subquery(sub))
            }
            Pred::Created { op, date } => Cond::Expr(cmp(doc_col("created"), *op, Expr::val(*date).into())),
            Pred::CreatedBetween { from, to } => Cond::Expr(doc_col("created").between(Expr::val(*from), Expr::val(*to))),
            Pred::CreatedPart { part, op, value } => Cond::Expr(cmp(extract(*part, doc_col("created")), *op, Expr::val(*value).into())),
            Pred::Ts { col, op, at } => Cond::Expr(cmp(doc_col(ts_col(*col)), *op, Expr::val(*at).into())),
            Pred::TsNotNull { col } => Cond::Expr(doc_col(ts_col(*col)).is_not_null()),
            Pred::TsPart { col, part, tz, op, value } => {
                let local = Expr::cust_with_exprs("$1 AT TIME ZONE $2", [doc_col(ts_col(*col)), Expr::val(tz.as_str()).into()]);
                Cond::Expr(cmp(extract(*part, local), *op, Expr::val(*value).into()))
            }
            Pred::Tag(t) => Cond::Expr(tag_exists(self.tag_cond(t))),
            Pred::Instance(c) => Cond::Expr(instance_exists(instance_cond(c))),
            Pred::CustomField { field_id, test } => match test {
                None => Cond::Expr(cf_exists(*field_id, None)),
                Some(c) => match cf_cond(c) {
                    Cond::False => Cond::False,
                    Cond::True => Cond::Expr(cf_exists(*field_id, None)),
                    Cond::Expr(e) => Cond::Expr(cf_exists(*field_id, Some(e))),
                },
            },
            Pred::HasDuplicates(b) => {
                let e = self.duplicates_exists();
                Cond::Expr(if *b { pl_docfilter::filterset::duplicate_candidates().and(e) } else { e.not() })
            }
            Pred::SharedBy(uid) => Cond::Expr(doc_col("owner_id").eq(*uid).and(pl_docfilter::perms::document_is_shared())),
            Pred::Folder(c) => Cond::Expr(pl_docfilter::sql::folder_exists(c)),
        }
    }

    /// Title/content leaf that can run via the trigram index.
    fn trgm_arm<'p>(&self, p: &'p Pred) -> Option<(TextCol, TextLookup, &'p str)> {
        match p {
            Pred::Text { col: c @ (TextCol::Title | TextCol::Content), lookup, value } if self.trgm && trigram_eligible(*lookup, value) => {
                Some((*c, *lookup, value.as_str()))
            }
            _ => None,
        }
    }

    /// `D.id IN (<candidates>)` for substring searches over title/content (OR-combined), exactly
    /// equal to `UPPER(<column>) LIKE UPPER(p)` or its folded form:
    ///
    /// * Title: `SELECT t.id … WHERE UPPER(t.title::text) LIKE UPPER(p)` – the same condition on
    ///   the same row (Django form, index `paperrust_doc_title_trgm`).
    /// * Content = `effective_content` = content of the newest non-deleted version, otherwise the
    ///   own. (a) Documents **without** a live version: own content, `UPPER(c.content::text)
    ///   LIKE UPPER(p)` via `paperrust_doc_content_trgm`; (b) documents **with** a live version
    ///   (rare, via the index on `root_document_id`): the previous condition on the
    ///   effective content.
    ///
    /// pg_trgm only delivers candidates and re-checks every row with the `LIKE` (lossy +
    /// recheck), so the result is the same condition; the candidate set also contains
    /// trash/version rows, which the base and visibility of the outer row continue to filter.
    fn text_candidates(&self, arms: &[(TextCol, TextLookup, &str)]) -> SimpleExpr {
        let mut parts: Vec<String> = Vec::new();
        let mut exprs: Vec<SimpleExpr> = Vec::new();
        let ph = |e: SimpleExpr, exprs: &mut Vec<SimpleExpr>| -> String {
            exprs.push(e);
            format!("${}", exprs.len())
        };
        for (c, lookup, value) in arms {
            let l = sql_lookup(*lookup);
            match c {
                TextCol::Title => {
                    let cond = ph(text_lookup(col("trg_t", "title"), l, value), &mut exprs);
                    parts.push(format!(r#"SELECT "trg_t"."id" FROM "{DOC}" AS "trg_t" WHERE {cond}"#));
                }
                _ => {
                    let own = ph(text_lookup(col("trg_c", "content"), l, value), &mut exprs);
                    parts.push(format!(
                        r#"SELECT "trg_c"."id" FROM "{DOC}" AS "trg_c" WHERE {own} AND "trg_c"."root_document_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "{DOC}" AS "trg_cv" WHERE "trg_cv"."root_document_id" = "trg_c"."id" AND "trg_cv"."deleted_at" IS NULL)"#
                    ));
                    let eff = ph(folded_lookup(self.fold, effective_content_of("trg_r"), l, value), &mut exprs);
                    parts.push(format!(
                        r#"SELECT "trg_r"."id" FROM "{DOC}" AS "trg_r" WHERE "trg_r"."id" IN (SELECT "trg_rv"."root_document_id" FROM "{DOC}" AS "trg_rv" WHERE "trg_rv"."root_document_id" IS NOT NULL AND "trg_rv"."deleted_at" IS NULL) AND {eff}"#
                    ));
                }
            }
        }
        let sql = format!(r#""{DOC}"."id" IN ({})"#, parts.join(" UNION ALL "));
        Expr::cust_with_exprs(sql, exprs)
    }

    fn text(&self, c: TextCol, lookup: TextLookup, value: &str) -> SimpleExpr {
        if let Some((i, h)) = self.hit_for(c, lookup, value) {
            return self.text_hit(i, h, c, lookup, value);
        }
        if let Some(arm) = self.trgm_arm(&Pred::Text { col: c, lookup, value: value.to_string() }) {
            return self.text_candidates(&[(arm.0, arm.1, value)]);
        }
        let l = sql_lookup(lookup);
        match c {
            TextCol::Title => folded_lookup(self.fold, doc_col("title"), l, value),
            TextCol::Content => folded_lookup(self.fold, effective_content(), l, value),
            TextCol::OriginalFilename => text_lookup(doc_col("original_filename"), l, value),
            TextCol::Checksum => text_lookup(doc_col("checksum"), l, value),
            TextCol::MimeType => text_lookup(doc_col("mime_type"), l, value),
        }
    }

    fn tag_cond(&self, t: &TagCond) -> Option<SimpleExpr> {
        match t {
            TagCond::Any => None,
            TagCond::Is(id) => Some(col("dt", "tag_id").eq(*id)),
            TagCond::In(ids) => Some(col("dt", "tag_id").is_in(ids.iter().copied())),
            TagCond::Name(lookup, value) => {
                let sub = Query::select()
                    .column((Alias::new("tn"), Alias::new("id")))
                    .from_as(Alias::new(TAG), Alias::new("tn"))
                    .and_where(text_lookup(col("tn", "name"), sql_lookup(*lookup), value))
                    .to_owned();
                Some(col("dt", "tag_id").in_subquery(sub))
            }
            TagCond::Inbox => {
                let sub = Query::select()
                    .column((Alias::new("it"), Alias::new("id")))
                    .from_as(Alias::new(TAG), Alias::new("it"))
                    .and_where(col("it", "is_inbox_tag").eq(true))
                    .to_owned();
                Some(col("dt", "tag_id").in_subquery(sub))
            }
        }
    }

    /// Visible duplicate (`permitted_document_ids(user, include_deleted=True)`, roots only):
    /// visibility directly on the duplicate row.
    fn duplicates_exists(&self) -> SimpleExpr {
        let mut sel = select_one_from(DOC, "dup");
        sel.and_where(col("dup", "root_document_id").is_null()).and_where(col("dup", "id").eq(doc_col("id")).not());
        if let Some(v) = self.vis.root_row("dup") {
            sel.and_where(v);
        }
        let nonempty_archive = || Expr::cust_with_exprs("NULLIF($1, '')", [doc_col("archive_checksum")]);
        sel.and_where(
            col("dup", "checksum")
                .eq(doc_col("checksum"))
                .or(col("dup", "checksum").eq(nonempty_archive()))
                .or(col("dup", "archive_checksum").eq(doc_col("checksum")))
                .or(col("dup", "archive_checksum").eq(nonempty_archive())),
        );
        exists(sel)
    }
}

/// `effective_content` (`pl_docfilter::sql::effective_content`) for a row with alias `alias`.
fn effective_content_of(alias: &str) -> SimpleExpr {
    effective_content_with(alias, col(alias, "content"))
}

/// Like [`effective_content_of`], the row's own content as expression `own`.
fn effective_content_with(alias: &str, own: SimpleExpr) -> SimpleExpr {
    let sub = Query::select()
        .column((Alias::new("ecx_v"), Alias::new("content")))
        .from_as(Alias::new(DOC), Alias::new("ecx_v"))
        .and_where(col("ecx_v", "root_document_id").eq(col(alias, "id")))
        .and_where(col("ecx_v", "deleted_at").is_null())
        .order_by_with_nulls((Alias::new("ecx_v"), Alias::new("version_index")), Order::Desc, sea_query::NullOrdering::Last)
        .order_by((Alias::new("ecx_v"), Alias::new("id")), Order::Desc)
        .limit(1)
        .to_owned();
    Expr::cust_with_exprs("COALESCE(($1), $2)", [scalar(sub), own])
}

fn sql_lookup(l: TextLookup) -> SqlLookup {
    match l {
        TextLookup::IContains => SqlLookup::IContains,
        TextLookup::IStartsWith => SqlLookup::IStartsWith,
        TextLookup::IEndsWith => SqlLookup::IEndsWith,
        TextLookup::IExact => SqlLookup::IExact,
    }
}

fn num_col(c: NumCol) -> &'static str {
    match c {
        NumCol::Id => "id",
        NumCol::Asn => "archive_serial_number",
        NumCol::Correspondent => "correspondent_id",
        NumCol::DocumentType => "document_type_id",
        NumCol::StoragePath => "storage_path_id",
        NumCol::Owner => "owner_id",
    }
}

fn ts_col(c: TsCol) -> &'static str {
    match c {
        TsCol::Added => "added",
        TsCol::Modified => "modified",
    }
}

fn rel_table(r: NamedRel) -> (&'static str, &'static str) {
    match r {
        NamedRel::Correspondent => (CORRESPONDENT, "correspondent_id"),
        NamedRel::DocumentType => (DOCTYPE, "document_type_id"),
        NamedRel::StoragePath => (STORAGEPATH, "storage_path_id"),
    }
}

fn cmp(lhs: SimpleExpr, op: CmpOp, rhs: SimpleExpr) -> SimpleExpr {
    match op {
        CmpOp::Eq => lhs.eq(rhs),
        CmpOp::Gt => lhs.gt(rhs),
        CmpOp::Gte => lhs.gte(rhs),
        CmpOp::Lt => lhs.lt(rhs),
        CmpOp::Lte => lhs.lte(rhs),
    }
}

/// `EXTRACT` like Django on PostgreSQL (`week_day` = `DOW + 1`).
fn extract(part: DatePart, e: SimpleExpr) -> SimpleExpr {
    let tpl = match part {
        DatePart::WeekDay => "EXTRACT(DOW FROM $1) + 1",
        DatePart::IsoWeekDay => "EXTRACT(ISODOW FROM $1)",
        DatePart::IsoYear => "EXTRACT(ISOYEAR FROM $1)",
        DatePart::Year => "EXTRACT(YEAR FROM $1)",
        DatePart::Month => "EXTRACT(MONTH FROM $1)",
        DatePart::Day => "EXTRACT(DAY FROM $1)",
        DatePart::Week => "EXTRACT(WEEK FROM $1)",
        DatePart::Quarter => "EXTRACT(QUARTER FROM $1)",
    };
    Expr::cust_with_exprs(tpl, [e])
}

/// `EXISTS (SELECT 1 FROM documents_document_tags dt WHERE dt.document_id = doc.id AND …)`
fn tag_exists(cond: Option<SimpleExpr>) -> SimpleExpr {
    let mut sel = select_one_from(DOC_TAGS, "dt");
    sel.and_where(col("dt", "document_id").eq(doc_col("id")));
    if let Some(c) = cond {
        sel.and_where(c);
    }
    exists(sel)
}

/// `EXISTS (SELECT 1 FROM documents_customfieldinstance ci WHERE ci.document_id = doc.id AND …)`
/// (incl. soft-deleted instances, like the JOIN in upstream)
fn instance_exists(cond: Option<SimpleExpr>) -> SimpleExpr {
    let mut sel = select_one_from(CFI, "ci");
    sel.and_where(col("ci", "document_id").eq(doc_col("id")));
    if let Some(c) = cond {
        sel.and_where(c);
    }
    exists(sel)
}

fn instance_cond(c: &InstanceCond) -> Option<SimpleExpr> {
    match c {
        InstanceCond::Any => None,
        InstanceCond::Field(id) => Some(col("ci", "field_id").eq(*id)),
        InstanceCond::FieldIn(ids) => Some(col("ci", "field_id").is_in(ids.iter().copied())),
        InstanceCond::IContains { value, select_option_ids } => {
            let lk = |c: SimpleExpr| text_lookup(c, SqlLookup::IContains, value);
            let field_names = Query::select()
                .column((Alias::new("cfn"), Alias::new("id")))
                .from_as(Alias::new(CF), Alias::new("cfn"))
                .and_where(lk(col("cfn", "name")))
                .to_owned();
            let mut any = col("ci", "field_id").in_subquery(field_names);
            for c in
                ["value_text", "value_bool", "value_int", "value_float", "value_date", "value_url", "value_monetary", "value_document_ids"]
            {
                any = any.or(lk(col("ci", c)));
            }
            if !select_option_ids.is_empty() {
                any = any.or(col("ci", "value_select").is_in(select_option_ids.iter().cloned()));
            }
            Some(any.or(lk(col("ci", "value_long_text"))))
        }
    }
}

/// `EXISTS (SELECT 1 FROM documents_customfieldinstance cq WHERE cq.document_id = doc.id AND
/// cq.field_id = <id> [AND cond])`
fn cf_exists(field_id: i64, cond: Option<SimpleExpr>) -> SimpleExpr {
    let mut sel = select_one_from(CFI, "cq");
    sel.and_where(col("cq", "document_id").eq(doc_col("id"))).and_where(col("cq", "field_id").eq(field_id));
    if let Some(e) = cond {
        sel.and_where(e);
    }
    exists(sel)
}

fn cf_column(c: CfColumn) -> &'static str {
    match c {
        CfColumn::Text => "value_text",
        CfColumn::LongText => "value_long_text",
        CfColumn::Bool => "value_bool",
        CfColumn::Int => "value_int",
        CfColumn::Float => "value_float",
        CfColumn::Date => "value_date",
        CfColumn::Url => "value_url",
        CfColumn::Monetary => "value_monetary",
        CfColumn::MonetaryAmount => "value_monetary_amount",
        CfColumn::DocumentIds => "value_document_ids",
        CfColumn::Select => "value_select",
    }
}

fn cf_value(v: &CfValue) -> SimpleExpr {
    match v {
        CfValue::Bool(b) => Expr::val(*b).into(),
        CfValue::Int(i) => Expr::val(*i).into(),
        CfValue::HugeInt { negative } => {
            let s = if *negative { "-1e400" } else { "1e400" };
            Expr::cust_with_exprs("CAST($1 AS numeric)", [Expr::val(s).into()])
        }
        CfValue::Float(f) => Expr::val(*f).into(),
        CfValue::Text(s) => Expr::val(s.as_str()).into(),
        CfValue::Date(d) => Expr::val(*d).into(),
        CfValue::Numeric(n) => Expr::cust_with_exprs("CAST($1 AS numeric)", [Expr::val(n.as_str()).into()]),
        CfValue::Json(j) => Expr::val(Value::Json(Some(Box::new(j.clone())))).into(),
    }
}

/// Test on `cq.<column>` (like `pl_docfilter::cfquery::value_lookup`).
fn cf_cond(c: &CfCond) -> Cond {
    let column = col("cq", cf_column(c.column));
    match &c.test {
        CfTest::Cmp(op, v) => Cond::Expr(cmp(column, *op, cf_value(v))),
        CfTest::In(vals) => Cond::Expr(column.is_in(vals.iter().map(cf_value))),
        CfTest::IsNull(b) => Cond::Expr(if *b { column.is_null() } else { column.is_not_null() }),
        CfTest::Between(a, b) => Cond::Expr(column.between(cf_value(a), cf_value(b))),
        CfTest::Text(l, s) => Cond::Expr(text_lookup(column, sql_lookup(*l), s)),
        CfTest::YearBounds(op, start, end) => Cond::Expr(match op {
            CmpOp::Eq => column.between(Expr::val(*start), Expr::val(*end)),
            CmpOp::Gt => column.gt(*end),
            CmpOp::Gte => column.gte(*start),
            CmpOp::Lt => column.lt(*start),
            CmpOp::Lte => column.lte(*end),
        }),
        CfTest::Part(part, t) => {
            let e = extract(*part, column);
            match t {
                PartTest::IsNull(b) => Cond::Expr(if *b { e.is_null() } else { e.is_not_null() }),
                PartTest::Cmp(op, v) => Cond::Expr(cmp(e, *op, Expr::val(*v).into())),
                PartTest::In(vals) => {
                    if vals.is_empty() {
                        Cond::False
                    } else {
                        Cond::Expr(e.is_in(vals.iter().copied()))
                    }
                }
                PartTest::Between(a, b) => Cond::Expr(e.between(Expr::val(*a), Expr::val(*b))),
            }
        }
    }
}

// ------------------------------------------------------------------------- Sorting

const NOTES_ALIAS: &str = "onn";

fn join_related(q: &mut SelectStatement, table: &str, fk: &str, alias: &str) {
    q.join_as(JoinType::LeftJoin, Alias::new(table), Alias::new(alias), col(alias, "id").eq(doc_col(fk)));
}

fn join_notes(q: &mut SelectStatement) {
    let counts = Query::select()
        .column((Alias::new("nn"), Alias::new("document_id")))
        .expr_as(Expr::cust("COUNT(*)"), Alias::new("n"))
        .from_as(Alias::new(NOTE), Alias::new("nn"))
        .and_where(col("nn", "deleted_at").is_null())
        .group_by_col((Alias::new("nn"), Alias::new("document_id")))
        .to_owned();
    q.join_subquery(JoinType::LeftJoin, counts, Alias::new(NOTES_ALIAS), col(NOTES_ALIAS, "document_id").eq(doc_col("id")));
}

/// Sort expression per key; adds any needed LEFT JOINs (once per alias).
fn sort_expr(q: &mut SelectStatement, joined: &mut Vec<&'static str>, k: SortKey) -> SimpleExpr {
    let mut join = |q: &mut SelectStatement, alias: &'static str, f: &dyn Fn(&mut SelectStatement)| {
        if !joined.contains(&alias) {
            f(q);
            joined.push(alias);
        }
    };
    match k {
        SortKey::Id => doc_col("id"),
        SortKey::Title => doc_col("title"),
        SortKey::Created => doc_col("created"),
        SortKey::Modified => doc_col("modified"),
        SortKey::Added => doc_col("added"),
        SortKey::Asn => doc_col("archive_serial_number"),
        SortKey::PageCount => doc_col("page_count"),
        SortKey::Owner => doc_col("owner_id"),
        SortKey::CorrespondentName => {
            join(q, "oc", &|q| join_related(q, CORRESPONDENT, "correspondent_id", "oc"));
            col("oc", "name")
        }
        SortKey::DocumentTypeName => {
            join(q, "odt", &|q| join_related(q, DOCTYPE, "document_type_id", "odt"));
            col("odt", "name")
        }
        SortKey::StoragePathName => {
            join(q, "osp", &|q| join_related(q, STORAGEPATH, "storage_path_id", "osp"));
            col("osp", "name")
        }
        SortKey::NumNotes => {
            join(q, NOTES_ALIAS, &join_notes);
            Expr::cust_with_exprs("COALESCE($1, 0)", [col(NOTES_ALIAS, "n")])
        }
    }
}

/// Alias of the LEFT JOIN row for the custom field sorting.
const CF_SORT_ALIAS: &str = "ocf";

/// Custom field sorting as a LEFT JOIN instead of correlated subqueries per document.
///
/// `UNIQUE (document_id, field_id)` (`documents_customfieldinstance_unique_document_field`, also applies
/// to soft-deleted instances) ⇒ at most one instance of the field per document, so the row
/// `ocf` (without trash filter) is exactly this instance or NULL. From that, equivalent to the
/// previous form (contract pl-docfilter "L1", checklist 6/8):
/// * `has_field` = `EXISTS (non-deleted instance)` = `ocf.id IS NOT NULL AND ocf.deleted_at IS NULL`,
/// * value of the first non-deleted instance by `created` = `CASE WHEN ocf.deleted_at IS NULL
///   THEN ocf.<column> END` (NULL without an instance or with a deleted one),
/// * select: `COALESCE(SUM(CASE …), 0)` over **all** instances of the document (also deleted ones; other
///   fields contribute 0) = `CASE` over `ocf` (without an instance: 0).
///
/// For the count (`COUNT(*)`) PostgreSQL removes the LEFT JOIN (unique via the UNIQUE index).
fn join_cf_sort(q: &mut SelectStatement, field_id: i64) {
    q.join_as(
        JoinType::LeftJoin,
        Alias::new(CFI),
        Alias::new(CF_SORT_ALIAS),
        col(CF_SORT_ALIAS, "document_id").eq(doc_col("id")).and(col(CF_SORT_ALIAS, "field_id").eq(field_id)),
    );
}

fn cf_sort_value(field_id: i64, v: &CfSortValue) -> SimpleExpr {
    let _ = field_id;
    match v {
        CfSortValue::Column(c) => {
            Expr::cust_with_exprs("CASE WHEN $1 IS NULL THEN $2 END", [col(CF_SORT_ALIAS, "deleted_at"), col(CF_SORT_ALIAS, cf_column(*c))])
        }
        CfSortValue::SelectOptions(opts) => {
            let field_match = || col(CF_SORT_ALIAS, "id").is_not_null();
            let mut case: Option<sea_query::CaseStatement> = None;
            for (pos, id) in opts.iter().enumerate() {
                let cond = match id {
                    None => field_match().and(col(CF_SORT_ALIAS, "value_select").is_null()),
                    Some(s) => field_match().and(col(CF_SORT_ALIAS, "value_select").eq(s.clone())),
                };
                case = Some(match case {
                    None => Expr::case(cond, Expr::val(pos as i64)),
                    Some(c) => c.case(cond, Expr::val(pos as i64)),
                });
            }
            let null_cond = field_match().and(col(CF_SORT_ALIAS, "value_select").is_null());
            let n = opts.len() as i64;
            let case = match case {
                None => Expr::case(null_cond, Expr::val(n)),
                Some(c) => c.case(null_cond, Expr::val(n)),
            }
            .finally(Expr::val(0i64));
            case.into()
        }
    }
}

/// Previous form (correlated subqueries) – now only for the comparison in the differential test.
#[doc(hidden)]
pub fn cf_sort_value_subquery(field_id: i64, v: &CfSortValue) -> SimpleExpr {
    match v {
        CfSortValue::Column(c) => {
            let sub = Query::select()
                .column((Alias::new("cfo"), Alias::new(cf_column(*c))))
                .from_as(Alias::new(CFI), Alias::new("cfo"))
                .and_where(col("cfo", "document_id").eq(doc_col("id")))
                .and_where(col("cfo", "field_id").eq(field_id))
                .and_where(col("cfo", "deleted_at").is_null())
                .order_by((Alias::new("cfo"), Alias::new("created")), Order::Asc)
                .limit(1)
                .to_owned();
            scalar(sub)
        }
        CfSortValue::SelectOptions(opts) => {
            let field_match = || col("cfs", "field_id").eq(field_id);
            let mut case: Option<sea_query::CaseStatement> = None;
            for (pos, id) in opts.iter().enumerate() {
                let cond = match id {
                    None => field_match().and(col("cfs", "value_select").is_null()),
                    Some(s) => field_match().and(col("cfs", "value_select").eq(s.clone())),
                };
                case = Some(match case {
                    None => Expr::case(cond, Expr::val(pos as i64)),
                    Some(c) => c.case(cond, Expr::val(pos as i64)),
                });
            }
            let null_cond = field_match().and(col("cfs", "value_select").is_null());
            let n = opts.len() as i64;
            let case = match case {
                None => Expr::case(null_cond, Expr::val(n)),
                Some(c) => c.case(null_cond, Expr::val(n)),
            }
            .finally(Expr::val(0i64));
            let sum = Query::select()
                .expr(Expr::cust_with_exprs("SUM($1)", [case.into()]))
                .from_as(Alias::new(CFI), Alias::new("cfs"))
                .and_where(col("cfs", "document_id").eq(doc_col("id")))
                .to_owned();
            Expr::cust_with_exprs("COALESCE($1, 0)", [scalar(sum)])
        }
    }
}

/// ORDER BY like `DocumentsOrderingFilter` (sets the complete sorting).
pub fn apply_sort(q: &mut SelectStatement, sort: &Sort) {
    match sort {
        Sort::None => {}
        Sort::Default => {
            q.order_by_expr(doc_col("created"), Order::Desc).order_by_expr(doc_col("id"), Order::Desc);
        }
        Sort::Terms(terms) => {
            let mut joined = Vec::new();
            for (k, desc) in terms {
                let e = sort_expr(q, &mut joined, *k);
                q.order_by_expr(e, if *desc { Order::Desc } else { Order::Asc });
            }
        }
        Sort::CustomField { field_id, value, desc } => {
            join_cf_sort(q, *field_id);
            let has =
                Expr::cust_with_exprs("($1 IS NOT NULL AND $2 IS NULL)", [col(CF_SORT_ALIAS, "id"), col(CF_SORT_ALIAS, "deleted_at")]);
            q.order_by_expr(has, Order::Desc);
            q.order_by_expr(cf_sort_value(*field_id, value), if *desc { Order::Desc } else { Order::Asc });
        }
    }
}

/// Previous custom field sorting (EXISTS + value subquery) – for the differential test.
#[doc(hidden)]
pub fn apply_sort_cf_subquery(q: &mut SelectStatement, field_id: i64, value: &CfSortValue, desc: bool) {
    let mut has = select_one_from(CFI, "cfh");
    has.and_where(col("cfh", "document_id").eq(doc_col("id")))
        .and_where(col("cfh", "field_id").eq(field_id))
        .and_where(col("cfh", "deleted_at").is_null());
    q.order_by_expr(exists(has), Order::Desc);
    q.order_by_expr(cf_sort_value_subquery(field_id, value), if desc { Order::Desc } else { Order::Asc });
}

// ------------------------------------------------------------------------- v1 entry points

/// Filtered, sorted document list for v1 (`filter_queryset(get_queryset())` incl.
/// `PermittedObjectsFilter`): parameters → AST (`pl_docfilter::ast::parse`) → one query.
/// Second value: contains a substring search over the content (→ page + count in one scan).
pub async fn v1_list(
    db: &sqlx::PgPool,
    ctx: &pl_docfilter::FilterContext,
    user: &User,
    params: &pl_docfilter::QueryParams,
) -> Result<(SelectStatement, bool), pl_docfilter::FilterError> {
    let f = pl_docfilter::ast::parse(params, ctx, db).await?;
    let fold = fold_for(db, &f.pred).await;
    let trgm = trigram_for(db, &f.pred).await;
    let hits = if trgm { Vec::new() } else { text_hits_for(db, fold.as_deref(), &f.pred).await };
    // With a hit set the count no longer reads any text: page via the sort index (early
    // abort) + separate count instead of one pass with a window count.
    // Own data model: the join of the changed text rows reads the row versions of the
    // text table once per subquery – page and count are therefore done in one pass.
    let scan_text = f.pred.has_content_text() && (hits.is_empty() || hits.iter().any(|h| h.set.native()));
    let vis = Visibility::for_user(user);
    let q = Builder::new(&vis, fold.as_deref()).with_trigram(trgm).with_hits(hits).select(&f);
    Ok((q, scan_text))
}

/// Selection `all=true` (`DocumentSelectionMixin._resolve_document_ids`): lenient filters,
/// visible root documents, without sorting.
pub async fn v1_selection(
    db: &sqlx::PgPool,
    ctx: &pl_docfilter::FilterContext,
    user: &User,
    params: &pl_docfilter::QueryParams,
) -> Result<SelectStatement, pl_docfilter::FilterError> {
    let p = pl_docfilter::ast::parse_filters(params, ctx, db, true).await?;
    let fold = fold_for(db, &p).await;
    let trgm = trigram_for(db, &p).await;
    let hits = if trgm { Vec::new() } else { text_hits_for(db, fold.as_deref(), &p).await };
    let vis = Visibility::for_user(user);
    Ok(Builder::new(&vis, fold.as_deref()).with_trigram(trgm).with_hits(hits).select_unordered(&p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_query::PostgresQueryBuilder;

    #[test]
    fn trigram_leaves_only_eligible_title_content() {
        let t = |col, v: &str| Pred::Text { col, lookup: TextLookup::IContains, value: v.into() };
        let p = Pred::And(vec![
            Pred::Or(vec![t(TextCol::Title, "rechnung"), t(TextCol::Content, "rechnung")]),
            Pred::Not(Box::new(t(TextCol::Content, "ab"))),
            t(TextCol::OriginalFilename, "scan"),
        ]);
        let mut v = Vec::new();
        trigram_leaves(&p, &mut v);
        assert_eq!(v.len(), 2);
        assert!(v.iter().all(|(_, _, s)| *s == "rechnung"));
    }

    fn user(id: i64, su: bool, active: bool) -> User {
        User {
            id,
            username: format!("user{id}"),
            password: "!".into(),
            first_name: String::new(),
            last_name: String::new(),
            email: String::new(),
            is_active: active,
            is_staff: false,
            is_superuser: su,
            last_login: None,
            date_joined: chrono::Utc::now(),
            groups: Vec::new(),
            permissions: Default::default(),
        }
    }

    #[test]
    fn visibility_on_outer_row() {
        let v = Visibility::for_user(&user(7, false, true));
        let s = Builder::new(&v, None).select(&DocFilter::default()).to_string(PostgresQueryBuilder);
        assert!(s.contains(r#""documents_document"."owner_id" = 7"#), "{s}");
        assert!(s.contains(r#"CAST("documents_document"."id" AS varchar) IN"#), "{s}");
        assert!(!s.contains("pl_authz_parent"), "{s}");
        // L5: the same place via the grant index (the only test here that sets the switch)
        pl_authz::grants::set_override(Some(pl_authz::grants::Engine::Grants));
        let g = Builder::new(&v, None).select(&DocFilter::default()).to_string(PostgresQueryBuilder);
        pl_authz::grants::set_override(None);
        assert!(g.contains(r#""documents_document"."id" IN (SELECT "pg_g"."object_id" FROM "paperrust_grant""#), "{g}");
        assert!(!g.contains("guardian"), "{g}");
        assert!(s.ends_with(r#"ORDER BY "documents_document"."created" DESC, "documents_document"."id" DESC"#), "{s}");
        let all = Visibility::for_user(&user(1, true, true));
        let s = Builder::new(&all, None).select_unordered(&Pred::True).to_string(PostgresQueryBuilder);
        assert_eq!(
            s,
            r#"SELECT "documents_document"."id" FROM "documents_document" WHERE "documents_document"."root_document_id" IS NULL AND "documents_document"."deleted_at" IS NULL"#
        );
        let none = Visibility::for_user(&user(1, true, false));
        assert!(none.visible_ids_sql().ends_with("AND (FALSE)"), "{}", none.visible_ids_sql());
    }

    #[test]
    fn text_only_decides_hit_cache() {
        let t = |col, v: &str| Pred::Text { col, lookup: TextLookup::IContains, value: v.into() };
        let tc = Pred::Or(vec![t(TextCol::Title, "rechnung"), t(TextCol::Content, "rechnung")]);
        assert!(text_only(&tc));
        assert!(text_only(&Pred::Not(Box::new(t(TextCol::Content, "x")))));
        assert!(!text_only(&Pred::And(vec![tc.clone(), Pred::Num { col: NumCol::Asn, op: CmpOp::Gt, value: 1 }])));
        assert!(!text_only(&t(TextCol::OriginalFilename, "scan")));
        assert!(!text_only(&Pred::True));
        assert!(!text_only(&Pred::Or(vec![])));
        let mut v = Vec::new();
        let p = Pred::And(vec![tc.clone(), Pred::Not(Box::new(t(TextCol::Content, "ab")))]);
        text_leaves(&p, &mut v);
        assert_eq!(v.len(), 3);
    }

    #[test]
    fn hit_cache_sql_on_outer_row() {
        let t = |col, v: &str| Pred::Text { col, lookup: TextLookup::IContains, value: v.into() };
        let set = Arc::new(crate::textcache::HitSet::for_tests("5:9:", &[1, 3]).unwrap());
        let hit = |col| TextHit { col, lookup: TextLookup::IContains, value: "rechnung".into(), set: set.clone() };
        let v = Visibility::All;
        let p = Pred::Or(vec![t(TextCol::Title, "rechnung"), t(TextCol::Content, "rechnung")]);
        let s = Builder::new(&v, None)
            .with_hits(vec![hit(TextCol::Title), hit(TextCol::Content)])
            .select_unordered(&p)
            .to_string(PostgresQueryBuilder);
        assert_eq!(s.matches("get_bit(").count(), 2, "{s}");
        assert!(s.contains(r#"age("documents_document"."xmin") > age(('9')::text::xid)"#), "{s}");
        // re-check on the text for changed rows: title, own content, effective content (versions)
        assert!(s.contains(r#"UPPER(("documents_document"."title")::text)"#), "{s}");
        assert!(s.contains(r#"UPPER(("documents_document"."content")::text)"#), "{s}");
        assert!(s.contains(r#"SELECT "hit_v"."root_document_id" FROM "documents_document" AS "hit_v""#), "{s}");
        assert!(s.contains("COALESCE(("), "{s}");
        // different search text: no hit set
        let s2 = Builder::new(&v, None)
            .with_hits(vec![hit(TextCol::Title)])
            .select_unordered(&t(TextCol::Title, "beitrag"))
            .to_string(PostgresQueryBuilder);
        assert!(!s2.contains("get_bit("), "{s2}");
        assert!(!s.contains("hit_ch"), "{s}");
    }

    #[test]
    fn hit_cache_sql_native_layout_joins_content_table() {
        let t = |col, v: &str| Pred::Text { col, lookup: TextLookup::IContains, value: v.into() };
        let set = Arc::new(crate::textcache::HitSet::for_tests_native("5:9:", &[1, 3]).unwrap());
        let hit = TextHit { col: TextCol::Content, lookup: TextLookup::IContains, value: "rechnung".into(), set };
        let v = Visibility::All;
        let p = Pred::Or(vec![t(TextCol::Title, "rechnung"), t(TextCol::Content, "rechnung")]);
        let s = Builder::new(&v, None).with_hits(vec![hit]).select_unordered(&p).to_string(PostgresQueryBuilder);
        // changed text rows as their own join, row version from the text table
        assert!(s.contains(r#"LEFT JOIN (SELECT unnest(ARRAY (SELECT "hit_chg"."document_id" FROM "paperrust_document_content" AS "hit_chg" WHERE NOT "#), "{s}");
        assert!(s.contains(&format!(r#"AS "hit_ch0" ON "hit_ch0"."document_id" = "{DOC}"."id""#)), "{s}");
        assert!(s.contains(r#"age("hit_chg"."xmin") > age(('9')::text::xid)"#), "{s}");
        assert!(s.contains(r#"CASE WHEN "hit_ch0"."document_id" IS NOT NULL THEN"#), "{s}");
        assert_eq!(s.matches("get_bit(").count(), 1, "{s}");
        // own content read individually from the text table, title directly (no hit set for the title);
        // the view's text column does not appear in the outer row
        let own =
            format!(r#"SELECT "hit_t"."content" FROM "paperrust_document_content" AS "hit_t" WHERE "hit_t"."document_id" = "{DOC}"."id""#);
        assert!(s.contains(&own), "{s}");
        assert!(s.contains(&format!(r#"UPPER(("{DOC}"."title")::text)"#)), "{s}");
        for gone in ["content", "xmin"] {
            assert!(!s.contains(&format!(r#""{DOC}"."{gone}""#)), "{gone}: {s}");
        }
    }

    #[test]
    fn complement_and_constants() {
        let v = Visibility::All;
        let b = Builder::new(&v, None);
        let p = Pred::Num { col: NumCol::Asn, op: CmpOp::Gt, value: 5 }.complement();
        let s = b.select_unordered(&p).to_string(PostgresQueryBuilder);
        assert!(s.contains(r#"("documents_document"."archive_serial_number" > 5) IS NOT TRUE"#), "{s}");
        let s = b.select_unordered(&Pred::False).to_string(PostgresQueryBuilder);
        assert!(s.ends_with("AND (FALSE)"), "{s}");
    }
}
