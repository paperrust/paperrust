//! Side tables and queries. Visibility is decided by the caller: [`Visibility`] contains
//! subqueries (`SELECT id …`) that `pl-store` builds from `pl-authz` – the same as for the
//! document and object lists of the API.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::areas::{Area, AreaValues, Candidate, DocKeys, MAX_AREAS, ResolvedRules, Rules, Scope};
use crate::build::{Amount, DateRow, DocRow, ItemState, RefOut, ReminderRef};
use crate::fields::{FieldInfo, FieldKinds, parse_monetary};
use crate::{DateKind, Result, Source, TimelineError, invalid};

/// Migration (name, SQL) for `paperrust manage migrate`.
pub const MIGRATION: (&str, &str) = ("pl_timeline.0001_paperrust_design2", include_str!("../migrations/0001_paperrust_design2.sql"));

pub mod migrations {
    pub const ALL: &[(&str, &str)] = &[super::MIGRATION];
}

/// sync kind codes (`pl_domain::Kind::code`).
pub const KIND_DOCUMENT: i16 = 1;
pub const KIND_TAG: i16 = 2;
pub const KIND_CORRESPONDENT: i16 = 3;
pub const KIND_DOCUMENT_TYPE: i16 = 4;
pub const KIND_STORAGE_PATH: i16 = 5;
pub const KIND_CUSTOM_FIELD: i16 = 6;

static READY: AtomicBool = AtomicBool::new(false);

/// SQL file into statements (without `--` comments; statements end with `;` at the end of a line).
fn statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in sql.lines() {
        let l = line.split("--").next().unwrap_or("").trim_end();
        if l.trim().is_empty() {
            continue;
        }
        cur.push_str(l);
        cur.push('\n');
        if l.ends_with(';') {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Create tables (idempotent, serialised via an advisory lock).
pub async fn migrate(pool: &PgPool) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('paperrust_design2'))").execute(&mut *tx).await?;
    // Single statements (raw_sql on a transaction does not yield a generally `Send` future)
    for stmt in statements(MIGRATION.1) {
        sqlx::query(&stmt).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    READY.store(true, Ordering::Relaxed);
    Ok(())
}

/// [`migrate`] once per process.
pub async fn ensure(pool: &PgPool) -> Result<()> {
    if !READY.load(Ordering::Relaxed) {
        migrate(pool).await?;
    }
    Ok(())
}

/// Visibility as SQL subqueries (one column `id` each).
#[derive(Debug, Clone)]
pub struct Visibility {
    pub documents: String,
    pub tags: String,
    pub correspondents: String,
    pub document_types: String,
    pub storage_paths: String,
    /// Show custom field names (`documents.view_customfield`).
    pub custom_field_names: bool,
}

impl Visibility {
    fn of(&self, kind: i16) -> &str {
        match kind {
            KIND_TAG => &self.tags,
            KIND_CORRESPONDENT => &self.correspondents,
            KIND_DOCUMENT_TYPE => &self.document_types,
            KIND_STORAGE_PATH => &self.storage_paths,
            _ => &self.documents,
        }
    }
}

// ------------------------------------------------------------------------------ Identity

/// UUIDs per `local_id`; missing rows are created (like pl-store).
pub async fn uuids(pool: &PgPool, kind: i16, ids: &[i64]) -> Result<HashMap<i64, Uuid>> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let fetch = |ids: Vec<i64>| async move {
        let rows: Vec<(i64, Uuid)> =
            sqlx::query_as("SELECT local_id::int8, uuid FROM paperrust_sync_object WHERE kind = $1 AND local_id = ANY($2::int8[])")
                .bind(kind)
                .bind(&ids)
                .fetch_all(pool)
                .await?;
        Ok::<_, sqlx::Error>(rows.into_iter().collect::<HashMap<i64, Uuid>>())
    };
    let mut out = fetch(ids.clone()).await?;
    let missing: Vec<i64> = ids.iter().copied().filter(|i| !out.contains_key(i)).collect();
    if !missing.is_empty() {
        sqlx::query("SELECT paperrust_sync_ensure($1, $2::int8[])").bind(kind).bind(&missing).execute(pool).await?;
        out.extend(fetch(missing).await?);
    }
    Ok(out)
}

/// Live UUIDs → `local_id`.
pub async fn resolve(pool: &PgPool, kind: i16, uuids: &[Uuid]) -> Result<HashMap<Uuid, i64>> {
    if uuids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT uuid, local_id::int8 FROM paperrust_sync_object WHERE kind = $1 AND uuid = ANY($2::uuid[]) AND deleted_at IS NULL",
    )
    .bind(kind)
    .bind(uuids)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Visible document for a UUID (otherwise `NotFound`).
pub async fn visible_document(pool: &PgPool, vis: &Visibility, uuid: Uuid) -> Result<i64> {
    let id = resolve(pool, KIND_DOCUMENT, &[uuid]).await?.remove(&uuid).ok_or(TimelineError::NotFound)?;
    let ok: Option<i64> = sqlx::query_scalar(&format!("SELECT v.id::int8 FROM ({}) AS v(id) WHERE v.id = $1", vis.documents))
        .bind(id as i32)
        .fetch_optional(pool)
        .await?;
    ok.ok_or(TimelineError::NotFound)
}

/// Visible subset of `ids`.
pub async fn visible_documents(pool: &PgPool, vis: &Visibility, ids: &[i64]) -> Result<HashSet<i64>> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    let ids32: Vec<i32> = ids.iter().map(|i| *i as i32).collect();
    let rows: Vec<i64> = sqlx::query_scalar(&format!("SELECT v.id::int8 FROM ({}) AS v(id) WHERE v.id = ANY($1)", vis.documents))
        .bind(&ids32)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().collect())
}

// ------------------------------------------------------------------------------ Life areas

fn area_from(r: &sqlx::postgres::PgRow) -> Result<Area> {
    let rules: serde_json::Value = r.try_get("rules")?;
    let user_id: Option<i32> = r.try_get("user_id")?;
    Ok(Area {
        id: r.try_get("id")?,
        uuid: r.try_get("uuid")?,
        scope: if user_id.is_some() { Scope::User } else { Scope::Instance },
        user_id: user_id.map(i64::from),
        name: r.try_get("name")?,
        color: r.try_get("color")?,
        icon: r.try_get("icon")?,
        order: r.try_get("sort")?,
        rules: serde_json::from_value::<Rules>(rules).unwrap_or_default(),
        created: r.try_get("created")?,
        modified: r.try_get("modified")?,
    })
}

const AREA_COLS: &str = "id, uuid, user_id, name, color, icon, sort, rules, created, modified";

/// Areas of a scope (`user_id` only for `Scope::User`).
pub async fn list_areas(pool: &PgPool, scope: Scope, user_id: i64) -> Result<Vec<Area>> {
    let rows = match scope {
        Scope::User => {
            sqlx::query(&format!("SELECT {AREA_COLS} FROM paperrust_life_area WHERE user_id = $1 ORDER BY sort, name, id"))
                .bind(user_id as i32)
                .fetch_all(pool)
                .await?
        }
        Scope::Instance => {
            sqlx::query(&format!("SELECT {AREA_COLS} FROM paperrust_life_area WHERE user_id IS NULL ORDER BY sort, name, id"))
                .fetch_all(pool)
                .await?
        }
    };
    rows.iter().map(area_from).collect()
}

/// Effective areas: own ones, otherwise instance default. Also returns the source.
pub async fn effective_areas(pool: &PgPool, user_id: i64) -> Result<(Vec<Area>, &'static str)> {
    let own = list_areas(pool, Scope::User, user_id).await?;
    if !own.is_empty() {
        return Ok((own, "user"));
    }
    let inst = list_areas(pool, Scope::Instance, user_id).await?;
    let src = if inst.is_empty() { "none" } else { "instance" };
    Ok((inst, src))
}

pub async fn get_area(pool: &PgPool, uuid: Uuid) -> Result<Option<Area>> {
    let r = sqlx::query(&format!("SELECT {AREA_COLS} FROM paperrust_life_area WHERE uuid = $1")).bind(uuid).fetch_optional(pool).await?;
    r.as_ref().map(area_from).transpose()
}

fn owner(scope: Scope, user_id: i64) -> Option<i32> {
    match scope {
        Scope::User => Some(user_id as i32),
        Scope::Instance => None,
    }
}

async fn count_scope(conn: &mut sqlx::PgConnection, scope: Scope, user_id: i64) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM paperrust_life_area WHERE user_id IS NOT DISTINCT FROM $1")
        .bind(owner(scope, user_id))
        .fetch_one(&mut *conn)
        .await?)
}

pub async fn insert_area(pool: &PgPool, scope: Scope, user_id: i64, v: &AreaValues) -> Result<Area> {
    let mut tx = pool.begin().await?;
    if count_scope(&mut tx, scope, user_id).await? as usize >= MAX_AREAS {
        return Err(invalid("items", format!("At most {MAX_AREAS} life areas.")));
    }
    let r = sqlx::query(&format!(
        "INSERT INTO paperrust_life_area (user_id, name, color, icon, sort, rules) VALUES ($1, $2, $3, $4, $5, $6) RETURNING {AREA_COLS}"
    ))
    .bind(owner(scope, user_id))
    .bind(&v.name)
    .bind(&v.color)
    .bind(&v.icon)
    .bind(v.order)
    .bind(serde_json::to_value(&v.rules).unwrap_or_default())
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    area_from(&r)
}

pub async fn update_area(pool: &PgPool, id: i64, v: &AreaValues) -> Result<Area> {
    let r = sqlx::query(&format!(
        "UPDATE paperrust_life_area SET name = $2, color = $3, icon = $4, sort = $5, rules = $6, modified = now() WHERE id = $1 RETURNING {AREA_COLS}"
    ))
    .bind(id)
    .bind(&v.name)
    .bind(&v.color)
    .bind(&v.icon)
    .bind(v.order)
    .bind(serde_json::to_value(&v.rules).unwrap_or_default())
    .fetch_optional(pool)
    .await?
    .ok_or(TimelineError::NotFound)?;
    area_from(&r)
}

pub async fn delete_area(pool: &PgPool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM paperrust_life_area WHERE id = $1").bind(id).execute(pool).await?;
    Ok(())
}

pub async fn delete_scope(pool: &PgPool, scope: Scope, user_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM paperrust_life_area WHERE user_id IS NOT DISTINCT FROM $1").bind(owner(scope, user_id)).execute(pool).await?;
    Ok(())
}

/// Replace the whole set; entries with `uuid` from this set keep their identity.
pub async fn replace_areas(pool: &PgPool, scope: Scope, user_id: i64, items: &[(Option<Uuid>, AreaValues)]) -> Result<Vec<Area>> {
    if items.len() > MAX_AREAS {
        return Err(invalid("items", format!("At most {MAX_AREAS} life areas.")));
    }
    let mut tx = pool.begin().await?;
    let o = owner(scope, user_id);
    let existing: Vec<(i64, Uuid)> =
        sqlx::query_as("SELECT id, uuid FROM paperrust_life_area WHERE user_id IS NOT DISTINCT FROM $1 FOR UPDATE")
            .bind(o)
            .fetch_all(&mut *tx)
            .await?;
    let by_uuid: HashMap<Uuid, i64> = existing.iter().map(|(i, u)| (*u, *i)).collect();
    let mut keep: HashSet<i64> = HashSet::new();
    for (u, v) in items {
        let rules = serde_json::to_value(&v.rules).unwrap_or_default();
        match u.and_then(|u| by_uuid.get(&u).copied()).filter(|id| !keep.contains(id)) {
            Some(id) => {
                keep.insert(id);
                sqlx::query("UPDATE paperrust_life_area SET name = $2, color = $3, icon = $4, sort = $5, rules = $6, modified = now() WHERE id = $1")
                    .bind(id)
                    .bind(&v.name)
                    .bind(&v.color)
                    .bind(&v.icon)
                    .bind(v.order)
                    .bind(rules)
                    .execute(&mut *tx)
                    .await?;
            }
            None => {
                let id: i64 = sqlx::query_scalar(
                    "INSERT INTO paperrust_life_area (user_id, name, color, icon, sort, rules) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
                )
                .bind(o)
                .bind(&v.name)
                .bind(&v.color)
                .bind(&v.icon)
                .bind(v.order)
                .bind(rules)
                .fetch_one(&mut *tx)
                .await?;
                keep.insert(id);
            }
        }
    }
    let drop: Vec<i64> = existing.iter().map(|(i, _)| *i).filter(|i| !keep.contains(i)).collect();
    sqlx::query("DELETE FROM paperrust_life_area WHERE id = ANY($1)").bind(&drop).execute(&mut *tx).await?;
    tx.commit().await?;
    list_areas(pool, scope, user_id).await
}

/// Rules → legacy ids (unknown/deleted objects are dropped).
pub async fn resolve_rules(pool: &PgPool, rules: &Rules) -> Result<ResolvedRules> {
    let mut r = ResolvedRules::default();
    for (name, kind, list) in rules.lists() {
        let ids: HashSet<i64> = resolve(pool, kind, list).await?.into_values().collect();
        match name {
            "tags" => r.tags = ids,
            "correspondents" => r.correspondents = ids,
            "document_types" => r.document_types = ids,
            _ => r.storage_paths = ids,
        }
    }
    Ok(r)
}

/// All rule objects must be visible to the writer (otherwise field error).
pub async fn check_rules_visible(pool: &PgPool, rules: &Rules, vis: &Visibility) -> Result<()> {
    for (name, kind, list) in rules.lists() {
        if list.is_empty() {
            continue;
        }
        let ok: Vec<Uuid> = sqlx::query_scalar(&format!(
            "SELECT o.uuid FROM paperrust_sync_object o WHERE o.kind = $1 AND o.uuid = ANY($2::uuid[]) AND o.deleted_at IS NULL
               AND o.local_id IN (SELECT v.id::int8 FROM ({}) AS v(id))",
            vis.of(kind)
        ))
        .bind(kind)
        .bind(list)
        .fetch_all(pool)
        .await?;
        if let Some(bad) = list.iter().find(|u| !ok.contains(u)) {
            return Err(invalid(&format!("rules.{name}"), format!("Unknown object \"{bad}\".")));
        }
    }
    Ok(())
}

/// Assignment attributes of all visible documents (for suggestions).
pub async fn doc_keys(pool: &PgPool, vis: &Visibility) -> Result<Vec<DocKeys>> {
    let rows = sqlx::query(&format!(
        "SELECT d.id::int8 AS id, d.document_type_id, d.storage_path_id, d.correspondent_id,
                COALESCE((SELECT array_agg(dt.tag_id::int8) FROM documents_document_tags dt WHERE dt.document_id = d.id), '{{}}') AS tags
           FROM documents_document d WHERE d.id IN (SELECT v.id FROM ({}) AS v(id))",
        vis.documents
    ))
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|r| {
            Ok(DocKeys {
                id: r.try_get("id")?,
                tags: r.try_get("tags")?,
                document_type: r.try_get::<Option<i32>, _>("document_type_id")?.map(i64::from),
                storage_path: r.try_get::<Option<i32>, _>("storage_path_id")?.map(i64::from),
                correspondent: r.try_get::<Option<i32>, _>("correspondent_id")?.map(i64::from),
            })
        })
        .collect()
}

/// Visible tags, document types, storage paths as suggestion candidates.
pub async fn candidates(pool: &PgPool, vis: &Visibility) -> Result<Vec<Candidate>> {
    let mut out = Vec::new();
    for (list, kind, table, inbox) in [
        ("tags", KIND_TAG, "documents_tag", "is_inbox_tag"),
        ("document_types", KIND_DOCUMENT_TYPE, "documents_documenttype", "false"),
        ("storage_paths", KIND_STORAGE_PATH, "documents_storagepath", "false"),
    ] {
        let rows: Vec<(i64, String, bool)> = sqlx::query_as(&format!(
            "SELECT o.id::int8, o.name::text, {inbox} FROM {table} o WHERE o.id IN (SELECT v.id FROM ({}) AS v(id)) ORDER BY o.name, o.id",
            vis.of(kind)
        ))
        .fetch_all(pool)
        .await?;
        let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
        let u = uuids(pool, kind, &ids).await?;
        for (id, name, is_inbox) in rows {
            if let Some(uuid) = u.get(&id) {
                out.push(Candidate { list, id, uuid: *uuid, name, is_inbox });
            }
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------------------ Timeline

/// All custom fields.
pub async fn custom_fields(pool: &PgPool) -> Result<Vec<FieldInfo>> {
    let rows: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT id::int8, name::text, data_type::text FROM documents_customfield ORDER BY id").fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id, name, data_type)| FieldInfo { id, name, data_type }).collect())
}

/// What is to be loaded.
#[derive(Debug, Clone)]
pub struct LoadSpec<'a> {
    pub user_id: i64,
    /// Document date in the range [from, to] (for `created` entries); `None` = none.
    pub created_range: Option<(NaiveDate, NaiveDate)>,
    pub fields: &'a FieldKinds,
    pub field_names: &'a HashMap<i64, String>,
    pub content_dates: bool,
}

/// Load documents and date rows for the user (visible documents only).
pub async fn load(pool: &PgPool, vis: &Visibility, spec: &LoadSpec<'_>) -> Result<(HashMap<i64, DocRow>, Vec<DateRow>)> {
    let in_vis = format!("IN (SELECT v.id FROM ({}) AS v(id))", vis.documents);
    let mut rows: Vec<DateRow> = Vec::new();
    // 1. Document date
    if let Some((from, to)) = spec.created_range {
        let r: Vec<(i64, NaiveDate)> = sqlx::query_as(&format!(
            "SELECT d.id::int8, d.created FROM documents_document d WHERE d.id {in_vis} AND d.created BETWEEN $1 AND $2"
        ))
        .bind(from)
        .bind(to)
        .fetch_all(pool)
        .await?;
        rows.extend(r.into_iter().map(|(id, date)| DateRow {
            doc_id: id,
            kind: DateKind::Created,
            date,
            source: Source::Created,
            confidence: 1.0,
            field: None,
            phrase: None,
            page: None,
            reminder: None,
        }));
    }
    // 2. Custom fields of type date
    let field_ids: Vec<i32> = spec.fields.dates.keys().map(|i| *i as i32).collect();
    let field_uuids =
        uuids(pool, KIND_CUSTOM_FIELD, &spec.fields.dates.keys().chain(spec.fields.amount.iter()).copied().collect::<Vec<_>>()).await?;
    if !field_ids.is_empty() {
        let r: Vec<(i64, i64, NaiveDate)> = sqlx::query_as(&format!(
            "SELECT i.document_id::int8, i.field_id::int8, i.value_date FROM documents_customfieldinstance i
              WHERE i.deleted_at IS NULL AND i.value_date IS NOT NULL AND i.field_id = ANY($1) AND i.document_id {in_vis}"
        ))
        .bind(&field_ids)
        .fetch_all(pool)
        .await?;
        for (doc, field, date) in r {
            let Some((kind, _)) = spec.fields.dates.get(&field) else {
                continue;
            };
            rows.push(DateRow {
                doc_id: doc,
                kind: *kind,
                date,
                source: Source::CustomField,
                confidence: 1.0,
                field: field_uuids
                    .get(&field)
                    .map(|u| RefOut { uuid: *u, name: if vis.custom_field_names { spec.field_names.get(&field).cloned() } else { None } }),
                phrase: None,
                page: None,
                reminder: None,
            });
        }
    }
    // 3. stored data (content, own reminders)
    let r = sqlx::query(&format!(
        "SELECT id, document_id::int8 AS document_id, kind, date, source, confidence, phrase, page, note FROM paperrust_doc_date
          WHERE document_id {in_vis} AND ((source = 'user' AND created_by = $1) OR (source = 'content' AND $2))"
    ))
    .bind(spec.user_id as i32)
    .bind(spec.content_dates)
    .fetch_all(pool)
    .await?;
    for x in r {
        let (Some(kind), Some(source)) =
            (DateKind::parse(&x.try_get::<String, _>("kind")?), Source::parse(&x.try_get::<String, _>("source")?))
        else {
            continue;
        };
        rows.push(DateRow {
            doc_id: x.try_get("document_id")?,
            kind,
            date: x.try_get("date")?,
            source,
            confidence: if source == Source::User { 1.0 } else { x.try_get("confidence")? },
            field: None,
            phrase: x.try_get("phrase")?,
            page: x.try_get("page")?,
            reminder: (source == Source::User)
                .then(|| -> Result<ReminderRef> { Ok(ReminderRef { id: x.try_get("id")?, note: x.try_get("note")? }) })
                .transpose()?,
        });
    }
    // 4. Documents
    let mut ids: Vec<i64> = rows.iter().map(|r| r.doc_id).collect();
    ids.sort_unstable();
    ids.dedup();
    let docs = docs(pool, vis, &ids, spec.fields).await?;
    rows.retain(|r| docs.contains_key(&r.doc_id));
    Ok((docs, rows))
}

/// Document rows (without renewed visibility check: `ids` come from visible sets).
async fn docs(pool: &PgPool, vis: &Visibility, ids: &[i64], fields: &FieldKinds) -> Result<HashMap<i64, DocRow>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids32: Vec<i32> = ids.iter().map(|i| *i as i32).collect();
    let rows = sqlx::query(
        "SELECT d.id::int8 AS id, d.title, d.created, d.correspondent_id, d.document_type_id, d.storage_path_id,
                COALESCE((SELECT array_agg(dt.tag_id::int8) FROM documents_document_tags dt WHERE dt.document_id = d.id), '{}') AS tags
           FROM documents_document d WHERE d.id = ANY($1)",
    )
    .bind(&ids32)
    .fetch_all(pool)
    .await?;
    let doc_uuids = uuids(pool, KIND_DOCUMENT, ids).await?;
    let mut corr: Vec<i64> = Vec::new();
    let mut types: Vec<i64> = Vec::new();
    for r in &rows {
        if let Some(c) = r.try_get::<Option<i32>, _>("correspondent_id")? {
            corr.push(c.into());
        }
        if let Some(t) = r.try_get::<Option<i32>, _>("document_type_id")? {
            types.push(t.into());
        }
    }
    let corr_ref = refs(pool, vis, KIND_CORRESPONDENT, "documents_correspondent", &corr).await?;
    let type_ref = refs(pool, vis, KIND_DOCUMENT_TYPE, "documents_documenttype", &types).await?;
    // Amount
    let mut amounts: HashMap<i64, Amount> = HashMap::new();
    if !fields.amount.is_empty() {
        let fids: Vec<i32> = fields.amount.iter().map(|i| *i as i32).collect();
        let fu = uuids(pool, KIND_CUSTOM_FIELD, &fields.amount).await?;
        let r: Vec<(i64, i64, String)> = sqlx::query_as(
            "SELECT DISTINCT ON (i.document_id) i.document_id::int8, i.field_id::int8, i.value_monetary::text
               FROM documents_customfieldinstance i
              WHERE i.deleted_at IS NULL AND i.document_id = ANY($1) AND i.field_id = ANY($2)
                AND i.value_monetary IS NOT NULL AND i.value_monetary <> ''
              ORDER BY i.document_id, array_position($2, i.field_id)",
        )
        .bind(&ids32)
        .bind(&fids)
        .fetch_all(pool)
        .await?;
        for (doc, field, v) in r {
            if let (Some((value, currency)), Some(u)) = (parse_monetary(&v), fu.get(&field)) {
                amounts.insert(doc, Amount { value, currency, field: *u });
            }
        }
    }
    let mut out = HashMap::new();
    for r in rows {
        let id: i64 = r.try_get("id")?;
        let Some(uuid) = doc_uuids.get(&id) else {
            continue;
        };
        let c = r.try_get::<Option<i32>, _>("correspondent_id")?.map(i64::from);
        let t = r.try_get::<Option<i32>, _>("document_type_id")?.map(i64::from);
        out.insert(
            id,
            DocRow {
                uuid: *uuid,
                title: r.try_get("title")?,
                created: r.try_get("created")?,
                keys: DocKeys {
                    id,
                    tags: r.try_get("tags")?,
                    document_type: t,
                    storage_path: r.try_get::<Option<i32>, _>("storage_path_id")?.map(i64::from),
                    correspondent: c,
                },
                correspondent: c.and_then(|c| corr_ref.get(&c).cloned()),
                document_type: t.and_then(|t| type_ref.get(&t).cloned()),
                amount: amounts.remove(&id),
            },
        );
    }
    Ok(out)
}

/// References with names (visible names only).
async fn refs(pool: &PgPool, vis: &Visibility, kind: i16, table: &str, ids: &[i64]) -> Result<HashMap<i64, RefOut>> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let u = uuids(pool, kind, &ids).await?;
    let ids32: Vec<i32> = ids.iter().map(|i| *i as i32).collect();
    let names: HashMap<i64, String> = sqlx::query_as::<_, (i64, String)>(&format!(
        "SELECT o.id::int8, o.name::text FROM {table} o WHERE o.id = ANY($1) AND o.id IN (SELECT v.id FROM ({}) AS v(id))",
        vis.of(kind)
    ))
    .bind(&ids32)
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    Ok(ids.iter().filter_map(|i| Some((*i, RefOut { uuid: *u.get(i)?, name: names.get(i).cloned() }))).collect())
}

// ------------------------------------------------------------------------------ State

/// States of the user for `docs`.
pub async fn states(pool: &PgPool, user_id: i64, docs: &[i64]) -> Result<HashMap<String, ItemState>> {
    if docs.is_empty() {
        return Ok(HashMap::new());
    }
    let ids32: Vec<i32> = docs.iter().map(|i| *i as i32).collect();
    let r: Vec<(String, Option<DateTime<Utc>>, Option<NaiveDate>)> = sqlx::query_as(
        "SELECT item::text, done_at, snoozed_until FROM paperrust_timeline_state WHERE user_id = $1 AND document_id = ANY($2)",
    )
    .bind(user_id as i32)
    .bind(&ids32)
    .fetch_all(pool)
    .await?;
    Ok(r.into_iter().map(|(i, d, s)| (i, ItemState { done_at: d, snoozed_until: s })).collect())
}

/// Set the state of an entry. `done`: `Some` sets/clears; `snoozed`: `Some` sets/clears.
pub async fn set_state(
    pool: &PgPool,
    user_id: i64,
    item: &str,
    doc_id: i64,
    done: Option<bool>,
    snoozed: Option<Option<NaiveDate>>,
) -> Result<ItemState> {
    let r: (Option<DateTime<Utc>>, Option<NaiveDate>) = sqlx::query_as(
        "INSERT INTO paperrust_timeline_state (user_id, item, document_id, done_at, snoozed_until)
              VALUES ($1, $2, $3, CASE WHEN $4 THEN now() END, $6)
         ON CONFLICT (user_id, item) DO UPDATE SET
              done_at = CASE WHEN $4 IS NULL THEN paperrust_timeline_state.done_at
                             WHEN $4 THEN COALESCE(paperrust_timeline_state.done_at, now()) END,
              snoozed_until = CASE WHEN $5 THEN $6 ELSE paperrust_timeline_state.snoozed_until END,
              updated = now()
         RETURNING done_at, snoozed_until",
    )
    .bind(user_id as i32)
    .bind(item)
    .bind(doc_id as i32)
    .bind(done)
    .bind(snoozed.is_some())
    .bind(snoozed.flatten())
    .fetch_one(pool)
    .await?;
    Ok(ItemState { done_at: r.0, snoozed_until: r.1 })
}

pub async fn clear_state(pool: &PgPool, user_id: i64, item: &str) -> Result<()> {
    sqlx::query("DELETE FROM paperrust_timeline_state WHERE user_id = $1 AND item = $2")
        .bind(user_id as i32)
        .bind(item)
        .execute(pool)
        .await?;
    Ok(())
}

// ------------------------------------------------------------------------------ Reminders

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reminder {
    pub id: i64,
    #[serde(skip)]
    pub document_id: i64,
    pub kind: DateKind,
    pub date: NaiveDate,
    pub note: Option<String>,
    pub created: DateTime<Utc>,
}

fn reminder_from(r: &sqlx::postgres::PgRow) -> Result<Reminder> {
    let kind: String = r.try_get("kind")?;
    Ok(Reminder {
        id: r.try_get("id")?,
        document_id: r.try_get("document_id")?,
        kind: DateKind::parse(&kind).unwrap_or(DateKind::Due),
        date: r.try_get("date")?,
        note: r.try_get("note")?,
        created: r.try_get("created")?,
    })
}

/// Create reminder; `(reminder, newly created)`.
pub async fn add_reminder(
    pool: &PgPool,
    user_id: i64,
    doc_id: i64,
    kind: DateKind,
    date: NaiveDate,
    note: Option<&str>,
) -> Result<(Reminder, bool)> {
    let ins = sqlx::query(
        "INSERT INTO paperrust_doc_date (document_id, kind, date, source, confidence, note, created_by)
         VALUES ($1, $2, $3, 'user', 1, $4, $5) ON CONFLICT DO NOTHING
         RETURNING id, document_id::int8 AS document_id, kind, date, note, created",
    )
    .bind(doc_id as i32)
    .bind(kind.as_str())
    .bind(date)
    .bind(note)
    .bind(user_id as i32)
    .fetch_optional(pool)
    .await?;
    if let Some(r) = ins {
        return Ok((reminder_from(&r)?, true));
    }
    let r = sqlx::query(
        "SELECT id, document_id::int8 AS document_id, kind, date, note, created FROM paperrust_doc_date
          WHERE document_id = $1 AND kind = $2 AND date = $3 AND source = 'user' AND created_by = $4",
    )
    .bind(doc_id as i32)
    .bind(kind.as_str())
    .bind(date)
    .bind(user_id as i32)
    .fetch_one(pool)
    .await?;
    Ok((reminder_from(&r)?, false))
}

/// Eigene Erinnerungen an sichtbaren Dokumenten.
pub async fn list_reminders(pool: &PgPool, user_id: i64, vis: &Visibility) -> Result<Vec<Reminder>> {
    let r = sqlx::query(&format!(
        "SELECT id, document_id::int8 AS document_id, kind, date, note, created FROM paperrust_doc_date
          WHERE source = 'user' AND created_by = $1 AND document_id IN (SELECT v.id FROM ({}) AS v(id)) ORDER BY date, id",
        vis.documents
    ))
    .bind(user_id as i32)
    .fetch_all(pool)
    .await?;
    r.iter().map(reminder_from).collect()
}

/// Delete own reminder (`false` = not found).
pub async fn delete_reminder(pool: &PgPool, user_id: i64, id: i64) -> Result<bool> {
    let n = sqlx::query("DELETE FROM paperrust_doc_date WHERE id = $1 AND source = 'user' AND created_by = $2")
        .bind(id)
        .bind(user_id as i32)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n > 0)
}

/// (Document, date) of all dates that can already be on the timeline for the user
/// (own reminders, content detection, custom fields of type date).
/// Content dates count only with `content` (detection switched on), like on the timeline.
pub async fn existing_dates(pool: &PgPool, user_id: i64, docs: &[i64], content: bool) -> Result<HashSet<(i64, NaiveDate)>> {
    if docs.is_empty() {
        return Ok(HashSet::new());
    }
    let ids32: Vec<i32> = docs.iter().map(|i| *i as i32).collect();
    let r: Vec<(i64, NaiveDate)> = sqlx::query_as(
        "SELECT document_id::int8, date FROM paperrust_doc_date
          WHERE document_id = ANY($1) AND ((source = 'content' AND $3) OR (source = 'user' AND created_by = $2))
         UNION
         SELECT document_id::int8, value_date FROM documents_customfieldinstance
          WHERE document_id = ANY($1) AND deleted_at IS NULL AND value_date IS NOT NULL",
    )
    .bind(&ids32)
    .bind(user_id as i32)
    .bind(content)
    .fetch_all(pool)
    .await?;
    Ok(r.into_iter().collect())
}

/// Content and document date (for on-demand content detection).
pub async fn contents(pool: &PgPool, ids: &[i64]) -> Result<HashMap<i64, (String, NaiveDate)>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids32: Vec<i32> = ids.iter().map(|i| *i as i32).collect();
    let r: Vec<(i64, String, NaiveDate)> =
        sqlx::query_as("SELECT id::int8, content, created FROM documents_document WHERE id = ANY($1)").bind(&ids32).fetch_all(pool).await?;
    Ok(r.into_iter().map(|(i, c, d)| (i, (c, d))).collect())
}

/// Title/UUID of visible documents (for lists).
pub async fn titles(pool: &PgPool, ids: &[i64]) -> Result<HashMap<i64, (Uuid, String)>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids32: Vec<i32> = ids.iter().map(|i| *i as i32).collect();
    let r: Vec<(i64, String)> =
        sqlx::query_as("SELECT id::int8, title::text FROM documents_document WHERE id = ANY($1)").bind(&ids32).fetch_all(pool).await?;
    let u = uuids(pool, KIND_DOCUMENT, ids).await?;
    Ok(r.into_iter().filter_map(|(i, t)| Some((i, (*u.get(&i)?, t)))).collect())
}

// ------------------------------------------------------------------------------ Detection state

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ExtractionStats {
    /// Root documents outside the trash.
    pub documents: i64,
    /// with current detection.
    pub extracted: i64,
    /// without or with outdated detection.
    pub stale: i64,
}

pub async fn extraction_stats(pool: &PgPool) -> Result<ExtractionStats> {
    let (documents, extracted): (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(s.document_id) FILTER (WHERE s.modified >= d.modified AND s.version >= $1)
           FROM documents_document d LEFT JOIN paperrust_doc_date_state s ON s.document_id = d.id
          WHERE d.deleted_at IS NULL AND d.root_document_id IS NULL",
    )
    .bind(crate::extract::EXTRACTOR_VERSION)
    .fetch_one(pool)
    .await?;
    Ok(ExtractionStats { documents, extracted, stale: documents - extracted })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_statements() {
        let s = statements(MIGRATION.1);
        assert_eq!(s.len(), 11, "{s:#?}");
        assert!(s.iter().all(|x| x.trim_end().ends_with(';') && !x.contains("--")));
        assert!(s.iter().all(|x| x.contains("IF NOT EXISTS")), "idempotent");
        assert!(s.iter().all(|x| !x.contains("documents_")), "keine Django-Tabellen anfassen (expand-only)");
    }
}
