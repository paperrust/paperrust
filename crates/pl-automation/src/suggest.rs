//! Suggestions from the archive (without AI): patterns in existing assignments ("correspondent X →
//! almost always tag Y", "41 of 43") over the documents the user sees; result cached per user and
//! parameter for 10 minutes. From it a workflow draft in ngx format.
//!
//! Patterns: `correspondent_tag`, `correspondent_document_type`, `correspondent_storage_path`,
//! `document_type_tag`, `filename_document_type` (file name starts with a word). A pattern
//! "mail rule → type" does not exist: ngx does not store which mail rule a document came through
//! (Known difference).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use pl_authz::User;
use pl_core::AppState;
use sea_query::PostgresQueryBuilder;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};

use crate::common::{self, ADD_WORKFLOW, NameKind, Names, VIEW_DOCUMENT, VIEW_WORKFLOW, Who};
use crate::error::{AErr, AResult, json as json_resp};

const TTL: Duration = Duration::from_secs(600);
const MAX_CACHE: usize = 64;
/// Generic file name beginnings without meaning.
const STOP_PREFIXES: &[&str] =
    &["scan", "img", "image", "doc", "document", "dokument", "file", "datei", "untitled", "pdf", "page", "seite", "export"];

type CacheKey = (i64, i64, u32);
static CACHE: Mutex<Option<HashMap<CacheKey, (Instant, Value)>>> = Mutex::new(None);

#[derive(serde::Deserialize, Default)]
pub struct SuggestQuery {
    pub min_documents: Option<String>,
    pub min_share: Option<String>,
    pub limit: Option<String>,
    pub refresh: Option<String>,
}

/// A found pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub kind: &'static str,
    /// Source object (id) or file name beginning.
    pub from_id: Option<i64>,
    pub from_prefix: Option<String>,
    pub to_id: i64,
    pub matches: i64,
    pub total: i64,
}

impl Pattern {
    pub fn key(&self) -> String {
        match &self.from_prefix {
            Some(p) => format!("{}:{}:{}", self.kind, p, self.to_id),
            None => format!("{}:{}:{}", self.kind, self.from_id.unwrap_or(0), self.to_id),
        }
    }
    pub fn share(&self) -> f64 {
        if self.total == 0 { 0.0 } else { self.matches as f64 / self.total as f64 }
    }
}

/// Split the key of a pattern (for the draft). File name beginning letters only.
pub fn parse_key(key: &str) -> Option<Pattern> {
    let mut it = key.splitn(3, ':');
    let kind = match it.next()? {
        "correspondent_tag" => "correspondent_tag",
        "correspondent_document_type" => "correspondent_document_type",
        "correspondent_storage_path" => "correspondent_storage_path",
        "document_type_tag" => "document_type_tag",
        "filename_document_type" => "filename_document_type",
        _ => return None,
    };
    let from = it.next()?;
    let to_id = it.next()?.parse::<i64>().ok()?;
    if kind == "filename_document_type" {
        let p = from.to_lowercase();
        if p.len() < 3 || p.len() > 64 || !p.chars().all(char::is_alphabetic) {
            return None;
        }
        Some(Pattern { kind, from_id: None, from_prefix: Some(p), to_id, matches: 0, total: 0 })
    } else {
        Some(Pattern { kind, from_id: Some(from.parse().ok()?), from_prefix: None, to_id, matches: 0, total: 0 })
    }
}

fn parse_param<T: std::str::FromStr>(v: Option<&str>, name: &str, default: T, ok: impl Fn(&T) -> bool) -> AResult<T> {
    match v {
        None | Some("") => Ok(default),
        Some(s) => s.parse::<T>().ok().filter(|x| ok(x)).ok_or_else(|| AErr::field(name, "Invalid value.")),
    }
}

/// GET /suggestions
pub async fn list(State(st): State<AppState>, who: Who, Query(q): Query<SuggestQuery>) -> Result<Response, AErr> {
    let user = who.require(&[VIEW_WORKFLOW, VIEW_DOCUMENT])?;
    let min_docs: i64 = parse_param(q.min_documents.as_deref(), "min_documents", 5, |n| (2..=100_000).contains(n))?;
    let min_share: f64 = parse_param(q.min_share.as_deref(), "min_share", 0.9, |s: &f64| (0.5..=1.0).contains(s))?;
    let limit: usize = parse_param(q.limit.as_deref(), "limit", 30, |n| (1..=100).contains(n))?;
    let refresh = matches!(q.refresh.as_deref(), Some("true" | "1" | "yes"));
    let key: CacheKey = (user.id, min_docs, (min_share * 1000.0).round() as u32);
    if !refresh && let Some(v) = cache_get(&key) {
        return Ok(json_resp(StatusCode::OK, &with_limit(v, limit, true)));
    }
    let mut conn = st.db().acquire().await?;
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    sqlx::query("SET LOCAL statement_timeout = '20s'").execute(&mut *tx).await?;
    let patterns = find_patterns(&mut tx, user, min_docs, min_share).await?;
    tx.commit().await?;
    let covered = coverage(&mut conn).await?;
    let mut names = Names::default();
    for p in &patterns {
        let (fk, tk) = kinds(p.kind);
        if let (Some(id), Some(k)) = (p.from_id, fk) {
            names.want(k, id);
        }
        names.want(tk, p.to_id);
    }
    names.resolve(&mut conn, user).await?;
    let items: Vec<Value> = patterns
        .iter()
        .map(|p| {
            let (fk, tk) = kinds(p.kind);
            let from = match (&p.from_prefix, p.from_id, fk) {
                (Some(pre), _, _) => json!({"field": "filename_prefix", "value": pre}),
                (None, Some(id), Some(k)) => json!({"field": field_name(k), "id": id, "name": names.get(k, id)}),
                _ => Value::Null,
            };
            json!({
                "key": p.key(),
                "kind": p.kind,
                "from": from,
                "to": {"field": field_name(tk), "id": p.to_id, "name": names.get(tk, p.to_id)},
                "matches": p.matches,
                "total": p.total,
                "share": (p.share() * 1000.0).round() / 1000.0,
                "covered_by_workflow": is_covered(&covered, p),
            })
        })
        .collect();
    let body = json!({
        "items": items,
        "generated_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "min_documents": min_docs,
        "min_share": min_share,
    });
    cache_put(key, body.clone());
    Ok(json_resp(StatusCode::OK, &with_limit(body, limit, false)))
}

fn with_limit(mut v: Value, limit: usize, cached: bool) -> Value {
    if let Some(items) = v.get_mut("items").and_then(Value::as_array_mut) {
        items.truncate(limit);
    }
    v["cached"] = json!(cached);
    v
}

fn cache_get(k: &CacheKey) -> Option<Value> {
    let g = CACHE.lock().ok()?;
    let (at, v) = g.as_ref()?.get(k)?;
    (at.elapsed() < TTL).then(|| v.clone())
}

fn cache_put(k: CacheKey, v: Value) {
    if let Ok(mut g) = CACHE.lock() {
        let m = g.get_or_insert_with(HashMap::new);
        m.retain(|_, (at, _)| at.elapsed() < TTL);
        if m.len() >= MAX_CACHE {
            m.clear();
        }
        m.insert(k, (Instant::now(), v));
    }
}

fn kinds(kind: &str) -> (Option<NameKind>, NameKind) {
    match kind {
        "correspondent_tag" => (Some(NameKind::Correspondent), NameKind::Tag),
        "correspondent_document_type" => (Some(NameKind::Correspondent), NameKind::DocumentType),
        "correspondent_storage_path" => (Some(NameKind::Correspondent), NameKind::StoragePath),
        "document_type_tag" => (Some(NameKind::DocumentType), NameKind::Tag),
        _ => (None, NameKind::DocumentType),
    }
}

fn field_name(k: NameKind) -> &'static str {
    match k {
        NameKind::Tag => "tag",
        NameKind::Correspondent => "correspondent",
        NameKind::DocumentType => "document_type",
        NameKind::StoragePath => "storage_path",
        _ => "other",
    }
}

/// Patterns over the visible root documents (one query per pattern kind; grouping in the
/// database via the FK indexes).
pub async fn find_patterns(conn: &mut PgConnection, user: &User, min_docs: i64, min_share: f64) -> AResult<Vec<Pattern>> {
    let visible = pl_authz::objects::permitted_document_ids(Some(user), "view_document", false).to_string(PostgresQueryBuilder);
    let base = format!(
        "d AS MATERIALIZED (SELECT id, correspondent_id, document_type_id, storage_path_id, original_filename FROM documents_document \
         WHERE root_document_id IS NULL AND deleted_at IS NULL AND id IN ({visible}))"
    );
    let inbox_free = "NOT EXISTS (SELECT 1 FROM documents_tag it WHERE it.id = x.to_id AND it.is_inbox_tag)";
    let pair = |from: &str, to_sql: &str, extra: &str| {
        format!(
            "WITH {base}, tot AS (SELECT {from} AS from_id, count(*) AS n FROM d WHERE {from} IS NOT NULL GROUP BY 1 HAVING count(*) >= $1), \
             x AS (SELECT d.{from} AS from_id, {to_sql} AS to_id, count(*) AS k FROM d {extra} WHERE d.{from} IS NOT NULL GROUP BY 1, 2) \
             SELECT x.from_id, x.to_id, x.k, tot.n FROM x JOIN tot USING (from_id) WHERE x.to_id IS NOT NULL AND x.k::float8 / tot.n >= $2"
        )
    };
    let mut out = Vec::new();
    let specs: [(&'static str, String, bool); 4] = [
        ("correspondent_tag", pair("correspondent_id", "t.tag_id", "JOIN documents_document_tags t ON t.document_id = d.id"), true),
        ("correspondent_document_type", pair("correspondent_id", "d.document_type_id", ""), false),
        ("correspondent_storage_path", pair("correspondent_id", "d.storage_path_id", ""), false),
        ("document_type_tag", pair("document_type_id", "t.tag_id", "JOIN documents_document_tags t ON t.document_id = d.id"), true),
    ];
    for (kind, sql, tags) in specs {
        let sql = if tags { format!("{sql} AND {inbox_free}") } else { sql };
        for r in sqlx::query(&sql).bind(min_docs).bind(min_share).fetch_all(&mut *conn).await? {
            out.push(Pattern {
                kind,
                from_id: Some(i64::from(r.try_get::<i32, _>(0)?)),
                from_prefix: None,
                to_id: i64::from(r.try_get::<i32, _>(1)?),
                matches: r.try_get(2)?,
                total: r.try_get(3)?,
            });
        }
    }
    let fsql = format!(
        "WITH {base}, f AS (SELECT lower(substring(original_filename from '^([[:alpha:]]{{3,}})')) AS p, document_type_id FROM d \
         WHERE original_filename IS NOT NULL), tot AS (SELECT p, count(*) AS n FROM f WHERE p IS NOT NULL GROUP BY p HAVING count(*) >= $1), \
         x AS (SELECT p, document_type_id AS to_id, count(*) AS k FROM f WHERE p IS NOT NULL AND document_type_id IS NOT NULL GROUP BY 1, 2) \
         SELECT x.p, x.to_id, x.k, tot.n FROM x JOIN tot USING (p) WHERE x.k::float8 / tot.n >= $2 AND NOT (x.p = ANY($3))"
    );
    let stop: Vec<String> = STOP_PREFIXES.iter().map(|s| s.to_string()).collect();
    for r in sqlx::query(&fsql).bind(min_docs).bind(min_share).bind(&stop).fetch_all(&mut *conn).await? {
        let p: String = r.try_get(0)?;
        if p.chars().count() > 64 {
            continue;
        }
        out.push(Pattern {
            kind: "filename_document_type",
            from_id: None,
            from_prefix: Some(p),
            to_id: i64::from(r.try_get::<i32, _>(1)?),
            matches: r.try_get(2)?,
            total: r.try_get(3)?,
        });
    }
    sort_patterns(&mut out);
    Ok(out)
}

/// Most frequent first, then higher share, then key (stable).
pub fn sort_patterns(v: &mut [Pattern]) {
    v.sort_by(|a, b| {
        b.matches
            .cmp(&a.matches)
            .then(b.share().partial_cmp(&a.share()).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.key().cmp(&b.key()))
    });
}

/// What existing active workflows already assign: (kind, from, to).
pub async fn coverage(conn: &mut PgConnection) -> AResult<HashSet<(String, String, i64)>> {
    let rows = sqlx::query(
        "SELECT w.id, t.filter_has_correspondent_id, t.filter_has_document_type_id, t.filter_filename, \
           a.assign_document_type_id, a.assign_storage_path_id, \
           COALESCE((SELECT array_agg(x.tag_id) FROM documents_workflowaction_assign_tags x WHERE x.workflowaction_id = a.id), '{}'), \
           COALESCE((SELECT array_agg(c.correspondent_id) FROM documents_workflowtrigger_filter_has_any_correspondents c WHERE c.workflowtrigger_id = t.id), '{}') \
         FROM documents_workflow w \
         JOIN documents_workflow_triggers wt ON wt.workflow_id = w.id JOIN documents_workflowtrigger t ON t.id = wt.workflowtrigger_id \
         JOIN documents_workflow_actions wa ON wa.workflow_id = w.id JOIN documents_workflowaction a ON a.id = wa.workflowaction_id \
         WHERE w.enabled AND a.type = 1",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = HashSet::new();
    for r in rows {
        let corr: Option<i32> = r.try_get(1)?;
        let dtype: Option<i32> = r.try_get(2)?;
        let fname: Option<String> = r.try_get(3)?;
        let a_dt: Option<i32> = r.try_get(4)?;
        let a_sp: Option<i32> = r.try_get(5)?;
        let a_tags: Vec<i32> = r.try_get(6)?;
        let any_corr: Vec<i32> = r.try_get(7)?;
        let corrs: Vec<i32> = corr.into_iter().chain(any_corr).collect();
        for c in &corrs {
            for t in &a_tags {
                out.insert(("correspondent_tag".into(), c.to_string(), i64::from(*t)));
            }
            if let Some(d) = a_dt {
                out.insert(("correspondent_document_type".into(), c.to_string(), i64::from(d)));
            }
            if let Some(s) = a_sp {
                out.insert(("correspondent_storage_path".into(), c.to_string(), i64::from(s)));
            }
        }
        if let Some(d) = dtype {
            for t in &a_tags {
                out.insert(("document_type_tag".into(), d.to_string(), i64::from(*t)));
            }
        }
        if let (Some(f), Some(d)) = (fname, a_dt) {
            let p: String = f.trim_end_matches('*').to_lowercase();
            if !p.is_empty() && p.chars().all(char::is_alphabetic) {
                out.insert(("filename_document_type".into(), p, i64::from(d)));
            }
        }
    }
    Ok(out)
}

pub fn is_covered(c: &HashSet<(String, String, i64)>, p: &Pattern) -> bool {
    let from = p.from_prefix.clone().unwrap_or_else(|| p.from_id.unwrap_or(0).to_string());
    c.contains(&(p.kind.to_string(), from, p.to_id))
}

/// Workflow draft (ngx format, `enabled: false`, nothing saved) for a pattern.
pub fn draft_for(p: &Pattern, from_name: Option<&str>, to_name: Option<&str>) -> Value {
    let from_label =
        from_name.map(str::to_string).or_else(|| p.from_prefix.as_ref().map(|x| format!("{x}*"))).unwrap_or_else(|| "?".into());
    let to_label = to_name.unwrap_or("?");
    let name = format!("{from_label} \u{2192} {to_label}");
    let action = match p.kind {
        "correspondent_tag" | "document_type_tag" => json!({"type": 1, "assign_tags": [p.to_id]}),
        "correspondent_storage_path" => json!({"type": 1, "assign_storage_path": p.to_id}),
        _ => json!({"type": 1, "assign_document_type": p.to_id}),
    };
    let triggers = match p.kind {
        "filename_document_type" => vec![json!({
            "type": 1,
            "sources": [1, 2, 3, 4],
            "filter_filename": format!("{}*", p.from_prefix.clone().unwrap_or_default()),
            "matching_algorithm": 0,
            "match": "",
            "is_insensitive": true,
        })],
        kind => {
            let filter = if kind == "document_type_tag" { "filter_has_document_type" } else { "filter_has_correspondent" };
            // added and updated: also if the correspondent/type is only set later
            [2, 3]
                .iter()
                .map(|ty| {
                    let mut t = serde_json::Map::new();
                    t.insert("type".into(), json!(ty));
                    t.insert("matching_algorithm".into(), json!(0));
                    t.insert("match".into(), json!(""));
                    t.insert("is_insensitive".into(), json!(true));
                    t.insert(filter.into(), json!(p.from_id));
                    Value::Object(t)
                })
                .collect()
        }
    };
    json!({
        "name": name.chars().take(256).collect::<String>(),
        "order": 0,
        "enabled": false,
        "triggers": triggers,
        "actions": [action],
    })
}

/// POST /suggestions/draft `{"key"}`
pub async fn draft(State(st): State<AppState>, who: Who, headers: HeaderMap, body: Bytes) -> Result<Response, AErr> {
    let user = who.require(&[ADD_WORKFLOW])?;
    let b = common::body_object(&headers, &body)?;
    let key = b.get("key").and_then(Value::as_str).ok_or_else(|| AErr::field("key", "This field is required."))?;
    let p = parse_key(key).ok_or_else(|| AErr::field("key", "Unknown suggestion."))?;
    let mut conn = st.db().acquire().await?;
    let (fk, tk) = kinds(p.kind);
    let mut names = Names::default();
    if let (Some(id), Some(k)) = (p.from_id, fk) {
        names.want(k, id);
    }
    names.want(tk, p.to_id);
    names.resolve(&mut conn, user).await?;
    // Objects the user does not see (or that do not exist) belong in no draft
    let from_name = match (p.from_id, fk) {
        (Some(id), Some(k)) => Some(names.get(k, id).ok_or_else(|| AErr::field("key", "Unknown suggestion."))?),
        _ => None,
    };
    let to_name = names.get(tk, p.to_id).ok_or_else(|| AErr::field("key", "Unknown suggestion."))?;
    Ok(json_resp(StatusCode::OK, &json!({"workflow": draft_for(&p, from_name, Some(to_name))})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_roundtrip_and_validation() {
        let p = Pattern { kind: "correspondent_tag", from_id: Some(4), from_prefix: None, to_id: 9, matches: 41, total: 43 };
        assert_eq!(p.key(), "correspondent_tag:4:9");
        let q = parse_key(&p.key()).unwrap();
        assert_eq!((q.kind, q.from_id, q.to_id), ("correspondent_tag", Some(4), 9));
        assert!(parse_key("filename_document_type:rechnung:3").is_some());
        assert!(parse_key("filename_document_type:re'; drop:3").is_none());
        assert!(parse_key("filename_document_type:ab:3").is_none());
        assert!(parse_key("nope:1:2").is_none());
        assert!(parse_key("correspondent_tag:x:2").is_none());
        assert!((p.share() - 41.0 / 43.0).abs() < 1e-9);
    }

    #[test]
    fn drafts_are_ngx_workflows() {
        let p = Pattern { kind: "correspondent_tag", from_id: Some(4), from_prefix: None, to_id: 9, matches: 1, total: 1 };
        let d = draft_for(&p, Some("ACME"), Some("Invoices"));
        assert_eq!(d["name"], "ACME \u{2192} Invoices");
        assert_eq!(d["enabled"], false);
        assert_eq!(d["triggers"][0]["filter_has_correspondent"], 4);
        assert_eq!(d["triggers"][1]["type"], 3);
        assert_eq!(d["actions"][0]["assign_tags"][0], 9);
        let f =
            Pattern { kind: "filename_document_type", from_id: None, from_prefix: Some("rechnung".into()), to_id: 2, matches: 1, total: 1 };
        let d = draft_for(&f, None, Some("Invoice"));
        assert_eq!(d["triggers"][0]["filter_filename"], "rechnung*");
        assert_eq!(d["actions"][0]["assign_document_type"], 2);
    }

    #[test]
    fn coverage_and_sorting() {
        let mut c = HashSet::new();
        c.insert(("correspondent_tag".to_string(), "4".to_string(), 9));
        let p = Pattern { kind: "correspondent_tag", from_id: Some(4), from_prefix: None, to_id: 9, matches: 1, total: 1 };
        assert!(is_covered(&c, &p));
        let mut v = vec![
            Pattern { kind: "correspondent_tag", from_id: Some(1), from_prefix: None, to_id: 1, matches: 5, total: 5 },
            Pattern { kind: "correspondent_tag", from_id: Some(2), from_prefix: None, to_id: 1, matches: 9, total: 10 },
            Pattern { kind: "correspondent_tag", from_id: Some(3), from_prefix: None, to_id: 1, matches: 9, total: 9 },
        ];
        sort_patterns(&mut v);
        assert_eq!(v.iter().map(|p| p.from_id.unwrap()).collect::<Vec<_>>(), vec![3, 2, 1]);
    }
}
