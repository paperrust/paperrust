// upstream: src/documents/views.py@v3.4.2 (GlobalSearchView.get: object sections),
//           src/documents/permissions.py (restrict_queryset_to_visible),
//           src/documents/serialisers.py (SavedViewSerializer, TagSerializer, CorrespondentSerializer,
//           DocumentTypeSerializer, StoragePathSerializer, CustomFieldSerializer, WorkflowSerializer,
//           OwnedObjectSerializer.get_user_can_change),
//           src/paperless/serialisers.py (UserSerializer, GroupSerializer),
//           src/paperless_mail/serialisers.py (MailRuleSerializer, MailAccountSerializer),
//           Django 5.2 postgresql `icontains` (`UPPER(col::text) LIKE UPPER(%s)`, `prep_for_like_query`).
//
// Built-in object sections of `/api/search/`. Every section is
// `Model.objects.filter(name__icontains=query)` (users: `username`), restricted with
// `restrict_queryset_to_visible` where upstream does, in the model's `Meta.ordering`
// (no ORDER BY for models without one, so PostgreSQL returns the same rows as for Django),
// sliced to `[:3]` and serialized with the section's serializer.
//
// Serializer context: upstream passes only `context={"request": request}` – no `user=`
// kwarg, no `full_perms`, no `view`. Consequences reproduced here:
//   * `self.user` is None → `user_can_change` is `owner is None` (never the per-user check),
//   * `permissions` never appears (full_perms is False regardless of the query string),
//   * annotated `document_count` is missing on top-level objects (read-only field skipped),
//     but present on tag children (`get_children` annotates), children ordered by
//     `Lower("name")` (no view → no OrderingFilter),
//   * saved views: `show_on_dashboard`/`show_in_sidebar` for API version < 10.
//
// Serializers that live in crates pl-search cannot depend on (pl-mail → pl-consumer →
// pl-search, pl-fields-views keeps them private) are reproduced locally from those crates'
// implementations; see docs/contract/pl-search.md.

use std::collections::HashMap;
use std::sync::Arc;

use pl_authz::ObjectModel;
use pl_authz::User;
use pl_authz::models::{MAIL_ACCOUNT, MAIL_RULE, SAVED_VIEW};
use pl_authz::objects::restrict_to_visible;
use pl_core::RequestContext;
use pl_objects::model::{Kind, ObjectRow};
use pl_objects::serialize::{ChangeSource, SerCtx};
use sea_query::{Alias, Cond, Expr, Order, PostgresQueryBuilder, Query, SelectStatement};
use pl_core::sqlbind::SqlxBinder;
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Row};

use crate::db::RequestUser;
use crate::views::{BoxFut, GLOBAL_SECTIONS, GlobalSearchProviders, ObjectSearchProvider, ViewError};

fn a(s: &str) -> Alias {
    Alias::new(s)
}

/// Django `connection.ops.prep_for_like_query` + `%…%` (icontains).
pub fn icontains_pattern(q: &str) -> String {
    let mut s = String::with_capacity(q.len() + 2);
    s.push('%');
    for c in q.chars() {
        if matches!(c, '\\' | '%' | '_') {
            s.push('\\');
        }
        s.push(c);
    }
    s.push('%');
    s
}

/// `UPPER("<alias>"."<col>"::text) LIKE UPPER($n)`.
fn icontains(alias: &str, col: &str, q: &str) -> sea_query::SimpleExpr {
    Expr::cust_with_values(
        format!("UPPER(\"{alias}\".\"{col}\"::text) LIKE UPPER($1)"),
        [icontains_pattern(q)],
    )
}

/// Per-request data the built-in providers need.
pub struct Ctx {
    pub db: PgPool,
    pub user: User,
    pub tz: chrono_tz::Tz,
    pub api_version: u16,
    /// Request context for tag children (`ordering` removed: no view in upstream's context).
    pub request: RequestContext,
}

impl Ctx {
    fn restrict(&self, model: &ObjectModel, perm: &str, alias: &str) -> Cond {
        restrict_to_visible(Some(&self.user), model, perm, alias)
    }
}

/// The serializers' `self.user` (None): `get_user_can_change` → `owner is None`.
fn no_user() -> User {
    User {
        id: i64::MIN,
        username: String::new(),
        password: String::new(),
        first_name: String::new(),
        last_name: String::new(),
        email: String::new(),
        is_active: false,
        is_staff: false,
        is_superuser: false,
        last_login: None,
        date_joined: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        groups: Vec::new(),
        permissions: Default::default(),
    }
}

fn can_change(owner: Option<i64>) -> Value {
    Value::Bool(owner.is_none())
}

async fn ids_query(db: &PgPool, q: &SelectStatement) -> Result<Vec<i64>, ViewError> {
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(db).await?;
    rows.iter().map(|r| r.try_get::<i64, _>(0).map_err(ViewError::from)).collect()
}

fn api(e: pl_core::ApiError) -> ViewError {
    ViewError::Internal(e.to_string())
}

// --- tags, correspondents, document types, storage paths ---------------------------------------

async fn matching_objects(c: &Ctx, kind: Kind, query: &str, limit: usize) -> Result<Vec<Value>, ViewError> {
    use pl_objects::listing::{ALIAS, fetch, select};
    let mut q = select(kind, &c.user, false, false, false);
    q.and_where(icontains(ALIAS, "name", query));
    q.cond_where(c.restrict(kind.model(), kind.codename("view"), ALIAS));
    // MatchingModel.Meta.ordering = ("name",)
    q.order_by((a(ALIAS), a("name")), Order::Asc);
    q.limit(limit as u64);
    let rows: Vec<ObjectRow> = fetch(&c.db, kind, &q, false, false).await.map_err(api)?;
    let nobody = no_user();
    let no_grants = HashMap::new();
    let none = Default::default();
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let children = if kind == Kind::Tag {
            Some(pl_objects::objects::children_of(&c.db, &c.user, row, &c.request).await.map_err(api)?)
        } else {
            None
        };
        let sctx = SerCtx {
            user: &nobody,
            full_perms: false,
            change: ChangeSource::PerObject(&none),
            grantees: &no_grants,
            children: children.as_ref(),
        };
        out.push(pl_objects::serialize::serialize(kind, row, &sctx));
    }
    Ok(out)
}

// --- saved views (SavedViewSerializer, cf. pl-fields-views saved_views.rs) ---------------------

async fn saved_views(c: &Ctx, query: &str, limit: usize) -> Result<Vec<Value>, ViewError> {
    let mut q = Query::select();
    q.from_as(a("documents_savedview"), a("sv"));
    for (e, n) in [
        ("sv.id::int8", "id"),
        ("sv.name", "name"),
        ("sv.icon", "icon"),
        ("sv.sort_field", "sort_field"),
        ("sv.sort_reverse", "sort_reverse"),
        ("sv.page_size::int8", "page_size"),
        ("sv.display_mode", "display_mode"),
        ("sv.display_fields", "display_fields"),
        ("sv.owner_id::int8", "owner_id"),
    ] {
        q.expr_as(Expr::cust(e), a(n));
    }
    q.and_where(icontains("sv", "name", query));
    q.cond_where(c.restrict(&SAVED_VIEW, "view_savedview", "sv"));
    // SavedView.Meta.ordering = ("name",)
    q.order_by((a("sv"), a("name")), Order::Asc);
    q.limit(limit as u64);
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(&c.db).await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.try_get::<i64, _>("id")).collect::<Result<_, _>>()?;
    let mut rules: HashMap<i64, Vec<Value>> = HashMap::new();
    if !ids.is_empty() {
        let rr = sqlx::query(
            "SELECT saved_view_id::int8 AS sv, rule_type::int8 AS rule_type, value \
             FROM documents_savedviewfilterrule WHERE saved_view_id = ANY($1::int8[]) ORDER BY id",
        )
        .bind(&ids)
        .fetch_all(&c.db)
        .await?;
        for r in rr {
            let sv: i64 = r.try_get("sv")?;
            let rt: i64 = r.try_get("rule_type")?;
            let v: Option<String> = r.try_get("value")?;
            rules.entry(sv).or_default().push(json!({"rule_type": rt, "value": v}));
        }
    }
    let legacy = if c.api_version < 10 && !rows.is_empty() { Some(legacy_visibility(&c.db, c.user.id).await?) } else { None };
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let id: i64 = r.try_get("id")?;
        let owner: Option<i64> = r.try_get("owner_id")?;
        let mut m = Map::new();
        m.insert("id".into(), json!(id));
        m.insert("name".into(), json!(r.try_get::<String, _>("name")?));
        m.insert("icon".into(), json!(r.try_get::<Option<String>, _>("icon")?));
        m.insert("sort_field".into(), json!(r.try_get::<Option<String>, _>("sort_field")?));
        m.insert("sort_reverse".into(), json!(r.try_get::<bool, _>("sort_reverse")?));
        m.insert("filter_rules".into(), Value::Array(rules.remove(&id).unwrap_or_default()));
        m.insert("page_size".into(), json!(r.try_get::<Option<i64>, _>("page_size")?));
        m.insert("display_mode".into(), json!(r.try_get::<Option<String>, _>("display_mode")?));
        m.insert(
            "display_fields".into(),
            r.try_get::<Option<sqlx::types::Json<Value>>, _>("display_fields")?.map(|j| j.0).unwrap_or(Value::Null),
        );
        m.insert("owner".into(), json!(owner));
        m.insert("user_can_change".into(), can_change(owner));
        if let Some((dash, side)) = &legacy {
            m.insert("show_on_dashboard".into(), json!(py_in(dash, id)));
            m.insert("show_in_sidebar".into(), json!(py_in(side, id)));
        }
        out.push(Value::Object(m));
    }
    Ok(out)
}

/// `SavedViewSerializer.to_representation` (v < 10): `user.ui_settings.settings["saved_views"]`.
async fn legacy_visibility(db: &PgPool, uid: i64) -> Result<(Vec<Value>, Vec<Value>), ViewError> {
    let s: Option<Option<sqlx::types::Json<Value>>> =
        sqlx::query_scalar("SELECT settings FROM documents_uisettings WHERE user_id = $1::int8")
            .bind(uid)
            .fetch_optional(db)
            .await?;
    let mut out = (Vec::new(), Vec::new());
    let Some(Some(sqlx::types::Json(Value::Object(settings)))) = s else { return Ok(out) };
    // `ui_settings.settings or None`: empty dict → no saved views
    let Some(Value::Object(sv)) = settings.get("saved_views").cloned().or(Some(Value::Object(Map::new()))) else {
        return Ok(out);
    };
    for (key, target) in [("dashboard_views_visible_ids", &mut out.0), ("sidebar_views_visible_ids", &mut out.1)] {
        match sv.get(key) {
            None => {}
            Some(Value::Array(v)) => *target = v.clone(),
            // set("…") / set({…}): string elements never equal an int id
            Some(Value::String(_)) | Some(Value::Object(_)) => {}
            // set(None) / set(5) → TypeError (500 upstream)
            Some(_) => return Err(ViewError::Internal("saved_views visibility ids not iterable".into())),
        }
    }
    Ok(out)
}

/// Python `instance.id in set(values)`.
fn py_in(values: &[Value], id: i64) -> bool {
    values.iter().any(|v| match v {
        Value::Number(n) => n.as_f64() == Some(id as f64),
        Value::Bool(b) => (*b as i64) == id,
        _ => false,
    })
}

// --- users, groups (pl-auth-api serializers) ---------------------------------------------------

async fn users(c: &Ctx, query: &str, limit: usize) -> Result<Vec<Value>, ViewError> {
    let mut q = Query::select();
    q.expr(Expr::cust("\"auth_user\".\"id\"::int8")).from(a("auth_user"));
    q.and_where(icontains("auth_user", "username", query));
    q.limit(limit as u64);
    let ids = ids_query(&c.db, &q).await?;
    Ok(pl_auth_api::users::users_repr(&c.db, &c.tz, &ids).await?)
}

async fn groups(c: &Ctx, query: &str, limit: usize) -> Result<Vec<Value>, ViewError> {
    let mut q = Query::select();
    q.expr(Expr::cust("\"auth_group\".\"id\"::int8")).from(a("auth_group"));
    q.and_where(icontains("auth_group", "name", query));
    q.limit(limit as u64);
    let ids = ids_query(&c.db, &q).await?;
    Ok(pl_auth_api::groups::groups_repr(&c.db, &ids).await?)
}

// --- mail rules, mail accounts (cf. pl-mail rules.rs / accounts.rs) ----------------------------

async fn mail_rules(c: &Ctx, query: &str, limit: usize) -> Result<Vec<Value>, ViewError> {
    let mut q = Query::select();
    q.from_as(a("paperless_mail_mailrule"), a("r"));
    for (e, n) in [
        ("r.id::int8", "id"),
        ("r.name", "name"),
        ("r.account_id::int8", "account_id"),
        ("r.enabled", "enabled"),
        ("r.folder", "folder"),
        ("r.filter_from", "filter_from"),
        ("r.filter_to", "filter_to"),
        ("r.filter_subject", "filter_subject"),
        ("r.filter_body", "filter_body"),
        ("r.filter_attachment_filename_include", "filter_attachment_filename_include"),
        ("r.filter_attachment_filename_exclude", "filter_attachment_filename_exclude"),
        ("r.maximum_age::int8", "maximum_age"),
        ("r.action::int8", "action"),
        ("r.action_parameter", "action_parameter"),
        ("r.assign_title_from::int8", "assign_title_from"),
        ("r.assign_correspondent_from::int8", "assign_correspondent_from"),
        ("r.assign_correspondent_id::int8", "assign_correspondent_id"),
        ("r.assign_document_type_id::int8", "assign_document_type_id"),
        ("r.assign_owner_from_rule", "assign_owner_from_rule"),
        ("r.\"order\"::int8", "order"),
        ("r.attachment_type::int8", "attachment_type"),
        ("r.consumption_scope::int8", "consumption_scope"),
        ("r.pdf_layout::int8", "pdf_layout"),
        ("r.owner_id::int8", "owner_id"),
        ("r.stop_processing", "stop_processing"),
    ] {
        q.expr_as(Expr::cust(e), a(n));
    }
    q.and_where(icontains("r", "name", query));
    q.cond_where(c.restrict(&MAIL_RULE, "view_mailrule", "r"));
    q.limit(limit as u64);
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(&c.db).await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.try_get::<i64, _>("id")).collect::<Result<_, _>>()?;
    // assign_tags in Tag.Meta.ordering ("name",)
    let mut tags: HashMap<i64, Vec<i64>> = HashMap::new();
    if !ids.is_empty() {
        let tr: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT at.mailrule_id::int8, t.id::int8 FROM documents_tag t \
             JOIN paperless_mail_mailrule_assign_tags at ON at.tag_id = t.id \
             WHERE at.mailrule_id = ANY($1::int8[]) ORDER BY t.name ASC",
        )
        .bind(&ids)
        .fetch_all(&c.db)
        .await?;
        for (rule, tag) in tr {
            tags.entry(rule).or_default().push(tag);
        }
    }
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let id: i64 = r.try_get("id")?;
        let owner: Option<i64> = r.try_get("owner_id")?;
        let mut m = Map::new();
        m.insert("id".into(), json!(id));
        m.insert("name".into(), json!(r.try_get::<String, _>("name")?));
        m.insert("account".into(), json!(r.try_get::<i64, _>("account_id")?));
        m.insert("enabled".into(), json!(r.try_get::<bool, _>("enabled")?));
        m.insert("folder".into(), json!(r.try_get::<String, _>("folder")?));
        for f in [
            "filter_from",
            "filter_to",
            "filter_subject",
            "filter_body",
            "filter_attachment_filename_include",
            "filter_attachment_filename_exclude",
        ] {
            m.insert(f.into(), json!(r.try_get::<Option<String>, _>(f)?));
        }
        m.insert("maximum_age".into(), json!(r.try_get::<i64, _>("maximum_age")?));
        m.insert("action".into(), json!(r.try_get::<i64, _>("action")?));
        m.insert("action_parameter".into(), json!(r.try_get::<Option<String>, _>("action_parameter")?));
        m.insert("assign_title_from".into(), json!(r.try_get::<i64, _>("assign_title_from")?));
        m.insert("assign_tags".into(), json!(tags.remove(&id).unwrap_or_default()));
        m.insert("assign_correspondent_from".into(), json!(r.try_get::<i64, _>("assign_correspondent_from")?));
        m.insert("assign_correspondent".into(), json!(r.try_get::<Option<i64>, _>("assign_correspondent_id")?));
        m.insert("assign_document_type".into(), json!(r.try_get::<Option<i64>, _>("assign_document_type_id")?));
        m.insert("assign_owner_from_rule".into(), json!(r.try_get::<bool, _>("assign_owner_from_rule")?));
        m.insert("order".into(), json!(r.try_get::<i64, _>("order")?));
        m.insert("attachment_type".into(), json!(r.try_get::<i64, _>("attachment_type")?));
        m.insert("consumption_scope".into(), json!(r.try_get::<i64, _>("consumption_scope")?));
        m.insert("pdf_layout".into(), json!(r.try_get::<i64, _>("pdf_layout")?));
        m.insert("owner".into(), json!(owner));
        m.insert("user_can_change".into(), can_change(owner));
        m.insert("stop_processing".into(), json!(r.try_get::<bool, _>("stop_processing")?));
        out.push(Value::Object(m));
    }
    Ok(out)
}

/// `ObfuscatedPasswordField.to_representation`.
fn obfuscate(pw: &str) -> String {
    "*".repeat(pw.chars().count().max(10))
}

async fn mail_accounts(c: &Ctx, query: &str, limit: usize) -> Result<Vec<Value>, ViewError> {
    let mut q = Query::select();
    q.from_as(a("paperless_mail_mailaccount"), a("ma"));
    for (e, n) in [
        ("ma.id::int8", "id"),
        ("ma.name", "name"),
        ("ma.imap_server", "imap_server"),
        ("ma.imap_port::int8", "imap_port"),
        ("ma.imap_security::int8", "imap_security"),
        ("ma.username", "username"),
        ("ma.password", "password"),
        ("ma.character_set", "character_set"),
        ("ma.is_token", "is_token"),
        ("ma.owner_id::int8", "owner_id"),
        ("ma.account_type::int8", "account_type"),
        ("ma.expiration", "expiration"),
    ] {
        q.expr_as(Expr::cust(e), a(n));
    }
    q.and_where(icontains("ma", "name", query));
    q.cond_where(c.restrict(&MAIL_ACCOUNT, "view_mailaccount", "ma"));
    q.limit(limit as u64);
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(&c.db).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let owner: Option<i64> = r.try_get("owner_id")?;
        let exp: Option<chrono::DateTime<chrono::Utc>> = r.try_get("expiration")?;
        let mut m = Map::new();
        m.insert("id".into(), json!(r.try_get::<i64, _>("id")?));
        m.insert("name".into(), json!(r.try_get::<String, _>("name")?));
        m.insert("imap_server".into(), json!(r.try_get::<String, _>("imap_server")?));
        m.insert("imap_port".into(), json!(r.try_get::<Option<i64>, _>("imap_port")?));
        m.insert("imap_security".into(), json!(r.try_get::<i64, _>("imap_security")?));
        m.insert("username".into(), json!(r.try_get::<String, _>("username")?));
        m.insert("password".into(), json!(obfuscate(&r.try_get::<String, _>("password")?)));
        m.insert("character_set".into(), json!(r.try_get::<String, _>("character_set")?));
        m.insert("is_token".into(), json!(r.try_get::<bool, _>("is_token")?));
        m.insert("owner".into(), json!(owner));
        m.insert("user_can_change".into(), can_change(owner));
        m.insert("account_type".into(), json!(r.try_get::<i64, _>("account_type")?));
        m.insert("expiration".into(), pl_core::datetime::format_datetime_opt(exp.as_ref(), &c.tz));
        out.push(Value::Object(m));
    }
    Ok(out)
}

// --- workflows, custom fields ------------------------------------------------------------------

async fn workflows(c: &Ctx, query: &str, limit: usize) -> Result<Vec<Value>, ViewError> {
    use pl_workflows::schema::T_WORKFLOW;
    let mut q = pl_workflows::load::select_rows(T_WORKFLOW);
    q.and_where(icontains(T_WORKFLOW, "name", query));
    q.limit(limit as u64);
    let mut conn = c.db.acquire().await?;
    let rows = pl_workflows::load::fetch_rows(&mut conn, &q).await?;
    // WorkflowSerializer on a plain queryset (no Prefetch ordering of the viewset)
    let out = pl_workflows::load::render_workflows(&mut conn, &rows, false).await?;
    Ok(out.into_iter().map(Value::Object).collect())
}

async fn custom_fields(c: &Ctx, query: &str, limit: usize) -> Result<Vec<Value>, ViewError> {
    let mut q = Query::select();
    q.from_as(a("documents_customfield"), a("cf"));
    for (e, n) in [("cf.id::int8", "id"), ("cf.name", "name"), ("cf.data_type", "data_type"), ("cf.extra_data", "extra_data")] {
        q.expr_as(Expr::cust(e), a(n));
    }
    q.and_where(icontains("cf", "name", query));
    // CustomField.Meta.ordering = ("created",)
    q.order_by((a("cf"), a("created")), Order::Asc);
    q.limit(limit as u64);
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(&c.db).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let mut m = Map::new();
        m.insert("id".into(), json!(r.try_get::<i64, _>("id")?));
        m.insert("name".into(), json!(r.try_get::<String, _>("name")?));
        m.insert("data_type".into(), json!(r.try_get::<String, _>("data_type")?));
        m.insert(
            "extra_data".into(),
            r.try_get::<Option<sqlx::types::Json<Value>>, _>("extra_data")?.map(|j| j.0).unwrap_or(Value::Null),
        );
        // document_count: read-only field without annotation → SkipField (omitted)
        out.push(Value::Object(m));
    }
    Ok(out)
}

// --- provider glue -----------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Section {
    SavedViews,
    Objects(Kind),
    Users,
    Groups,
    MailRules,
    MailAccounts,
    Workflows,
    CustomFields,
}

impl Section {
    /// Model permission checked before the section (`request.user.has_perm(...)`).
    fn perm(self) -> &'static str {
        match self {
            Section::SavedViews => "documents.view_savedview",
            Section::Objects(Kind::Tag) => "documents.view_tag",
            Section::Objects(Kind::Correspondent) => "documents.view_correspondent",
            Section::Objects(Kind::DocumentType) => "documents.view_documenttype",
            Section::Objects(Kind::StoragePath) => "documents.view_storagepath",
            Section::Users => "auth.view_user",
            Section::Groups => "auth.view_group",
            Section::MailRules => "paperless_mail.view_mailrule",
            Section::MailAccounts => "paperless_mail.view_mailaccount",
            Section::Workflows => "documents.view_workflow",
            Section::CustomFields => "documents.view_customfield",
        }
    }

    fn of(name: &str) -> Option<Section> {
        Some(match name {
            "saved_views" => Section::SavedViews,
            "tags" => Section::Objects(Kind::Tag),
            "correspondents" => Section::Objects(Kind::Correspondent),
            "document_types" => Section::Objects(Kind::DocumentType),
            "storage_paths" => Section::Objects(Kind::StoragePath),
            "users" => Section::Users,
            "groups" => Section::Groups,
            "mail_rules" => Section::MailRules,
            "mail_accounts" => Section::MailAccounts,
            "workflows" => Section::Workflows,
            "custom_fields" => Section::CustomFields,
            _ => return None,
        })
    }
}

struct Builtin {
    ctx: Arc<Ctx>,
    section: Section,
}

impl ObjectSearchProvider for Builtin {
    fn search<'a>(&'a self, _user: &'a RequestUser, query: &'a str, limit: usize) -> BoxFut<'a, Result<Vec<Value>, ViewError>> {
        Box::pin(async move {
            let c = &*self.ctx;
            if !c.user.has_perm(self.section.perm()) {
                return Ok(Vec::new());
            }
            match self.section {
                Section::SavedViews => saved_views(c, query, limit).await,
                Section::Objects(kind) => matching_objects(c, kind, query, limit).await,
                Section::Users => users(c, query, limit).await,
                Section::Groups => groups(c, query, limit).await,
                Section::MailRules => mail_rules(c, query, limit).await,
                Section::MailAccounts => mail_accounts(c, query, limit).await,
                Section::Workflows => workflows(c, query, limit).await,
                Section::CustomFields => custom_fields(c, query, limit).await,
            }
        })
    }
}

/// Built-in providers for all sections of [`GLOBAL_SECTIONS`] for one request.
pub fn builtin_providers(ctx: Ctx) -> GlobalSearchProviders {
    let ctx = Arc::new(ctx);
    let mut p = GlobalSearchProviders::default();
    for name in GLOBAL_SECTIONS {
        if let Some(section) = Section::of(name) {
            p.providers.insert(name, Arc::new(Builtin { ctx: ctx.clone(), section }));
        }
    }
    p
}

/// Request context for tag children: without `ordering` (upstream has no view in the
/// serializer context, so `get_children` orders by `Lower("name")`).
pub fn children_context(mut rc: RequestContext) -> RequestContext {
    let pairs: Vec<(String, String)> = rc.query.pairs().iter().filter(|(k, _)| k != "ordering").cloned().collect();
    rc.query = pl_core::QueryParams::from_pairs(pairs);
    rc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_pattern_escapes_wildcards() {
        assert_eq!(icontains_pattern("inv"), "%inv%");
        assert_eq!(icontains_pattern("a%b_c\\d"), "%a\\%b\\_c\\\\d%");
    }

    #[test]
    fn icontains_sql_renumbers_placeholder() {
        let mut q = Query::select();
        q.expr(Expr::cust("1")).from(a("t"));
        q.and_where(Expr::col(a("x")).eq(5));
        q.and_where(icontains("t", "name", "in_v"));
        let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
        // ` /*t:…*/` = type signature of pl_core::sqlbind (BEFUNDE N4)
        let body = sql.split(" /*t:").next().unwrap_or(&sql);
        assert!(body.ends_with("(UPPER(\"t\".\"name\"::text) LIKE UPPER($2))"), "{sql}");
        assert_eq!(values.0.0.len(), 2);
    }

    #[test]
    fn all_sections_have_a_builtin() {
        for name in GLOBAL_SECTIONS {
            assert!(Section::of(name).is_some(), "{name}");
        }
    }

    #[test]
    fn nobody_can_change_only_unowned() {
        assert_eq!(can_change(None), Value::Bool(true));
        assert_eq!(can_change(Some(2)), Value::Bool(false));
    }

    #[test]
    fn python_set_membership() {
        assert!(py_in(&[json!(1), json!(2.0)], 2));
        assert!(py_in(&[json!(true)], 1));
        assert!(!py_in(&[json!("2")], 2));
    }
}
