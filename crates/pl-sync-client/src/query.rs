//! Offline queries on the replica (docs/SYNC.md §4.6): document list with the v2 query AST
//! (`pl_domain::query::DocQuery`, same form as `POST /rs/api/v2/documents/query`), full text
//! via FTS5 and lists of the metadata kinds – responses in v2 form (`{"items","next_cursor"}`).
//!
//! Deviations from the server (deliberate, labeled in the UI as "Suche im lokalen Bestand"):
//! * `text` searches via FTS5 (words ANDed, every word as a prefix, `"…"` = phrase) instead of
//!   substring; `title_content` locally also covers notes and names (correspondent, type, tags);
//! * `custom_field_query`, `shared_by_me`, `any`/`not` → 501 `not_implemented`;
//! * date bounds of `added`/`modified` are compared on the UTC date.

use pl_domain::DomainError;
use pl_domain::cursor::KeysetCursor;
use pl_domain::query::{Cond, DateRange, DocQuery, RefSet, TextField, parse_sort};
use serde_json::{Value, json};
use sqlx::Row;

use crate::error::{Error, Result};
use crate::store::Replica;

/// Default/maximum size of a page (like the server).
pub const DEFAULT_LIMIT: i64 = 100;
pub const MAX_LIMIT: i64 = 1000;

/// Search text of the search bar → FTS5 expression. Words ANDed, `word*` = prefix, `"…"` =
/// phrase; with `implicit_prefix` every word is searched as a prefix (type-ahead search). All terms
/// are quoted – FTS5 operators from the input have no effect (no syntax errors).
pub fn fts_expr(input: &str, implicit_prefix: bool) -> Option<String> {
    let mut terms = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '"' {
            chars.next();
            let mut phrase = String::new();
            for ch in chars.by_ref() {
                if ch == '"' {
                    break;
                }
                phrase.push(ch);
            }
            let p = phrase.trim();
            if !p.is_empty() {
                terms.push(format!("\"{}\"", p.replace('"', "\"\"")));
            }
            continue;
        }
        let mut word = String::new();
        while let Some(&ch) = chars.peek() {
            if ch.is_whitespace() || ch == '"' {
                break;
            }
            word.push(ch);
            chars.next();
        }
        let prefix = word.ends_with('*');
        let w: String = word.trim_end_matches('*').to_string();
        // only punctuation → search nothing (FTS5 would reject the empty term)
        if !w.chars().any(char::is_alphanumeric) {
            continue;
        }
        let quoted = format!("\"{}\"", w.replace('"', "\"\""));
        terms.push(if prefix || implicit_prefix { format!("{quoted}*") } else { quoted });
    }
    if terms.is_empty() { None } else { Some(terms.join(" AND ")) }
}

#[derive(Debug, Clone)]
pub(crate) enum Arg {
    S(String),
    I(i64),
}

/// Conditions + parameters.
#[derive(Default)]
pub(crate) struct Where {
    pub(crate) sql: Vec<String>,
    pub(crate) args: Vec<Arg>,
}

impl Where {
    pub(crate) fn push(&mut self, sql: impl Into<String>, args: impl IntoIterator<Item = Arg>) {
        self.sql.push(sql.into());
        self.args.extend(args);
    }

    pub(crate) fn render(&self) -> String {
        if self.sql.is_empty() { "1".into() } else { self.sql.join(" AND ") }
    }
}

fn placeholders(n: usize) -> String {
    vec!["?"; n.max(1)].join(", ")
}

fn uuid_args(u: &[uuid::Uuid]) -> Vec<Arg> {
    if u.is_empty() { vec![Arg::S(String::new())] } else { u.iter().map(|x| Arg::S(x.to_string())).collect() }
}

fn not_impl(what: &str) -> Error {
    Error::Api { status: 501, code: "not_implemented".into(), message: format!("{what} is not available offline") }
}

fn invalid(e: DomainError) -> Error {
    Error::Api { status: 400, code: "invalid".into(), message: e.to_string() }
}

/// Reference column (correspondent, document_type, storage_path, owner).
fn ref_column(w: &mut Where, col: &str, r: &RefSet) -> Result<()> {
    if !r.any.is_empty() {
        w.push(format!("d.{col} IN ({})", placeholders(r.any.len())), uuid_args(&r.any));
    }
    if !r.all.is_empty() {
        return Err(Error::Api { status: 400, code: "invalid".into(), message: format!("\"all\" is not supported for {col}.") });
    }
    if !r.none.is_empty() {
        w.push(format!("(d.{col} IS NULL OR d.{col} NOT IN ({}))", placeholders(r.none.len())), uuid_args(&r.none));
    }
    match r.is_null {
        Some(true) => w.push(format!("d.{col} IS NULL"), []),
        Some(false) => w.push(format!("d.{col} IS NOT NULL"), []),
        None => {}
    }
    Ok(())
}

fn tags_cond(w: &mut Where, r: &RefSet) {
    if !r.any.is_empty() {
        w.push(
            format!("EXISTS (SELECT 1 FROM document_tag t WHERE t.doc = d.key AND t.tag IN ({}))", placeholders(r.any.len())),
            uuid_args(&r.any),
        );
    }
    for t in &r.all {
        w.push("EXISTS (SELECT 1 FROM document_tag t WHERE t.doc = d.key AND t.tag = ?)", [Arg::S(t.to_string())]);
    }
    if !r.none.is_empty() {
        w.push(
            format!("NOT EXISTS (SELECT 1 FROM document_tag t WHERE t.doc = d.key AND t.tag IN ({}))", placeholders(r.none.len())),
            uuid_args(&r.none),
        );
    }
    match r.is_null {
        Some(true) => w.push("NOT EXISTS (SELECT 1 FROM document_tag t WHERE t.doc = d.key)", []),
        Some(false) => w.push("EXISTS (SELECT 1 FROM document_tag t WHERE t.doc = d.key)", []),
        None => {}
    }
}

fn custom_fields_cond(w: &mut Where, r: &RefSet) {
    const EX: &str = "EXISTS (SELECT 1 FROM json_each(d.json, '$.custom_fields') cf WHERE json_extract(cf.value, '$.field')";
    if !r.any.is_empty() {
        w.push(format!("{EX} IN ({}))", placeholders(r.any.len())), uuid_args(&r.any));
    }
    for f in &r.all {
        w.push(format!("{EX} = ?)"), [Arg::S(f.to_string())]);
    }
    if !r.none.is_empty() {
        w.push(format!("NOT {EX} IN ({}))", placeholders(r.none.len())), uuid_args(&r.none));
    }
    match r.is_null {
        Some(true) => w.push("json_array_length(d.json, '$.custom_fields') = 0", []),
        Some(false) => w.push("json_array_length(d.json, '$.custom_fields') > 0", []),
        None => {}
    }
}

fn date_cond(w: &mut Where, expr: &str, d: &DateRange) {
    if let Some(f) = d.from {
        w.push(format!("{expr} >= ?"), [Arg::S(f.to_string())]);
    }
    if let Some(t) = d.to {
        w.push(format!("{expr} <= ?"), [Arg::S(t.to_string())]);
    }
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

pub(crate) fn build_where(q: &DocQuery) -> Result<Where> {
    let mut w = Where::default();
    w.push("d.in_selection = 1", []);
    let Some(filter) = &q.filter else { return Ok(w) };
    let leaves = filter.conjunction().map_err(|e| match e {
        DomainError::NotImplemented(m) => Error::Api { status: 501, code: "not_implemented".into(), message: m },
        other => invalid(other),
    })?;
    for leaf in leaves {
        match leaf {
            Cond::Text(t) => match t.field {
                TextField::OriginalFilename => {
                    w.push("d.original_filename LIKE ? ESCAPE '\\'", [Arg::S(format!("%{}%", escape_like(&t.query)))]);
                }
                f => {
                    let Some(expr) = fts_expr(&t.query, true) else { continue };
                    let scoped = match f {
                        TextField::Title => format!("title : ({expr})"),
                        TextField::Content => format!("content : ({expr})"),
                        _ => expr,
                    };
                    w.push("d.id IN (SELECT rowid FROM fts WHERE fts MATCH ?)", [Arg::S(scoped)]);
                }
            },
            Cond::Tags(r) => tags_cond(&mut w, r),
            Cond::Correspondent(r) => ref_column(&mut w, "correspondent", r)?,
            Cond::DocumentType(r) => ref_column(&mut w, "document_type", r)?,
            Cond::StoragePath(r) => ref_column(&mut w, "storage_path", r)?,
            Cond::Owner(r) => ref_column(&mut w, "owner", r)?,
            Cond::CustomFields(r) => custom_fields_cond(&mut w, r),
            Cond::CustomFieldQuery(_) => return Err(not_impl("custom_field_query")),
            Cond::SharedByMe(true) => return Err(not_impl("shared_by_me")),
            Cond::SharedByMe(false) => {}
            // folders are not part of the offline replica yet (folder view stage 3)
            Cond::Folder(_) => return Err(not_impl("folder")),
            Cond::Created(d) => date_cond(&mut w, "d.created", d),
            Cond::Added(d) => date_cond(&mut w, "substr(d.added, 1, 10)", d),
            Cond::Modified(d) => date_cond(&mut w, "substr(d.modified, 1, 10)", d),
            Cond::Asn(c) => {
                if let Some(v) = c.eq {
                    w.push("d.asn = ?", [Arg::I(v)]);
                }
                if let Some(v) = c.gt {
                    w.push("d.asn > ?", [Arg::I(v)]);
                }
                if let Some(v) = c.lt {
                    w.push("d.asn < ?", [Arg::I(v)]);
                }
                match c.is_null {
                    Some(true) => w.push("d.asn IS NULL", []),
                    Some(false) => w.push("d.asn IS NOT NULL", []),
                    None => {}
                }
            }
            Cond::Uuids(u) => w.push(format!("d.key IN ({})", placeholders(u.len())), uuid_args(u)),
            Cond::MimeType(m) => w.push("d.mime_type = ?", [Arg::S(m.clone())]),
            Cond::All(_) | Cond::Any(_) | Cond::Not(_) => return Err(not_impl("nested condition")),
        }
    }
    Ok(w)
}

/// Sort expression per field.
fn sort_expr(field: &str) -> &'static str {
    match field {
        "created" => "coalesce(d.created, '')",
        "added" => "coalesce(d.added, '')",
        "modified" => "coalesce(d.modified, '')",
        "title" => "d.title COLLATE NOCASE",
        _ => "d.legacy_id",
    }
}

pub(crate) fn bind_all<'q>(
    mut q: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    args: &[Arg],
) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    for a in args {
        q = match a {
            Arg::S(s) => q.bind(s.clone()),
            Arg::I(i) => q.bind(*i),
        };
    }
    q
}

/// `POST /documents/query` locally → `{"items":[…],"next_cursor":…,"count"?}`.
pub async fn query_documents(r: &Replica, q: &DocQuery) -> Result<Value> {
    let (field, desc) = parse_sort(q.sort.as_deref()).map_err(invalid)?;
    let sort_name = if desc { format!("-{field}") } else { field.to_string() };
    let limit = match q.limit {
        None => DEFAULT_LIMIT,
        Some(n) if (1..=MAX_LIMIT).contains(&n) => n,
        Some(_) => return Err(Error::Api { status: 400, code: "invalid".into(), message: "limit: Must be between 1 and 1000.".into() }),
    };
    let mut include_content = false;
    let mut include_notes = false;
    for i in &q.include {
        match i.as_str() {
            "content" => include_content = true,
            "notes" => include_notes = true,
            "sharing" => {}
            other => return Err(Error::Api { status: 400, code: "invalid".into(), message: format!("Unknown include \"{other}\".") }),
        }
    }
    let mut w = build_where(q)?;
    let base_where = w.render();
    let base_args = w.args.clone();
    let expr = sort_expr(field);
    if let Some(c) = q.cursor.as_deref().filter(|c| !c.is_empty()) {
        let c = KeysetCursor::decode_for(c, &sort_name).map_err(invalid)?;
        let op = if desc { "<" } else { ">" };
        if field == "id" {
            w.push(format!("d.legacy_id {op} ?"), [Arg::I(c.id)]);
        } else {
            let v = c.v.as_str().unwrap_or_default().to_string();
            w.push(format!("({expr} {op} ? OR ({expr} = ? AND d.legacy_id {op} ?))"), [Arg::S(v.clone()), Arg::S(v), Arg::I(c.id)]);
        }
    }
    let dir = if desc { "DESC" } else { "ASC" };
    let sql = format!(
        "SELECT d.json, d.content, d.notes_json, {expr} AS sortv, d.legacy_id FROM document d WHERE {} \
         ORDER BY {expr} {dir}, d.legacy_id {dir} LIMIT ?",
        w.render()
    );
    let mut args = w.args.clone();
    args.push(Arg::I(limit + 1));
    let rows = bind_all(sqlx::query(&sql), &args).fetch_all(r.pool()).await?;
    let has_more = rows.len() as i64 > limit;
    let mut items = Vec::new();
    let mut last: Option<(Value, i64)> = None;
    for row in rows.into_iter().take(limit as usize) {
        let mut v: Value = serde_json::from_str(row.get::<&str, _>(0))?;
        if include_content {
            v["content"] = row.get::<Option<String>, _>(1).map(Value::String).unwrap_or(Value::Null);
        }
        if include_notes {
            v["notes"] = match row.get::<Option<String>, _>(2) {
                Some(n) => serde_json::from_str(&n)?,
                None => json!([]),
            };
        }
        let sortv: Option<String> = if field == "id" { None } else { row.try_get::<Option<String>, _>(3).ok().flatten() };
        last = Some((sortv.map(Value::String).unwrap_or(Value::Null), row.get::<Option<i64>, _>(4).unwrap_or(0)));
        items.push(v);
    }
    let next_cursor = match (has_more, last) {
        (true, Some((v, id))) => Value::String(KeysetCursor { s: sort_name, v, id }.encode()),
        _ => Value::Null,
    };
    let mut out = json!({"items": items, "next_cursor": next_cursor});
    if q.count {
        let n: i64 = bind_all(sqlx::query(&format!("SELECT count(*) FROM document d WHERE {base_where}")), &base_args)
            .fetch_one(r.pool())
            .await?
            .get(0);
        out["count"] = json!(n);
    }
    Ok(out)
}

/// Hits with snippet for the search bar (`/local/search`): rank by bm25.
pub async fn search(r: &Replica, input: &str, limit: i64) -> Result<Value> {
    let Some(expr) = fts_expr(input, true) else { return Ok(json!({"items": []})) };
    let rows = sqlx::query(
        "SELECT d.json, snippet(fts, 1, '[[', ']]', '…', 12) AS snip, bm25(fts) AS rank \
         FROM fts JOIN document d ON d.id = fts.rowid WHERE fts MATCH ? AND d.in_selection = 1 \
         ORDER BY rank LIMIT ?",
    )
    .bind(&expr)
    .bind(limit.clamp(1, MAX_LIMIT))
    .fetch_all(r.pool())
    .await?;
    let mut items = Vec::new();
    for row in rows {
        let v: Value = serde_json::from_str(row.get::<&str, _>(0))?;
        items.push(json!({"document": v, "snippet": row.get::<Option<String>, _>(1)}));
    }
    Ok(json!({"items": items}))
}

/// List of a metadata kind locally (`GET /<plural>`): `sort=name|-name|id|-id`,
/// `name_contains`, cursor like the server (keyset over name + `legacy_id`).
pub async fn list_objects(
    r: &Replica,
    kind: &str,
    cursor: Option<&str>,
    limit: Option<i64>,
    sort: Option<&str>,
    name_contains: Option<&str>,
    count: bool,
) -> Result<Value> {
    let sort = sort.unwrap_or("name");
    let (field, desc) = match sort {
        "name" => ("name", false),
        "-name" => ("name", true),
        "id" => ("id", false),
        "-id" => ("id", true),
        _ => return Err(Error::Api { status: 400, code: "invalid".into(), message: format!("Unknown sort \"{sort}\".") }),
    };
    let limit = match limit {
        None => DEFAULT_LIMIT,
        Some(n) if (1..=MAX_LIMIT).contains(&n) => n,
        Some(_) => return Err(Error::Api { status: 400, code: "invalid".into(), message: "limit: Must be between 1 and 1000.".into() }),
    };
    let mut w = Where::default();
    w.push("o.kind = ?", [Arg::S(kind.to_string())]);
    if let Some(n) = name_contains.filter(|n| !n.is_empty()) {
        w.push("o.name LIKE ? ESCAPE '\\'", [Arg::S(format!("%{}%", escape_like(n)))]);
    }
    let base_where = w.render();
    let base_args = w.args.clone();
    let expr = if field == "name" { "coalesce(o.name, '') COLLATE NOCASE" } else { "o.legacy_id" };
    let op = if desc { "<" } else { ">" };
    if let Some(c) = cursor.filter(|c| !c.is_empty()) {
        let c = KeysetCursor::decode_for(c, sort).map_err(invalid)?;
        if field == "id" {
            w.push(format!("o.legacy_id {op} ?"), [Arg::I(c.id)]);
        } else {
            let v = c.v.as_str().unwrap_or_default().to_string();
            w.push(format!("({expr} {op} ? OR ({expr} = ? AND o.legacy_id {op} ?))"), [Arg::S(v.clone()), Arg::S(v), Arg::I(c.id)]);
        }
    }
    let dir = if desc { "DESC" } else { "ASC" };
    let sql = format!(
        "SELECT o.json, coalesce(o.name, ''), coalesce(o.legacy_id, 0) FROM object o WHERE {} ORDER BY {expr} {dir}, o.legacy_id {dir} LIMIT ?",
        w.render()
    );
    let mut args = w.args.clone();
    args.push(Arg::I(limit + 1));
    let rows = bind_all(sqlx::query(&sql), &args).fetch_all(r.pool()).await?;
    let has_more = rows.len() as i64 > limit;
    let mut items = Vec::new();
    let mut last = None;
    for row in rows.into_iter().take(limit as usize) {
        items.push(serde_json::from_str::<Value>(row.get::<&str, _>(0))?);
        last = Some((row.get::<String, _>(1), row.get::<i64, _>(2)));
    }
    let next_cursor = match (has_more, last) {
        (true, Some((name, id))) => {
            let v = if field == "name" { Value::String(name) } else { Value::Null };
            Value::String(KeysetCursor { s: sort.to_string(), v, id }.encode())
        }
        _ => Value::Null,
    };
    let mut out = json!({"items": items, "next_cursor": next_cursor});
    if count {
        let n: i64 = bind_all(sqlx::query(&format!("SELECT count(*) FROM object o WHERE {base_where}")), &base_args)
            .fetch_one(r.pool())
            .await?
            .get(0);
        out["count"] = json!(n);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Replica;
    use serde_json::json;

    #[test]
    fn fts_expressions() {
        assert_eq!(fts_expr("rech*", false).unwrap(), "\"rech\"*");
        assert_eq!(fts_expr("strom rechnung", false).unwrap(), "\"strom\" AND \"rechnung\"");
        assert_eq!(fts_expr("\"stadt zürich\" 2024", true).unwrap(), "\"stadt zürich\" AND \"2024\"*");
        assert_eq!(fts_expr("a OR b", false).unwrap(), "\"a\" AND \"OR\" AND \"b\"");
        assert_eq!(fts_expr("   ", false), None);
        assert_eq!(fts_expr("- * \"\"", false), None);
        assert_eq!(fts_expr("x\"y", false).unwrap(), "\"x\" AND \"y\"");
        assert_eq!(fts_expr("NEAR(", true).unwrap(), "\"NEAR(\"*");
    }

    fn d(uuid: &str, id: i64, title: &str, created: &str, tags: &[&str], corr: Option<&str>, asn: Option<i64>) -> Value {
        json!({"uuid": uuid, "legacy_id": id, "rev": 1, "title": title, "created": created,
               "added": format!("{created}T10:00:00Z"), "modified": format!("{created}T10:00:00Z"),
               "correspondent": corr, "document_type": null, "storage_path": null, "tags": tags,
               "custom_fields": [], "asn": asn, "owner": null, "page_count": 1, "mime_type": "application/pdf",
               "original_filename": format!("{title}.pdf"), "notes_count": 0,
               "files": {"original": {"checksum": "x", "size": 10}, "archive": null, "thumbnail": {"etag": null}},
               "access": {"can_view": true, "can_change": true, "can_delete": true},
               "content": format!("Inhalt von {title}")})
    }

    const T1: &str = "00000000-0000-0000-0000-0000000000a1";
    const T2: &str = "00000000-0000-0000-0000-0000000000a2";
    const C1: &str = "00000000-0000-0000-0000-0000000000c1";

    async fn fixture() -> Replica {
        let r = Replica::open_in_memory().await.unwrap();
        r.upsert_object("tag", &json!({"uuid": T1, "legacy_id": 1, "rev": 1, "name": "Steuern"})).await.unwrap();
        r.upsert_object("tag", &json!({"uuid": T2, "legacy_id": 2, "rev": 1, "name": "Auto"})).await.unwrap();
        r.upsert_object("correspondent", &json!({"uuid": C1, "legacy_id": 1, "rev": 1, "name": "Steueramt"})).await.unwrap();
        let docs: Vec<Value> = (1..=25)
            .map(|i| {
                let tags: Vec<&str> = match i % 3 {
                    0 => vec![T1],
                    1 => vec![T1, T2],
                    _ => vec![],
                };
                d(
                    &format!("00000000-0000-0000-0000-{i:012}"),
                    i,
                    &format!("Dokument {i:02}"),
                    &format!("2024-01-{i:02}"),
                    &tags,
                    if i % 5 == 0 { Some(C1) } else { None },
                    if i % 2 == 0 { Some(1000 + i) } else { None },
                )
            })
            .collect();
        r.upsert_documents(&docs, true, true).await.unwrap();
        r
    }

    fn q(v: Value) -> DocQuery {
        serde_json::from_value(v).unwrap()
    }

    #[tokio::test]
    async fn paging_is_stable_for_all_sorts() {
        let r = fixture().await;
        for sort in ["created", "-created", "title", "-title", "id", "-id", "added", "-modified"] {
            let mut seen = Vec::new();
            let mut cursor: Option<String> = None;
            loop {
                let page = query_documents(&r, &q(json!({"sort": sort, "limit": 7, "cursor": cursor}))).await.unwrap();
                for it in page["items"].as_array().unwrap() {
                    seen.push(it["legacy_id"].as_i64().unwrap());
                }
                match page["next_cursor"].as_str() {
                    Some(c) => cursor = Some(c.to_string()),
                    None => break,
                }
            }
            assert_eq!(seen.len(), 25, "sort {sort}");
            let mut sorted = seen.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), 25, "no duplicates for {sort}");
            if sort == "-created" || sort == "-id" {
                assert_eq!(seen.first(), Some(&25));
            }
        }
        // cursor belongs to the sort order
        let p = query_documents(&r, &q(json!({"sort": "title", "limit": 2}))).await.unwrap();
        let c = p["next_cursor"].as_str().unwrap();
        assert!(query_documents(&r, &q(json!({"sort": "-title", "cursor": c}))).await.is_err());
    }

    #[tokio::test]
    async fn filters() {
        let r = fixture().await;
        let n = |v: Value| {
            let r = r.clone();
            async move { query_documents(&r, &q(v)).await.unwrap()["count"].as_i64().unwrap() }
        };
        assert_eq!(n(json!({"count": true})).await, 25);
        assert_eq!(n(json!({"count": true, "filter": {"tags": {"any": [T1]}}})).await, 17);
        assert_eq!(n(json!({"count": true, "filter": {"tags": {"all": [T1, T2]}}})).await, 9);
        assert_eq!(n(json!({"count": true, "filter": {"tags": {"none": [T1]}}})).await, 8);
        assert_eq!(n(json!({"count": true, "filter": {"tags": {"is_null": true}}})).await, 8);
        assert_eq!(n(json!({"count": true, "filter": {"correspondent": {"any": [C1]}}})).await, 5);
        assert_eq!(n(json!({"count": true, "filter": {"correspondent": {"is_null": true}}})).await, 20);
        assert_eq!(n(json!({"count": true, "filter": {"created": {"from": "2024-01-10", "to": "2024-01-12"}}})).await, 3);
        assert_eq!(n(json!({"count": true, "filter": {"added": {"to": "2024-01-02"}}})).await, 2);
        assert_eq!(n(json!({"count": true, "filter": {"asn": {"is_null": false}}})).await, 12);
        assert_eq!(n(json!({"count": true, "filter": {"asn": {"gt": 1020}}})).await, 2);
        // full text: prefix, names (correspondent/tag), combination
        assert_eq!(n(json!({"count": true, "filter": {"text": {"query": "dokum"}}})).await, 25);
        assert_eq!(n(json!({"count": true, "filter": {"text": {"query": "steueramt"}}})).await, 5);
        assert_eq!(n(json!({"count": true, "filter": {"all": [{"text": {"query": "auto"}}, {"correspondent": {"any": [C1]}}]}})).await, 2);
        assert_eq!(n(json!({"count": true, "filter": {"text": {"query": "\"Dokument 07\"", "field": "title"}}})).await, 1);
        assert_eq!(n(json!({"count": true, "filter": {"text": {"query": "inhalt", "field": "title"}}})).await, 0);
        assert_eq!(n(json!({"count": true, "filter": {"text": {"query": "ent 1", "field": "original_filename"}}})).await, 10);
        // not offline
        let e = query_documents(&r, &q(json!({"filter": {"any": [{"mime_type": "x"}]}}))).await.unwrap_err();
        assert!(matches!(e, Error::Api { status: 501, .. }));
        let e = query_documents(&r, &q(json!({"filter": {"custom_field_query": ["x", "exists", true]}}))).await.unwrap_err();
        assert!(matches!(e, Error::Api { status: 501, .. }));
        // include
        let p = query_documents(&r, &q(json!({"limit": 1, "include": ["content", "notes"]}))).await.unwrap();
        assert!(p["items"][0]["content"].as_str().unwrap().starts_with("Inhalt"));
        assert!(p["items"][0]["notes"].is_array());
        let p = query_documents(&r, &q(json!({"limit": 1}))).await.unwrap();
        assert!(p["items"][0].get("content").is_none());
    }

    #[tokio::test]
    async fn search_and_object_lists() {
        let r = fixture().await;
        let s = search(&r, "inhalt dokument 07", 10).await.unwrap();
        assert!(!s["items"].as_array().unwrap().is_empty());
        assert!(s["items"][0]["snippet"].as_str().unwrap().contains("[["));
        let l = list_objects(&r, "tag", None, Some(1), None, None, true).await.unwrap();
        assert_eq!(l["items"][0]["name"], "Auto");
        assert_eq!(l["count"], 2);
        let l2 = list_objects(&r, "tag", l["next_cursor"].as_str(), Some(1), None, None, false).await.unwrap();
        assert_eq!(l2["items"][0]["name"], "Steuern");
        assert!(l2["next_cursor"].is_null());
        let l3 = list_objects(&r, "tag", None, None, Some("-id"), Some("teu"), false).await.unwrap();
        assert_eq!(l3["items"].as_array().unwrap().len(), 1);
    }
}
