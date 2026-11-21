//! Offline v1 from the replica (the light UI speaks v1 today, docs/FRONTEND-LIGHT.md §6).
//!
//! Read-only and only what the light UI needs for browsing, searching and opening: document list with the
//! v1 filters/sort orders (`page`, `page_size`, `ordering`, `tags__id__all|in|none`, `is_tagged`,
//! `correspondent__id[__in|__none]`, `document_type__…`, `storage_path__…`, `owner__…`,
//! `title_content`, `query`, `title__icontains`, `content__icontains`,
//! `original_filename__icontains`, `created|added|modified[__date]__gte|lte|gt|lt`,
//! `archive_serial_number[__gt|__lt|__isnull]`, `id__in`, `mime_type`, `is_in_inbox`,
//! `custom_fields__id__…`, `has_custom_fields`) including `all` and `selection_data`,
//! document (`fields`), notes, metadata (from the replica), suggestions (empty), master data
//! (synthesized if not in the response cache), `ui_settings` (fallback from `/me`).
//! v1 references = `legacy_id` of the objects; full text = FTS5 (like [`crate::query`]).

use std::collections::HashMap;

use pl_domain::query::{AsnCond, Cond, DateRange, DocQuery, RefSet, TextCond, TextField};
use serde_json::{Map, Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::query::{Arg, bind_all, build_where};
use crate::store::Replica;

/// Content in lists as upstream (`truncate_content` → 550 characters).
pub const TRUNCATE: usize = 550;

/// legacy_id ↔ key.
#[derive(Default)]
pub struct Ids {
    to_id: HashMap<String, i64>,
    to_key: HashMap<(String, i64), String>,
}

impl Ids {
    pub async fn load(r: &Replica) -> Result<Ids> {
        let mut i = Ids::default();
        for (kind, key, id) in r.legacy_ids().await? {
            i.to_id.insert(key.clone(), id);
            i.to_key.insert((kind, id), key);
        }
        Ok(i)
    }

    pub fn id(&self, key: Option<&str>) -> Value {
        key.and_then(|k| self.to_id.get(k)).map(|i| json!(i)).unwrap_or(Value::Null)
    }

    pub fn key(&self, kind: &str, id: i64) -> Option<&String> {
        self.to_key.get(&(kind.to_string(), id))
    }

    /// v1-IDs → UUIDs; unbekannte → Nil-UUID (trifft nichts).
    fn uuids(&self, kind: &str, ids: &[i64]) -> Vec<Uuid> {
        ids.iter().map(|i| self.key(kind, *i).and_then(|k| Uuid::parse_str(k).ok()).unwrap_or(Uuid::nil())).collect()
    }
}

fn ids_param(v: &str) -> Vec<i64> {
    v.split(',').filter_map(|x| x.trim().parse().ok()).collect()
}

fn truthy(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

fn date(v: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(v.get(..10)?, "%Y-%m-%d").ok()
}

fn invalid(m: impl Into<String>) -> Error {
    Error::Api { status: 400, code: "invalid".into(), message: m.into() }
}

/// v1-Filterparameter → `DocQuery`-Bedingungen (UND).
pub fn conditions(params: &[(String, String)], ids: &Ids, inbox_tags: &[i64]) -> Result<Vec<Cond>> {
    let mut out = Vec::new();
    let refkind = |prefix: &str| match prefix {
        "tags" => Some("tag"),
        "correspondent" => Some("correspondent"),
        "document_type" => Some("document_type"),
        "storage_path" => Some("storage_path"),
        "owner" => Some("user"),
        "custom_fields" => Some("custom_field"),
        _ => None,
    };
    let mk = |prefix: &str, rs: RefSet| -> Option<Cond> {
        Some(match prefix {
            "tags" => Cond::Tags(rs),
            "correspondent" => Cond::Correspondent(rs),
            "document_type" => Cond::DocumentType(rs),
            "storage_path" => Cond::StoragePath(rs),
            "owner" => Cond::Owner(rs),
            "custom_fields" => Cond::CustomFields(rs),
            _ => return None,
        })
    };
    for (k, v) in params {
        let k = k.as_str();
        let v = v.trim();
        // Referenzen: <x>__id, <x>__id__in, <x>__id__all, <x>__id__none, <x>__isnull
        if let Some((prefix, rest)) = k.split_once("__")
            && let Some(kind) = refkind(prefix)
        {
            let list = ids.uuids(kind, &ids_param(v));
            let rs = match rest {
                "id" if prefix == "tags" => RefSet { all: list, ..Default::default() },
                "id" | "id__in" => RefSet { any: list, ..Default::default() },
                "id__all" => RefSet { all: list, ..Default::default() },
                "id__none" => RefSet { none: list, ..Default::default() },
                "isnull" => RefSet { is_null: Some(truthy(v)), ..Default::default() },
                _ => return Err(invalid(format!("filter {k} is not available offline"))),
            };
            if let Some(c) = mk(prefix, rs) {
                out.push(c);
            }
            continue;
        }
        let text = |field| Cond::Text(TextCond { query: v.to_string(), field });
        match k {
            "is_tagged" => out.push(Cond::Tags(RefSet { is_null: Some(!truthy(v)), ..Default::default() })),
            "has_custom_fields" => out.push(Cond::CustomFields(RefSet { is_null: Some(!truthy(v)), ..Default::default() })),
            "is_in_inbox" if truthy(v) => {
                out.push(Cond::Tags(RefSet { any: ids.uuids("tag", inbox_tags), ..Default::default() }));
            }
            "title_content" | "query" | "search" => out.push(text(TextField::TitleContent)),
            "title__icontains" | "title__istartswith" => out.push(text(TextField::Title)),
            "content__icontains" => out.push(text(TextField::Content)),
            "original_filename__icontains" => out.push(text(TextField::OriginalFilename)),
            "archive_serial_number" => out.push(Cond::Asn(AsnCond { eq: v.parse().ok(), ..Default::default() })),
            "archive_serial_number__gt" => out.push(Cond::Asn(AsnCond { gt: v.parse().ok(), ..Default::default() })),
            "archive_serial_number__lt" => out.push(Cond::Asn(AsnCond { lt: v.parse().ok(), ..Default::default() })),
            "archive_serial_number__gte" => out.push(Cond::Asn(AsnCond { gt: v.parse::<i64>().ok().map(|n| n - 1), ..Default::default() })),
            "archive_serial_number__lte" => out.push(Cond::Asn(AsnCond { lt: v.parse::<i64>().ok().map(|n| n + 1), ..Default::default() })),
            "archive_serial_number__isnull" => out.push(Cond::Asn(AsnCond { is_null: Some(truthy(v)), ..Default::default() })),
            "id__in" | "id" => out.push(Cond::Uuids(ids.uuids("document", &ids_param(v)))),
            "mime_type" => out.push(Cond::MimeType(v.to_string())),
            "custom_field_query" | "more_like_id" | "shared_by__id" | "fulltext_query" => {
                return Err(Error::Api { status: 501, code: "not_implemented".into(), message: format!("{k} is not available offline") });
            }
            _ => {
                // Datumsgrenzen: created|added|modified [__date] __gte|__lte|__gt|__lt
                let mut parts = k.split("__");
                let field = parts.next().unwrap_or("");
                let mut op = parts.next().unwrap_or("");
                if op == "date" || op == "year" {
                    op = parts.next().unwrap_or("");
                }
                if matches!(field, "created" | "added" | "modified") && matches!(op, "gte" | "lte" | "gt" | "lt") {
                    let Some(d) = date(v) else { return Err(invalid(format!("{k}: date"))) };
                    let (from, to) = match op {
                        "gte" => (Some(d), None),
                        "gt" => (d.succ_opt(), None),
                        "lte" => (None, Some(d)),
                        _ => (None, d.pred_opt()),
                    };
                    let r = DateRange { from, to };
                    out.push(match field {
                        "created" => Cond::Created(r),
                        "added" => Cond::Added(r),
                        _ => Cond::Modified(r),
                    });
                }
                // sonstige Parameter (page, ordering, fields, truncate_content, …) ignorieren
            }
        }
    }
    Ok(out)
}

/// v1-Sortierung → SQL-Ausdruck.
fn order_expr(field: &str) -> &'static str {
    match field {
        "created" | "created_date" => "coalesce(d.created, '')",
        "added" => "coalesce(d.added, '')",
        "modified" => "coalesce(d.modified, '')",
        "title" => "d.title COLLATE NOCASE",
        "id" => "d.legacy_id",
        "archive_serial_number" | "asn" => "d.asn",
        "correspondent__name" => "(SELECT o.name FROM object o WHERE o.kind = 'correspondent' AND o.key = d.correspondent) COLLATE NOCASE",
        "document_type__name" => "(SELECT o.name FROM object o WHERE o.kind = 'document_type' AND o.key = d.document_type) COLLATE NOCASE",
        "storage_path__name" => "(SELECT o.name FROM object o WHERE o.kind = 'storage_path' AND o.key = d.storage_path) COLLATE NOCASE",
        "owner" => "d.owner",
        "page_count" => "json_extract(d.json, '$.page_count')",
        "num_notes" | "notes" => "json_extract(d.json, '$.notes_count')",
        "mime_type" => "d.mime_type",
        _ => "coalesce(d.created, '')",
    }
}

fn notes_v1(notes: &Value, ids: &Ids) -> Value {
    Value::Array(
        notes
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|n| {
                        let user = n.get("user").filter(|u| !u.is_null()).map(
                            |u| json!({"id": ids.id(u["uuid"].as_str()), "username": u["username"], "first_name": "", "last_name": ""}),
                        );
                        json!({"id": n["legacy_id"], "note": n["note"], "created": n["created"], "user": user})
                    })
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// v2-Dokument (Replikat) → v1-Dokument.
pub fn doc_v1(d: &Value, ids: &Ids, content: Option<&str>, notes: Option<&Value>, truncate: bool) -> Value {
    let s = |k: &str| d.get(k).and_then(Value::as_str);
    let tags: Vec<Value> =
        d["tags"].as_array().map(|a| a.iter().map(|t| ids.id(t.as_str())).filter(|v| !v.is_null()).collect()).unwrap_or_default();
    let cfs: Vec<Value> = d["custom_fields"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|c| {
                    // Document link: UUIDs → IDs
                    let value = match c["value"].as_array() {
                        Some(list) if list.iter().all(|x| x.as_str().is_some_and(|s| Uuid::parse_str(s).is_ok())) && !list.is_empty() => {
                            Value::Array(list.iter().map(|x| ids.id(x.as_str())).collect())
                        }
                        _ => c["value"].clone(),
                    };
                    json!({"field": ids.id(c["field"].as_str()), "value": value})
                })
                .collect()
        })
        .unwrap_or_default();
    let mut content = content.unwrap_or("").to_string();
    if truncate && content.chars().count() > TRUNCATE {
        content = content.chars().take(TRUNCATE).collect();
    }
    let archived = (!d["files"]["archive"].is_null()).then(|| {
        let o = s("original_filename").unwrap_or("document");
        let stem = o.rsplit_once('.').map(|(a, _)| a).unwrap_or(o);
        format!("{stem}.pdf")
    });
    json!({
        "id": d["legacy_id"],
        "correspondent": ids.id(s("correspondent")),
        "document_type": ids.id(s("document_type")),
        "storage_path": ids.id(s("storage_path")),
        "title": d["title"],
        "content": content,
        "tags": tags,
        "created": d["created"],
        "created_date": d["created"],
        "modified": d["modified"],
        "added": d["added"],
        "deleted_at": null,
        "archive_serial_number": d["asn"],
        "original_file_name": d["original_filename"],
        "archived_file_name": archived,
        "owner": ids.id(d["owner"]["uuid"].as_str()),
        "user_can_change": d["access"]["can_change"].as_bool().unwrap_or(false),
        "is_shared_by_requester": false,
        "notes": notes.map(|n| notes_v1(n, ids)).unwrap_or(json!([])),
        "custom_fields": cfs,
        "page_count": d["page_count"],
        "mime_type": d["mime_type"],
        "duplicate_documents": [],
        "root_document": null,
        "versions": [],
    })
}

/// Only requested fields (`fields=a,b`).
fn pick(v: Value, fields: Option<&str>) -> Value {
    let Some(f) = fields.filter(|f| !f.trim().is_empty()) else { return v };
    let want: Vec<&str> = f.split(',').map(str::trim).collect();
    match v {
        Value::Object(m) => Value::Object(m.into_iter().filter(|(k, _)| want.contains(&k.as_str())).collect()),
        other => other,
    }
}

fn matching_v1(m: &Value) -> (Value, Value, Value) {
    let alg = m["algorithm"].as_str().map(|a| pl_domain::model::matching_code(a).unwrap_or(0)).unwrap_or(0);
    (json!(alg), m["pattern"].clone(), m["is_insensitive"].clone())
}

fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('-') {
            s.push('-');
        }
    }
    s.trim_matches('-').to_string()
}

/// v2-Objekt → v1-Objekt.
pub fn obj_v1(kind: &str, o: &Value, ids: &Ids, counts: &HashMap<String, i64>) -> Value {
    let key = o["uuid"].as_str().unwrap_or("");
    let id = o["legacy_id"].clone();
    let name = o["name"].as_str().unwrap_or("");
    let owner = ids.id(o["owner"]["uuid"].as_str());
    let can_change = o["access"]["can_change"].as_bool().unwrap_or(true);
    let count = counts.get(key).copied().unwrap_or(0);
    match kind {
        "tag" => {
            let (alg, pat, ins) = matching_v1(&o["matching"]);
            json!({"id": id, "slug": slug(name), "name": name, "color": o["color"], "text_color": o["text_color"],
                   "match": pat, "matching_algorithm": alg, "is_insensitive": ins, "is_inbox_tag": o["is_inbox_tag"],
                   "document_count": count, "owner": owner, "user_can_change": can_change,
                   "parent": ids.id(o["parent"].as_str()), "children": []})
        }
        "correspondent" | "document_type" | "storage_path" => {
            let (alg, pat, ins) = matching_v1(&o["matching"]);
            let mut v = json!({"id": id, "slug": slug(name), "name": name, "match": pat, "matching_algorithm": alg,
                               "is_insensitive": ins, "document_count": count, "owner": owner, "user_can_change": can_change});
            if kind == "storage_path" {
                v["path"] = o["path"].clone();
            }
            if kind == "correspondent" {
                v["last_correspondence"] = Value::Null;
            }
            v
        }
        "custom_field" => {
            json!({"id": id, "name": name, "data_type": o["data_type"], "extra_data": o["extra_data"], "document_count": count})
        }
        "saved_view" => json!({"id": id, "name": name, "sort_field": o["sort_field"], "sort_reverse": o["sort_reverse"],
                               "filter_rules": o["filter_rules"], "page_size": o["page_size"], "display_mode": o["display_mode"],
                               "display_fields": o["display_fields"], "owner": owner, "user_can_change": can_change}),
        "user" => json!({"id": id, "username": o["username"], "first_name": o["first_name"], "last_name": o["last_name"]}),
        _ => json!({"id": id, "name": name}),
    }
}

/// v1-Pfadsegment → Art.
pub fn kind_of_v1(seg: &str) -> Option<&'static str> {
    Some(match seg {
        "tags" => "tag",
        "correspondents" => "correspondent",
        "document_types" => "document_type",
        "storage_paths" => "storage_path",
        "custom_fields" => "custom_field",
        "saved_views" => "saved_view",
        "users" => "user",
        "groups" => "group",
        _ => return None,
    })
}

/// Local document count per reference (for `document_count`).
async fn ref_counts(r: &Replica, kind: &str) -> Result<HashMap<String, i64>> {
    let sql = match kind {
        "tag" => "SELECT t.tag, count(*) FROM document_tag t JOIN document d ON d.key = t.doc AND d.in_selection = 1 GROUP BY t.tag",
        "correspondent" => "SELECT correspondent, count(*) FROM document WHERE in_selection = 1 AND correspondent IS NOT NULL GROUP BY 1",
        "document_type" => "SELECT document_type, count(*) FROM document WHERE in_selection = 1 AND document_type IS NOT NULL GROUP BY 1",
        "storage_path" => "SELECT storage_path, count(*) FROM document WHERE in_selection = 1 AND storage_path IS NOT NULL GROUP BY 1",
        _ => return Ok(HashMap::new()),
    };
    let rows = sqlx::query(sql).fetch_all(r.pool()).await?;
    Ok(rows.into_iter().map(|r| (r.get::<String, _>(0), r.get::<i64, _>(1))).collect())
}

/// `/api/<objects>/` (list) from the replica.
pub async fn list_objects(r: &Replica, kind: &str) -> Result<Value> {
    let ids = Ids::load(r).await?;
    let counts = ref_counts(r, kind).await?;
    let rows = sqlx::query("SELECT json FROM object WHERE kind = ? ORDER BY name COLLATE NOCASE, legacy_id")
        .bind(kind.to_string())
        .fetch_all(r.pool())
        .await?;
    let mut results = Vec::new();
    let mut all = Vec::new();
    for row in rows {
        let o: Value = serde_json::from_str(row.get::<&str, _>(0))?;
        all.push(o["legacy_id"].clone());
        results.push(obj_v1(kind, &o, &ids, &counts));
    }
    Ok(json!({"count": results.len(), "next": null, "previous": null, "all": all, "results": results}))
}

/// `/api/<objects>/<id>/`.
pub async fn get_object(r: &Replica, kind: &str, id: i64) -> Result<Value> {
    let ids = Ids::load(r).await?;
    let key = ids.key(kind, id).cloned().ok_or(Error::NotFound)?;
    let o = r.object(kind, &key).await?.ok_or(Error::NotFound)?;
    Ok(obj_v1(kind, &o, &ids, &ref_counts(r, kind).await?))
}

fn inbox_tag_ids(r_objs: &[Value]) -> Vec<i64> {
    r_objs.iter().filter(|t| t["is_inbox_tag"].as_bool() == Some(true)).filter_map(|t| t["legacy_id"].as_i64()).collect()
}

/// `/api/documents/` (list, paginated) from the replica.
pub async fn list_documents(r: &Replica, path: &str, params: &[(String, String)]) -> Result<Value> {
    let ids = Ids::load(r).await?;
    let get = |k: &str| params.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
    let tags: Vec<Value> = sqlx::query_scalar::<_, String>("SELECT json FROM object WHERE kind = 'tag'")
        .fetch_all(r.pool())
        .await?
        .iter()
        .filter_map(|j| serde_json::from_str(j).ok())
        .collect();
    let conds = conditions(params, &ids, &inbox_tag_ids(&tags))?;
    let q = DocQuery { filter: Some(Cond::All(conds)), ..Default::default() };
    let w = build_where(&q)?;
    let where_sql = w.render();
    let page: i64 = get("page").and_then(|p| p.parse().ok()).filter(|p| *p >= 1).unwrap_or(1);
    let page_size: i64 = get("page_size").and_then(|p| p.parse().ok()).filter(|p| *p >= 1).unwrap_or(25).min(100_000);
    let ordering = get("ordering").unwrap_or("-created");
    let (field, desc) = match ordering.strip_prefix('-') {
        Some(f) => (f, true),
        None => (ordering, false),
    };
    let expr = order_expr(field);
    let dir = if desc { "DESC" } else { "ASC" };
    let order = format!("ORDER BY {expr} {dir}, d.legacy_id {dir}");
    let all: Vec<i64> = bind_all(sqlx::query(&format!("SELECT d.legacy_id FROM document d WHERE {where_sql} {order}")), &w.args)
        .fetch_all(r.pool())
        .await?
        .into_iter()
        .map(|r| r.get::<Option<i64>, _>(0).unwrap_or(0))
        .collect();
    let count = all.len() as i64;
    let offset = (page - 1) * page_size;
    if offset > 0 && offset >= count {
        return Err(Error::Api { status: 404, code: "not_found".into(), message: "Invalid page.".into() });
    }
    let mut args = w.args.clone();
    args.push(Arg::I(page_size));
    args.push(Arg::I(offset));
    let rows = bind_all(
        sqlx::query(&format!("SELECT d.json, d.content, d.notes_json FROM document d WHERE {where_sql} {order} LIMIT ? OFFSET ?")),
        &args,
    )
    .fetch_all(r.pool())
    .await?;
    let truncate = get("truncate_content").is_some_and(truthy);
    let fields = get("fields");
    let mut results = Vec::new();
    for row in rows {
        let d: Value = serde_json::from_str(row.get::<&str, _>(0))?;
        let notes: Option<Value> = row.get::<Option<String>, _>(2).and_then(|n| serde_json::from_str(&n).ok());
        let content: Option<String> = row.get(1);
        results.push(pick(doc_v1(&d, &ids, content.as_deref(), notes.as_ref(), truncate), fields));
    }
    let link = |p: i64| -> Value {
        let mut ser = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in params.iter().filter(|(k, _)| k != "page") {
            ser.append_pair(k, v);
        }
        ser.append_pair("page", &p.to_string());
        json!(format!("{path}?{}", ser.finish()))
    };
    let mut out = json!({
        "count": count,
        "next": if offset + page_size < count { link(page + 1) } else { Value::Null },
        "previous": if page > 1 { link(page - 1) } else { Value::Null },
        "all": all,
        "results": results,
    });
    if get("include_selection_data").is_some_and(truthy) {
        out["selection_data"] = selection_data(r, &where_sql, &w.args, &ids).await?;
    }
    Ok(out)
}

async fn selection_data(r: &Replica, where_sql: &str, args: &[Arg], ids: &Ids) -> Result<Value> {
    let mut sd = Map::new();
    for (name, sql) in [
        (
            "selected_correspondents",
            format!("SELECT d.correspondent, count(*) FROM document d WHERE {where_sql} AND d.correspondent IS NOT NULL GROUP BY 1"),
        ),
        (
            "selected_document_types",
            format!("SELECT d.document_type, count(*) FROM document d WHERE {where_sql} AND d.document_type IS NOT NULL GROUP BY 1"),
        ),
        (
            "selected_storage_paths",
            format!("SELECT d.storage_path, count(*) FROM document d WHERE {where_sql} AND d.storage_path IS NOT NULL GROUP BY 1"),
        ),
        (
            "selected_tags",
            format!("SELECT t.tag, count(*) FROM document_tag t JOIN document d ON d.key = t.doc WHERE {where_sql} GROUP BY 1"),
        ),
        (
            "selected_custom_fields",
            format!(
                "SELECT json_extract(cf.value, '$.field'), count(*) FROM document d, json_each(d.json, '$.custom_fields') cf WHERE {where_sql} GROUP BY 1"
            ),
        ),
    ] {
        let rows = bind_all(sqlx::query(&sql), args).fetch_all(r.pool()).await?;
        let list: Vec<Value> = rows
            .into_iter()
            .filter_map(|row| {
                let key: Option<String> = row.try_get(0).ok().flatten();
                let id = ids.id(key.as_deref());
                (!id.is_null()).then(|| json!({"id": id, "document_count": row.get::<i64, _>(1)}))
            })
            .collect();
        sd.insert(name.into(), Value::Array(list));
    }
    Ok(Value::Object(sd))
}

/// Key of a document from the v1 ID.
pub async fn doc_key(r: &Replica, id: i64) -> Result<String> {
    sqlx::query_scalar::<_, String>("SELECT key FROM document WHERE legacy_id = ? AND in_selection = 1")
        .bind(id)
        .fetch_optional(r.pool())
        .await?
        .ok_or(Error::NotFound)
}

/// `/api/documents/<id>/` (+ `fields`).
pub async fn get_document(r: &Replica, id: i64, fields: Option<&str>) -> Result<Value> {
    let key = doc_key(r, id).await?;
    let full = r.document_full(&key, true, true).await?.ok_or(Error::NotFound)?;
    let ids = Ids::load(r).await?;
    Ok(pick(doc_v1(&full, &ids, full["content"].as_str(), Some(&full["notes"]), false), fields))
}

pub async fn notes(r: &Replica, id: i64) -> Result<Value> {
    let key = doc_key(r, id).await?;
    let full = r.document_full(&key, false, true).await?.ok_or(Error::NotFound)?;
    Ok(notes_v1(&full["notes"], &Ids::load(r).await?))
}

/// `/api/documents/<id>/metadata/` from the known file data (without file metadata).
pub async fn metadata(r: &Replica, id: i64) -> Result<Value> {
    let key = doc_key(r, id).await?;
    let d = r.document_json(&key).await?.ok_or(Error::NotFound)?;
    let has_archive = !d["files"]["archive"].is_null();
    Ok(json!({
        "original_checksum": d["files"]["original"]["checksum"],
        "original_size": d["files"]["original"]["size"],
        "original_mime_type": d["mime_type"],
        "media_filename": d["original_filename"],
        "has_archive_version": has_archive,
        "original_metadata": [],
        "archive_checksum": if has_archive { d["files"]["archive"]["checksum"].clone() } else { Value::Null },
        "archive_media_filename": null,
        "original_filename": d["original_filename"],
        "archive_size": if has_archive { d["files"]["archive"]["size"].clone() } else { Value::Null },
        "archive_metadata": if has_archive { json!([]) } else { Value::Null },
        "lang": "en",
    }))
}

/// `ui_settings` without cache: from `/me` (+ v2 settings).
pub async fn ui_settings(r: &Replica) -> Result<Value> {
    let me: Value = match r.remote_text("me_json").await? {
        Some(j) => serde_json::from_str(&j)?,
        None => return Err(Error::NotFound),
    };
    let settings = match r.remote_text("ui_settings").await? {
        Some(j) => serde_json::from_str::<Value>(&j)?.get("settings").cloned().unwrap_or(json!({})),
        None => json!({}),
    };
    let perms: Vec<Value> = me["permissions"]
        .as_array()
        .map(|a| a.iter().filter_map(|p| p.as_str()).map(|p| json!(p.split_once('.').map(|(_, c)| c).unwrap_or(p))).collect())
        .unwrap_or_default();
    Ok(json!({
        "user": {"id": me["legacy_id"], "username": me["username"], "first_name": me["first_name"], "last_name": me["last_name"],
                 "is_staff": me["is_staff"], "is_superuser": me["is_superuser"]},
        "settings": settings,
        "permissions": perms,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_params() {
        assert_eq!(slug("Steuer Amt Zürich!"), "steuer-amt-zürich");
        assert_eq!(ids_param("1, 2,x,3"), vec![1, 2, 3]);
    }

    #[test]
    fn conditions_from_v1() {
        let mut ids = Ids::default();
        let t = "00000000-0000-4000-8000-000000000001".to_string();
        ids.to_id.insert(t.clone(), 7);
        ids.to_key.insert(("tag".into(), 7), t.clone());
        let p = |k: &str, v: &str| (k.to_string(), v.to_string());
        let c = conditions(
            &[
                p("tags__id__all", "7,8"),
                p("title_content", "rech"),
                p("created__date__gte", "2024-01-01"),
                p("page", "2"),
                p("is_tagged", "0"),
            ],
            &ids,
            &[],
        )
        .unwrap();
        assert_eq!(c.len(), 4);
        match &c[0] {
            Cond::Tags(rs) => assert_eq!(rs.all, vec![Uuid::parse_str(&t).unwrap(), Uuid::nil()]),
            other => panic!("{other:?}"),
        }
        assert!(matches!(&c[3], Cond::Tags(RefSet { is_null: Some(true), .. })));
        assert!(conditions(&[p("custom_field_query", "[]")], &ids, &[]).is_err());
    }
}
