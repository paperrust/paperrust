//! Non-document kinds: tags, correspondents, document types, storage paths, custom fields,
//! saved views, users, groups; also `me` and `ui_settings`.

use std::collections::{HashMap, HashSet};

use pl_authz::User;
use pl_authz::objects::{FilterOpts, permitted_objects_filter};
use pl_domain::cursor::KeysetCursor;
use pl_domain::model::*;
use pl_domain::{DomainError, DomainResult, Kind, Principal};
use sea_query::{Expr, Order, PostgresQueryBuilder, Query, SelectStatement};
use pl_core::sqlbind::SqlxBinder as _;
use serde_json::Value;
use sqlx::Row as _;
use sqlx::postgres::PgRow;
use uuid::Uuid;

use crate::perm::{self, Spec};
use crate::{Store, a, auth, db_err, ident};

pub(crate) const MAX_LIMIT: i64 = 1000;
pub(crate) const DEFAULT_LIMIT: i64 = 100;

fn columns(kind: Kind) -> &'static str {
    match kind {
        Kind::Tag => {
            "o.id::int8 AS id, o.name, o.color, o.is_inbox_tag, o.match, o.matching_algorithm::int8 AS algo, \
             o.is_insensitive, o.tn_parent_id::int8 AS parent_id, o.owner_id::int8 AS owner_id"
        }
        Kind::Correspondent | Kind::DocumentType => {
            "o.id::int8 AS id, o.name, o.match, o.matching_algorithm::int8 AS algo, o.is_insensitive, o.owner_id::int8 AS owner_id"
        }
        Kind::StoragePath => {
            "o.id::int8 AS id, o.name, o.path, o.match, o.matching_algorithm::int8 AS algo, o.is_insensitive, \
             o.owner_id::int8 AS owner_id"
        }
        Kind::CustomField => "o.id::int8 AS id, o.name, o.data_type, o.extra_data, o.created",
        Kind::SavedView => {
            "o.id::int8 AS id, o.name, o.sort_field, o.sort_reverse, o.page_size::int8 AS page_size, o.display_mode, \
             o.display_fields, o.icon, o.owner_id::int8 AS owner_id"
        }
        Kind::User => "o.id::int8 AS id, o.username, o.first_name, o.last_name",
        Kind::Group => "o.id::int8 AS id, o.name",
        Kind::Document => "o.id::int8 AS id",
    }
}

/// Base query of a kind with visibility (like `PermittedObjectsFilter` in v1).
fn base(spec: &Spec, user: &User) -> SelectStatement {
    let mut q = Query::select();
    q.expr(Expr::cust(columns(spec.kind))).from_as(a(spec.table), a("o"));
    if let Some(m) = &spec.model {
        q.cond_where(permitted_objects_filter(Some(user), m, "o", &FilterOpts::default()));
    }
    q
}

async fn fetch(store: &Store, q: &SelectStatement) -> DomainResult<Vec<PgRow>> {
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).fetch_all(&store.db).await.map_err(db_err)
}

fn parse_sort(s: Option<&str>) -> DomainResult<(&'static str, bool)> {
    match s.unwrap_or("name") {
        "name" => Ok(("name", false)),
        "-name" => Ok(("name", true)),
        "id" => Ok(("id", false)),
        "-id" => Ok(("id", true)),
        other => Err(DomainError::field("sort", format!("Unknown sort \"{other}\" (name, -name, id, -id)."))),
    }
}

pub(crate) async fn list(store: &Store, p: &Principal, kind: Kind, req: &ListRequest) -> DomainResult<Page<Object>> {
    if kind == Kind::Document {
        return Err(DomainError::Internal("documents are listed via query".into()));
    }
    let user = auth::user(p)?;
    let spec = perm::spec(kind);
    perm::require(user, spec.view_perm)?;
    let limit = if req.limit <= 0 { DEFAULT_LIMIT } else { req.limit.min(MAX_LIMIT) };
    let (sort, desc) = parse_sort(req.sort.as_deref())?;
    let sort_key = if desc { format!("-{sort}") } else { sort.to_string() };
    let col = if sort == "name" { format!("o.{}", spec.name_col) } else { "o.id".to_string() };

    let mut q = base(&spec, user);
    if let Some(n) = req.name_contains.as_deref().filter(|s| !s.is_empty()) {
        q.and_where(Expr::cust_with_values(
            format!("UPPER(o.{}) LIKE UPPER($1)", spec.name_col),
            [sea_query::Value::from(like_pattern(n))],
        ));
    }
    let count = if req.count {
        let mut cq = q.clone();
        cq.clear_selects();
        cq.expr(Expr::cust("1"));
        let mut outer = Query::select();
        outer.expr(Expr::cust("count(*)::int8")).from_subquery(cq, a("sub"));
        let (sql, values) = outer.build_sqlx(PostgresQueryBuilder);
        Some(sqlx::query_scalar_with::<_, i64, _>(&sql, values).fetch_one(&store.db).await.map_err(db_err)?)
    } else {
        None
    };
    if let Some(c) = req.cursor.as_deref() {
        let c = KeysetCursor::decode_for(c, &sort_key)?;
        let op = if desc { "<" } else { ">" };
        if sort == "name" {
            let v = c.v.as_str().unwrap_or_default().to_string();
            q.and_where(Expr::cust_with_values(
                format!("({col}, o.id) {op} ($1, $2)"),
                [sea_query::Value::from(v), sea_query::Value::from(c.id)],
            ));
        } else {
            q.and_where(Expr::cust_with_values(format!("o.id {op} $1"), [c.id]));
        }
    }
    let ord = if desc { Order::Desc } else { Order::Asc };
    q.order_by_expr(Expr::cust(col.clone()), ord.clone());
    if sort == "name" {
        q.order_by_expr(Expr::cust("o.id"), ord);
    }
    q.limit((limit + 1) as u64);
    let mut rows = fetch(store, &q).await?;
    let more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let next_cursor = if more {
        rows.last().map(|r| {
            let id: i64 = r.try_get("id").unwrap_or_default();
            let v = if sort == "name" { Value::from(r.try_get::<String, _>(spec.name_col).unwrap_or_default()) } else { Value::Null };
            KeysetCursor { s: sort_key.clone(), v, id }.encode()
        })
    } else {
        None
    };
    let items = build(store, user, &spec, &rows, req.document_count, req.sharing).await?;
    Ok(Page { items, next_cursor, count })
}

/// `%…%` with escaped `%`/`_`/`\`.
pub(crate) fn like_pattern(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('%');
    for c in s.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

pub(crate) async fn get(store: &Store, p: &Principal, kind: Kind, uuid: Uuid, sharing: bool) -> DomainResult<Object> {
    let user = auth::user(p)?;
    let spec = perm::spec(kind);
    perm::require(user, spec.view_perm)?;
    let (id, _) = ident::resolve_one(&store.db, kind, uuid).await?;
    let mut q = base(&spec, user);
    q.and_where(Expr::cust_with_values("o.id = $1", [id]));
    let rows = fetch(store, &q).await?;
    let mut v = build(store, user, &spec, &rows, kind != Kind::CustomField && kind != Kind::SavedView, sharing).await?;
    v.pop().ok_or(DomainError::NotFound)
}

/// Visible objects by ID (order like `ids`).
async fn visible_by_ids(store: &Store, user: &User, spec: &Spec, ids: &[i64]) -> DomainResult<Vec<Object>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut q = base(spec, user);
    q.and_where(Expr::col((a("o"), a("id"))).is_in(ids.iter().copied()));
    let rows = fetch(store, &q).await?;
    build(store, user, spec, &rows, false, false).await
}

pub(crate) async fn batch(store: &Store, p: &Principal, kind: Kind, uuids: &[Uuid]) -> DomainResult<(Vec<Object>, Vec<Uuid>)> {
    let user = auth::user(p)?;
    let spec = perm::spec(kind);
    perm::require(user, spec.view_perm)?;
    let map = ident::resolve(&store.db, kind, uuids).await?;
    let ids: Vec<i64> = map.values().map(|(id, _)| *id).collect();
    let found = visible_by_ids(store, user, &spec, &ids).await?;
    let by_uuid: HashMap<Uuid, Object> = found.into_iter().map(|o| (o.meta().uuid, o)).collect();
    let mut items = Vec::new();
    let mut missing = Vec::new();
    let mut seen = HashSet::new();
    for u in uuids {
        if !seen.insert(*u) {
            continue;
        }
        match by_uuid.get(u) {
            Some(o) => items.push(o.clone()),
            None => missing.push(*u),
        }
    }
    Ok((items, missing))
}

pub(crate) async fn by_legacy_ids(store: &Store, p: &Principal, kind: Kind, ids: &[i64]) -> DomainResult<Vec<(i64, Option<Object>)>> {
    let user = auth::user(p)?;
    let spec = perm::spec(kind);
    if !(user.is_active && user.has_perm(spec.view_perm)) {
        // without the model permission nothing of this kind is visible (like the v1 list: 403)
        return Ok(ids.iter().map(|i| (*i, None)).collect());
    }
    let found = visible_by_ids(store, user, &spec, ids).await?;
    let mut by_id: HashMap<i64, Object> = found.into_iter().map(|o| (o.meta().legacy_id, o)).collect();
    Ok(ids.iter().map(|i| (*i, by_id.remove(i))).collect())
}

/// Rows → domain objects (identity, owner, permissions, optionally document count/shares).
async fn build(
    store: &Store,
    user: &User,
    spec: &Spec,
    rows: &[PgRow],
    document_count: bool,
    want_sharing: bool,
) -> DomainResult<Vec<Object>> {
    let db = &store.db;
    let ids: Vec<i64> = rows.iter().map(|r| r.try_get::<i64, _>("id")).collect::<Result<_, _>>().map_err(db_err)?;
    let idents = ident::idents(db, spec.kind, &ids).await?;
    let has_owner = spec.model.is_some();
    let owner = |r: &PgRow| -> Option<i64> { if has_owner { r.try_get("owner_id").ok().flatten() } else { None } };
    let owners: Vec<i64> = rows.iter().filter_map(owner).collect();
    let users = perm::user_refs(db, &owners).await?;
    let grants = match &spec.model {
        Some(m) => perm::grants(db, user, m, &ids, &["change", "delete"]).await?,
        None => HashMap::new(),
    };
    let sharing = match (&spec.model, want_sharing) {
        (Some(m), true) => {
            let objs: Vec<(i64, Option<i64>)> = rows.iter().map(|r| (r.try_get("id").unwrap_or_default(), owner(r))).collect();
            perm::sharing(db, user, m, &objs).await?
        }
        _ => HashMap::new(),
    };
    let counts = if document_count && matches!(spec.kind, Kind::Tag | Kind::Correspondent | Kind::DocumentType | Kind::StoragePath) {
        document_counts(store, user, spec.kind, &ids).await?
    } else {
        HashMap::new()
    };
    let parents: Vec<i64> = if spec.kind == Kind::Tag {
        rows.iter().filter_map(|r| r.try_get::<Option<i64>, _>("parent_id").ok().flatten()).collect()
    } else {
        Vec::new()
    };
    let parent_idents = ident::idents(db, Kind::Tag, &parents).await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let id: i64 = r.try_get("id").map_err(db_err)?;
        let Some(idn) = idents.get(&id) else { continue };
        let meta = Meta { uuid: idn.uuid, legacy_id: id, rev: idn.rev };
        let owner_id = owner(r);
        let owner_ref = owner_id.and_then(|o| users.get(&o).cloned());
        let acc = match &spec.model {
            Some(m) => perm::access(user, m, id, owner_id, &grants),
            None => match spec.kind {
                Kind::CustomField => perm::model_access(user, "documents", "customfield"),
                Kind::User => perm::model_access(user, "auth", "user"),
                _ => perm::model_access(user, "auth", "group"),
            },
        };
        let dc = if document_count { Some(counts.get(&id).copied().unwrap_or(0)) } else { None };
        let sh = sharing.get(&id).cloned();
        let matching = || -> DomainResult<Matching> {
            Ok(Matching {
                algorithm: matching_name(r.try_get::<i64, _>("algo").map_err(db_err)? as i16),
                pattern: r.try_get("match").map_err(db_err)?,
                is_insensitive: r.try_get("is_insensitive").map_err(db_err)?,
            })
        };
        let s = |c: &str| -> DomainResult<String> { r.try_get(c).map_err(db_err) };
        let obj = match spec.kind {
            Kind::Tag => {
                let color: String = s("color")?;
                Object::Tag(Tag {
                    meta,
                    name: s("name")?,
                    text_color: text_color(&color).to_string(),
                    color,
                    is_inbox_tag: r.try_get("is_inbox_tag").map_err(db_err)?,
                    matching: matching()?,
                    parent: r
                        .try_get::<Option<i64>, _>("parent_id")
                        .map_err(db_err)?
                        .and_then(|pid| parent_idents.get(&pid).map(|i| i.uuid)),
                    owner: owner_ref,
                    document_count: dc,
                    access: acc,
                    sharing: sh,
                })
            }
            Kind::Correspondent | Kind::DocumentType => Object::Named(NamedObject {
                meta,
                name: s("name")?,
                matching: matching()?,
                owner: owner_ref,
                document_count: dc,
                access: acc,
                sharing: sh,
            }),
            Kind::StoragePath => Object::StoragePath(StoragePath {
                meta,
                name: s("name")?,
                path: s("path")?,
                matching: matching()?,
                owner: owner_ref,
                document_count: dc,
                access: acc,
                sharing: sh,
            }),
            Kind::CustomField => Object::CustomField(CustomField {
                meta,
                name: s("name")?,
                data_type: s("data_type")?,
                extra_data: r
                    .try_get::<Option<sqlx::types::Json<Value>>, _>("extra_data")
                    .map_err(db_err)?
                    .map(|j| j.0)
                    .unwrap_or(Value::Null),
                created: r.try_get("created").map_err(db_err)?,
            }),
            Kind::SavedView => Object::SavedView(SavedView {
                meta,
                name: s("name")?,
                sort_field: r.try_get("sort_field").map_err(db_err)?,
                sort_reverse: r.try_get("sort_reverse").map_err(db_err)?,
                page_size: r.try_get("page_size").map_err(db_err)?,
                display_mode: r.try_get("display_mode").map_err(db_err)?,
                display_fields: r
                    .try_get::<Option<sqlx::types::Json<Value>>, _>("display_fields")
                    .map_err(db_err)?
                    .map(|j| j.0),
                icon: r.try_get("icon").map_err(db_err)?,
                filter_rules: Vec::new(),
                owner: owner_ref,
                access: acc,
            }),
            Kind::User => Object::User(User_ {
                meta,
                username: s("username")?,
                first_name: s("first_name")?,
                last_name: s("last_name")?,
            }),
            Kind::Group => Object::Group(Group { meta, name: s("name")? }),
            Kind::Document => continue,
        };
        out.push(obj);
    }
    if spec.kind == Kind::SavedView {
        let rules = filter_rules(store, &ids).await?;
        for o in out.iter_mut() {
            if let Object::SavedView(v) = o {
                v.filter_rules = rules.get(&v.meta.legacy_id).cloned().unwrap_or_default();
            }
        }
    }
    Ok(out)
}

type User_ = pl_domain::model::User;

/// `TagSerializer.get_text_color` (same formula as v1, pl-objects/src/text.rs).
pub(crate) fn text_color(color: &str) -> &'static str {
    let h = color.trim_start_matches('#');
    let part = |i: usize| h.get(i..i + 2).and_then(|x| i64::from_str_radix(x, 16).ok());
    match (part(0), part(2), part(4)) {
        (Some(r), Some(g), Some(b)) => {
            let (r, g, b) = (r as f64 / 256.0, g as f64 / 256.0, b as f64 / 256.0);
            let lum = (0.299 * r * r + 0.587 * g * g + 0.114 * b * b).sqrt();
            if lum < 0.53 { "#ffffff" } else { "#000000" }
        }
        _ => "#000000",
    }
}

pub(crate) async fn filter_rules(store: &Store, ids: &[i64]) -> DomainResult<HashMap<i64, Vec<FilterRule>>> {
    let mut out: HashMap<i64, Vec<FilterRule>> = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let rows: Vec<(i32, i16, Option<String>)> = sqlx::query_as(
        "SELECT saved_view_id, rule_type, value FROM documents_savedviewfilterrule
         WHERE saved_view_id = ANY($1::int4[]) ORDER BY id",
    )
    .bind(ids.iter().map(|i| *i as i32).collect::<Vec<_>>())
    .fetch_all(&store.db)
    .await
    .map_err(db_err)?;
    for (sv, rt, value) in rows {
        out.entry(i64::from(sv)).or_default().push(FilterRule { rule_type: i64::from(rt), value });
    }
    Ok(out)
}

/// Number of documents visible to the user per object (like v1 `document_count`).
async fn document_counts(store: &Store, user: &User, kind: Kind, ids: &[i64]) -> DomainResult<HashMap<i64, i64>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let visible = pl_authz::objects::permitted_document_ids(Some(user), "view_document", false).to_string(PostgresQueryBuilder);
    let sql = match kind {
        Kind::Tag => format!(
            "SELECT t.tag_id::int8, count(*)::int8 FROM documents_document_tags t
               JOIN documents_document d ON d.id = t.document_id
              WHERE t.tag_id = ANY($1::int4[]) AND d.deleted_at IS NULL AND d.id IN ({visible}) GROUP BY 1"
        ),
        _ => {
            let col = match kind {
                Kind::Correspondent => "correspondent_id",
                Kind::DocumentType => "document_type_id",
                _ => "storage_path_id",
            };
            format!(
                "SELECT d.{col}::int8, count(*)::int8 FROM documents_document d
                  WHERE d.{col} = ANY($1::int4[]) AND d.deleted_at IS NULL AND d.id IN ({visible}) GROUP BY 1"
            )
        }
    };
    let rows: Vec<(i64, i64)> = sqlx::query_as(&sql)
        .bind(ids.iter().map(|i| *i as i32).collect::<Vec<_>>())
        .fetch_all(&store.db)
        .await
        .map_err(db_err)?;
    Ok(rows.into_iter().collect())
}

pub(crate) async fn me(store: &Store, p: &Principal) -> DomainResult<Me> {
    let ext = auth::ext(p)?;
    let user = auth::user(p)?;
    let idn = ident::idents(&store.db, Kind::User, &[user.id]).await?;
    let groups = perm::group_refs(&store.db, &user.groups).await?;
    let mut groups: Vec<GroupRef> = groups.into_values().collect();
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    let auth = match ext.ctx.method {
        Some(pl_authz::AuthMethod::Session) => "session",
        Some(pl_authz::AuthMethod::AutoLogin) => "auto_login",
        Some(pl_authz::AuthMethod::Token) => "token",
        Some(pl_authz::AuthMethod::DeviceToken) => "device_token",
        Some(pl_authz::AuthMethod::Basic) => "basic",
        Some(pl_authz::AuthMethod::RemoteUser) => "remote_user",
        Some(pl_authz::AuthMethod::DebugAngular) => "debug",
        None => "none",
    };
    Ok(Me {
        uuid: idn.get(&user.id).map(|i| i.uuid).ok_or(DomainError::NotFound)?,
        legacy_id: user.id,
        username: user.username.clone(),
        first_name: user.first_name.clone(),
        last_name: user.last_name.clone(),
        email: user.email.clone(),
        is_superuser: user.is_superuser,
        is_staff: user.is_staff,
        groups,
        permissions: user.all_permissions_sorted(),
        auth: auth.to_string(),
    })
}

pub(crate) async fn ui_settings(store: &Store, p: &Principal) -> DomainResult<Value> {
    let user = auth::user(p)?;
    let v: Option<Option<sqlx::types::Json<Value>>> =
        sqlx::query_scalar("SELECT settings FROM documents_uisettings WHERE user_id = $1::int4")
            .bind(user.id)
            .fetch_optional(&store.db)
            .await
            .map_err(db_err)?;
    Ok(v.flatten().map(|j| j.0).unwrap_or_else(|| Value::Object(Default::default())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_color_like_v1() {
        assert_eq!(text_color("#a6cee3"), "#000000");
        assert_eq!(text_color("#1f78b4"), "#ffffff");
        assert_eq!(text_color("zz"), "#000000");
    }

    #[test]
    fn like_escapes() {
        assert_eq!(like_pattern("a%b_c"), "%a\\%b\\_c%");
    }
}
