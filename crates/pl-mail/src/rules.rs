//! `/api/mail_rules/` – MailRuleViewSet.
//!
//! upstream: src/paperless_mail/views.py@v3.4.2 (MailRuleViewSet), src/paperless_mail/serialisers.py@v3.4.2
//! (AccountField, MailRuleSerializer: create/update/validate/validate_account/validate_maximum_age),
//! src/paperless_mail/models.py (MailRule + Choices + UniqueConstraints), rest_framework/validators.py@3.16.1
//! (UniqueValidator with condition, UniqueTogetherValidator).

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use pl_core::AppState;
use serde_json::{Map, Value, json};
use sqlx::PgPool;

use crate::body::{as_object, parse_body};
use crate::ctx::MailUser;
use crate::drf::{self, CharConv, Errors, FieldError, Got, OPTIONAL, OPTIONAL_NULL, REQUIRED, raw};
use crate::error::{ApiError, ApiResult, json_response};
use crate::listing::{self, ReqInfo};
use crate::perms::{self, MAIL_ACCOUNT, MAIL_RULE, PermFields, ValidSetPermissions};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RuleRow {
    pub id: i32,
    pub name: String,
    pub account_id: i32,
    pub enabled: bool,
    pub folder: String,
    pub filter_from: Option<String>,
    pub filter_to: Option<String>,
    pub filter_subject: Option<String>,
    pub filter_body: Option<String>,
    pub filter_attachment_filename_include: Option<String>,
    pub filter_attachment_filename_exclude: Option<String>,
    pub maximum_age: i16,
    pub action: i16,
    pub action_parameter: Option<String>,
    pub assign_title_from: i16,
    pub assign_correspondent_from: i16,
    pub assign_correspondent_id: Option<i32>,
    pub assign_document_type_id: Option<i32>,
    pub assign_owner_from_rule: bool,
    pub order: i16,
    pub attachment_type: i16,
    pub consumption_scope: i16,
    pub pdf_layout: i16,
    pub owner_id: Option<i32>,
    pub stop_processing: bool,
}

const COLS: &str = "r.id, r.name, r.account_id, r.enabled, r.folder, r.filter_from, r.filter_to, \
    r.filter_subject, r.filter_body, r.filter_attachment_filename_include, \
    r.filter_attachment_filename_exclude, r.maximum_age, r.action, r.action_parameter, \
    r.assign_title_from, r.assign_correspondent_from, r.assign_correspondent_id, \
    r.assign_document_type_id, r.assign_owner_from_rule, r.\"order\", r.attachment_type, \
    r.consumption_scope, r.pdf_layout, r.owner_id, r.stop_processing";

/// Tag IDs per rule, sorted like `Tag.Meta.ordering = ("name",)`.
async fn tags_for(db: &PgPool, ids: &[i32]) -> ApiResult<HashMap<i32, Vec<i32>>> {
    let rows: Vec<(i32, i32)> = sqlx::query_as(
        "SELECT at.mailrule_id, t.id FROM documents_tag t \
         JOIN paperless_mail_mailrule_assign_tags at ON at.tag_id = t.id \
         WHERE at.mailrule_id = ANY($1) ORDER BY t.name ASC",
    )
    .bind(ids)
    .fetch_all(db)
    .await?;
    let mut m: HashMap<i32, Vec<i32>> = HashMap::new();
    for (rule, tag) in rows {
        m.entry(rule).or_default().push(tag);
    }
    Ok(m)
}

fn represent(r: &RuleRow, tags: &[i32], pf: &PermFields) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(r.id));
    m.insert("name".into(), json!(r.name));
    m.insert("account".into(), json!(r.account_id));
    m.insert("enabled".into(), json!(r.enabled));
    m.insert("folder".into(), json!(r.folder));
    m.insert("filter_from".into(), json!(r.filter_from));
    m.insert("filter_to".into(), json!(r.filter_to));
    m.insert("filter_subject".into(), json!(r.filter_subject));
    m.insert("filter_body".into(), json!(r.filter_body));
    m.insert("filter_attachment_filename_include".into(), json!(r.filter_attachment_filename_include));
    m.insert("filter_attachment_filename_exclude".into(), json!(r.filter_attachment_filename_exclude));
    m.insert("maximum_age".into(), json!(r.maximum_age));
    m.insert("action".into(), json!(r.action));
    m.insert("action_parameter".into(), json!(r.action_parameter));
    m.insert("assign_title_from".into(), json!(r.assign_title_from));
    m.insert("assign_tags".into(), json!(tags));
    m.insert("assign_correspondent_from".into(), json!(r.assign_correspondent_from));
    m.insert("assign_correspondent".into(), json!(r.assign_correspondent_id));
    m.insert("assign_document_type".into(), json!(r.assign_document_type_id));
    m.insert("assign_owner_from_rule".into(), json!(r.assign_owner_from_rule));
    m.insert("order".into(), json!(r.order));
    m.insert("attachment_type".into(), json!(r.attachment_type));
    m.insert("consumption_scope".into(), json!(r.consumption_scope));
    m.insert("pdf_layout".into(), json!(r.pdf_layout));
    m.insert("owner".into(), json!(r.owner_id));
    pf.insert(&mut m, r.id);
    m.insert("stop_processing".into(), json!(r.stop_processing));
    Value::Object(m)
}

async fn represent_one(db: &PgPool, user: &MailUser, info: &ReqInfo, r: &RuleRow) -> ApiResult<Value> {
    let pf = PermFields::load(db, user, MAIL_RULE, info.full_perms(), &[(r.id, r.owner_id)]).await?;
    let tags = tags_for(db, &[r.id]).await?;
    Ok(represent(r, tags.get(&r.id).map(Vec::as_slice).unwrap_or(&[]), &pf))
}

async fn fetch(db: &PgPool, id: i32) -> ApiResult<Option<RuleRow>> {
    let sql = format!("SELECT {COLS} FROM paperless_mail_mailrule r WHERE r.id = $1");
    Ok(sqlx::query_as(&sql).bind(id).fetch_optional(db).await?)
}

async fn get_object(db: &PgPool, user: &MailUser, method: &Method, raw_pk: &str) -> ApiResult<RuleRow> {
    let Some(pk) = crate::util::url_pk(raw_pk)? else {
        return Err(ApiError::not_found_model(MAIL_RULE.object_name));
    };
    let visible = perms::permitted_among(db, user, MAIL_RULE, "view_mailrule", &[pk]).await?;
    if !visible.contains(&pk) {
        return Err(ApiError::not_found_model(MAIL_RULE.object_name));
    }
    let row = fetch(db, pk)
        .await?
        .ok_or_else(|| ApiError::not_found_model(MAIL_RULE.object_name))?;
    let ct = perms::content_type_id(db, MAIL_RULE).await?;
    perms::check_object_permission(db, user, method, MAIL_RULE, ct, row.id, row.owner_id).await?;
    Ok(row)
}

// ---------------------------------------------------------------------------------------------
// Serializer (input)

#[derive(Debug)]
pub struct RuleInput {
    pub name: Got<String>,
    pub account: Got<i32>,
    pub enabled: Got<bool>,
    pub folder: Got<String>,
    pub filters: [Got<Option<String>>; 6],
    pub maximum_age: Got<i16>,
    pub action: Got<i16>,
    pub action_parameter: Got<Option<String>>,
    pub assign_title_from: Got<i16>,
    pub assign_tags: Got<Vec<i32>>,
    pub assign_correspondent_from: Got<i16>,
    pub assign_correspondent: Got<Option<i32>>,
    pub assign_document_type: Got<Option<i32>>,
    pub assign_owner_from_rule: Got<bool>,
    pub order: Got<i128>,
    pub attachment_type: Got<i16>,
    pub consumption_scope: Got<i16>,
    pub pdf_layout: Got<i16>,
    pub owner: Got<Option<i32>>,
    pub set_permissions: Got<ValidSetPermissions>,
    pub stop_processing: Got<bool>,
}

pub const FILTER_FIELDS: [&str; 6] = [
    "filter_from",
    "filter_to",
    "filter_subject",
    "filter_body",
    "filter_attachment_filename_include",
    "filter_attachment_filename_exclude",
];

fn nn<T>(g: Got<Option<T>>) -> Got<T> {
    match g {
        Got::Set(Some(v)) => Got::Set(v),
        _ => Got::Skip,
    }
}

fn field<T>(
    e: &mut Errors,
    data: &Map<String, Value>,
    name: &str,
    opts: drf::Opts,
    partial: bool,
    default: Option<T>,
    parse: impl FnOnce(&Value) -> Result<T, FieldError>,
) -> Got<Option<T>> {
    match drf::empty_values::<T>(raw(data, name), opts, partial, default) {
        Err(fe) => {
            e.add(name, fe);
            Got::Skip
        }
        Ok(Err(got)) => got,
        Ok(Ok(v)) => match parse(v) {
            Ok(t) => Got::Set(Some(t)),
            Err(fe) => {
                e.add(name, fe);
                Got::Skip
            }
        },
    }
}

/// FK field against a table without restriction (`CorrespondentField`, `DocumentTypeField`).
async fn fk_field(db: &PgPool, e: &mut Errors, data: &Map<String, Value>, name: &str, table: &str, partial: bool) -> Got<Option<i32>> {
    match drf::empty_values::<i32>(drf::raw_related(data, name), OPTIONAL_NULL, partial, None) {
        Err(fe) => {
            e.add(name, fe);
            Got::Skip
        }
        Ok(Err(g)) => g,
        Ok(Ok(v)) => match perms::resolve_pk(db, table, v).await {
            Ok(id) => Got::Set(Some(id)),
            Err(fe) => {
                e.add(name, fe);
                Got::Skip
            }
        },
    }
}

async fn unowned_name_exists(db: &PgPool, name: &str, exclude: Option<i32>) -> ApiResult<bool> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM paperless_mail_mailrule WHERE owner_id IS NULL AND name = $1 \
         AND ($2::int IS NULL OR id <> $2) LIMIT 1",
    )
    .bind(name)
    .bind(exclude)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

async fn owner_name_exists(db: &PgPool, owner: Option<i32>, name: &str, exclude: Option<i32>) -> ApiResult<bool> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM paperless_mail_mailrule WHERE owner_id IS NOT DISTINCT FROM $1 AND name = $2 \
         AND ($3::int IS NULL OR id <> $3) LIMIT 1",
    )
    .bind(owner)
    .bind(name)
    .bind(exclude)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// `MailRuleSerializer(...).is_valid()` incl. object validation.
pub async fn validate(
    db: &PgPool,
    user: &MailUser,
    data: &Map<String, Value>,
    partial: bool,
    instance: Option<&RuleRow>,
) -> ApiResult<RuleInput> {
    let mut e = Errors::default();
    let inst_id = instance.map(|i| i.id);

    // name: [UniqueValidator(owner IS NULL), MaxLength(256), ProhibitNull]
    let name = match drf::empty_values::<String>(raw(data, "name"), REQUIRED, partial, None) {
        Err(fe) => {
            e.add("name", fe);
            Got::Skip
        }
        Ok(Err(g)) => nn(g),
        Ok(Ok(v)) => match drf::char_convert(v, false) {
            Err(fe) => {
                e.add("name", fe);
                Got::Skip
            }
            Ok(CharConv::Blank) => Got::Set(String::new()),
            Ok(CharConv::Value(s)) => {
                let mut errs = Vec::new();
                if unowned_name_exists(db, &s, inst_id).await? {
                    errs.push(json!("mail rule with this name already exists."));
                }
                errs.extend(drf::char_validators(&s, Some(256)));
                if errs.is_empty() {
                    Got::Set(s)
                } else {
                    e.add("name", Value::Array(errs));
                    Got::Skip
                }
            }
        },
    };

    // account: AccountField (change-visible accounts) + validate_account
    let account = match drf::empty_values::<i32>(drf::raw_related(data, "account"), REQUIRED, partial, None) {
        Err(fe) => {
            e.add("account", fe);
            Got::Skip
        }
        Ok(Err(g)) => nn(g),
        Ok(Ok(v)) => match drf::pk_lookup(v) {
            Err(fe) => {
                e.add("account", fe);
                Got::Skip
            }
            Ok(drf::PkLookup::OutOfRange) => {
                e.add("account", drf::pk_does_not_exist(v));
                Got::Skip
            }
            Ok(drf::PkLookup::Id(id)) => {
                let ok = perms::permitted_among(db, user, MAIL_ACCOUNT, "change_mailaccount", &[id]).await?;
                if !ok.contains(&id) {
                    e.add("account", drf::pk_does_not_exist(v));
                    Got::Skip
                } else {
                    let owner: Option<(Option<i32>,)> =
                        sqlx::query_as("SELECT owner_id FROM paperless_mail_mailaccount WHERE id = $1")
                            .bind(id)
                            .fetch_optional(db)
                            .await?;
                    let ct = perms::content_type_id(db, MAIL_ACCOUNT).await?;
                    let owner = owner.and_then(|o| o.0);
                    if !perms::has_perms_owner_aware(db, user, "change_mailaccount", ct, id, owner).await? {
                        return Err(ApiError::detail(StatusCode::FORBIDDEN, "Insufficient permissions."));
                    }
                    Got::Set(id)
                }
            }
        },
    };

    let enabled = nn(field(&mut e, data, "enabled", OPTIONAL, partial, None, drf::boolean_field));
    let folder = nn(field(&mut e, data, "folder", OPTIONAL, partial, None, |v| {
        drf::char_field(v, drf::CharOpts { allow_blank: false, max_length: Some(256) })
    }));
    let filter_opts = drf::CharOpts { allow_blank: true, max_length: Some(256) };
    let filters = FILTER_FIELDS.map(|f| field(&mut e, data, f, OPTIONAL_NULL, partial, None, |v| drf::char_field(v, filter_opts)));
    let maximum_age = nn(field(&mut e, data, "maximum_age", OPTIONAL, partial, None, |v| {
        let n = drf::integer_field(v, Some(0), Some(32767))?;
        // validate_maximum_age
        if n > 36500 {
            return Err(drf::err("Maximum mail age is unreasonably large."));
        }
        Ok(n as i16)
    }));
    let action = nn(field(&mut e, data, "action", OPTIONAL, partial, None, |v| drf::choice_field(v, &[1, 2, 3, 4, 5])));
    // action_parameter: CharField(allow_null=True, required=False, default="")
    let action_parameter = field(&mut e, data, "action_parameter", OPTIONAL_NULL, partial, Some(String::new()), |v| {
        drf::char_field(v, drf::CharOpts { allow_blank: false, max_length: None })
    });
    let assign_title_from = nn(field(&mut e, data, "assign_title_from", OPTIONAL, partial, None, |v| {
        drf::choice_field(v, &[1, 2, 3])
    }));
    // assign_tags: ManyRelatedField(TagsField)
    let assign_tags = match drf::empty_values::<Vec<i32>>(raw(data, "assign_tags"), OPTIONAL, partial, None) {
        Err(fe) => {
            e.add("assign_tags", fe);
            Got::Skip
        }
        Ok(Err(g)) => nn(g),
        Ok(Ok(v)) => match drf::many_list(v) {
            Err(fe) => {
                e.add("assign_tags", fe);
                Got::Skip
            }
            Ok(items) => {
                // Element null → PrimaryKeyRelatedField.to_internal_value(None) → queryset.get(pk=None)
                match perms::resolve_pks(db, "documents_tag", &items).await {
                    Err(fe) => {
                        e.add("assign_tags", fe);
                        Got::Skip
                    }
                    Ok(ids) => Got::Set(ids),
                }
            }
        },
    };
    let assign_correspondent_from = nn(field(&mut e, data, "assign_correspondent_from", OPTIONAL, partial, None, |v| {
        drf::choice_field(v, &[1, 2, 3, 4])
    }));
    let assign_correspondent = fk_field(db, &mut e, data, "assign_correspondent", "documents_correspondent", partial).await;
    let assign_document_type = fk_field(db, &mut e, data, "assign_document_type", "documents_documenttype", partial).await;
    let assign_owner_from_rule = nn(field(&mut e, data, "assign_owner_from_rule", OPTIONAL, partial, None, drf::boolean_field));
    let order = nn(field(&mut e, data, "order", OPTIONAL, partial, None, |v| drf::integer_field(v, None, None)));
    let attachment_type = nn(field(&mut e, data, "attachment_type", OPTIONAL, partial, None, |v| drf::choice_field(v, &[1, 2])));
    let consumption_scope = nn(field(&mut e, data, "consumption_scope", OPTIONAL, partial, None, |v| {
        drf::choice_field(v, &[1, 2, 3])
    }));
    let pdf_layout = nn(field(&mut e, data, "pdf_layout", OPTIONAL, partial, None, |v| {
        drf::choice_field(v, &[0, 1, 2, 3, 4])
    }));
    // owner: default=None (UniqueConstraint name/owner), allow_null
    let owner = match drf::empty_values::<i32>(drf::raw_related(data, "owner"), OPTIONAL_NULL, partial, None) {
        Err(fe) => {
            e.add("owner", fe);
            Got::Skip
        }
        Ok(Err(Got::Skip)) => {
            if partial { Got::Skip } else { Got::Set(None) }
        }
        Ok(Err(g)) => g,
        Ok(Ok(v)) => match perms::resolve_user_pk(db, v).await {
            Ok(id) => Got::Set(Some(id)),
            Err(fe) => {
                e.add("owner", fe);
                Got::Skip
            }
        },
    };
    let set_permissions = crate::util::set_permissions_field(db, &mut e, data, partial).await;
    let stop_processing = nn(field(&mut e, data, "stop_processing", OPTIONAL, partial, None, drf::boolean_field));

    if !e.is_empty() {
        return Err(ApiError::bad_request(e.into_value()));
    }

    // run_validators: UniqueTogetherValidator(name, owner)
    let (u_name, u_owner) = match instance {
        None => (
            match &name { Got::Set(n) => Some(n.clone()), Got::Skip => None },
            match &owner { Got::Set(o) => *o, Got::Skip => None },
        ),
        Some(inst) => (
            Some(match &name { Got::Set(n) => n.clone(), Got::Skip => inst.name.clone() }),
            match &owner { Got::Set(o) => *o, Got::Skip => inst.owner_id },
        ),
    };
    let checked: Vec<Option<()>> = match instance {
        None => vec![u_name.as_ref().map(|_| ()), u_owner.map(|_| ())],
        Some(inst) => {
            let mut v = Vec::new();
            if u_name.as_deref() != Some(inst.name.as_str()) {
                v.push(u_name.as_ref().map(|_| ()));
            }
            if u_owner != inst.owner_id {
                v.push(u_owner.map(|_| ()));
            }
            v
        }
    };
    if !checked.is_empty()
        && checked.iter().all(Option::is_some)
        && let Some(n) = &u_name
        && owner_name_exists(db, u_owner, n, inst_id).await?
    {
        return Err(ApiError::bad_request(json!({
            "non_field_errors": ["The fields name, owner must make a unique set."]
        })));
    }

    // validate(): action TAG/MOVE needs action_parameter
    if let Got::Set(a) = action {
        let param_empty = match &action_parameter {
            Got::Set(Some(p)) => p.is_empty(),
            _ => true,
        };
        if (a == 2 || a == 5) && param_empty {
            return Err(ApiError::bad_request(json!({
                "non_field_errors": ["An action parameter is required."]
            })));
        }
    }

    Ok(RuleInput {
        name,
        account,
        enabled,
        folder,
        filters,
        maximum_age,
        action,
        action_parameter,
        assign_title_from,
        assign_tags,
        assign_correspondent_from,
        assign_correspondent,
        assign_document_type,
        assign_owner_from_rule,
        order,
        attachment_type,
        consumption_scope,
        pdf_layout,
        owner,
        set_permissions,
        stop_processing,
    })
}

fn order_i16(n: i128) -> ApiResult<i16> {
    i16::try_from(n).map_err(|_| ApiError::internal("order out of smallint range"))
}

fn unique_together_error() -> ApiError {
    ApiError::bad_request(json!({"error": "Object violates owner / name unique constraint"}))
}

/// Applies validated data to a row (ModelSerializer.update/create).
fn apply(row: &mut RuleRow, inp: &RuleInput) -> ApiResult<()> {
    if let Got::Set(v) = &inp.name { row.name = v.clone(); }
    if let Got::Set(v) = inp.account { row.account_id = v; }
    if let Got::Set(v) = inp.enabled { row.enabled = v; }
    if let Got::Set(v) = &inp.folder { row.folder = v.clone(); }
    let targets = [
        &mut row.filter_from,
        &mut row.filter_to,
        &mut row.filter_subject,
        &mut row.filter_body,
        &mut row.filter_attachment_filename_include,
        &mut row.filter_attachment_filename_exclude,
    ];
    for (t, g) in targets.into_iter().zip(inp.filters.iter()) {
        if let Got::Set(v) = g {
            *t = v.clone();
        }
    }
    if let Got::Set(v) = inp.maximum_age { row.maximum_age = v; }
    if let Got::Set(v) = inp.action { row.action = v; }
    if let Got::Set(v) = &inp.action_parameter { row.action_parameter = v.clone(); }
    if let Got::Set(v) = inp.assign_title_from { row.assign_title_from = v; }
    if let Got::Set(v) = inp.assign_correspondent_from { row.assign_correspondent_from = v; }
    if let Got::Set(v) = inp.assign_correspondent { row.assign_correspondent_id = v; }
    if let Got::Set(v) = inp.assign_document_type { row.assign_document_type_id = v; }
    if let Got::Set(v) = inp.assign_owner_from_rule { row.assign_owner_from_rule = v; }
    if let Got::Set(v) = inp.order { row.order = order_i16(v)?; }
    if let Got::Set(v) = inp.attachment_type { row.attachment_type = v; }
    if let Got::Set(v) = inp.consumption_scope { row.consumption_scope = v; }
    if let Got::Set(v) = inp.pdf_layout { row.pdf_layout = v; }
    if let Got::Set(v) = inp.owner { row.owner_id = v; }
    if let Got::Set(v) = inp.stop_processing { row.stop_processing = v; }
    Ok(())
}

fn default_row(account_id: i32) -> RuleRow {
    RuleRow {
        id: 0,
        name: String::new(),
        account_id,
        enabled: true,
        folder: "INBOX".into(),
        filter_from: None,
        filter_to: None,
        filter_subject: None,
        filter_body: None,
        filter_attachment_filename_include: None,
        filter_attachment_filename_exclude: None,
        maximum_age: 30,
        action: 3,
        action_parameter: None,
        assign_title_from: 1,
        assign_correspondent_from: 1,
        assign_correspondent_id: None,
        assign_document_type_id: None,
        assign_owner_from_rule: true,
        order: 0,
        attachment_type: 1,
        consumption_scope: 1,
        pdf_layout: 0,
        owner_id: None,
        stop_processing: false,
    }
}

const WRITE_COLS: &str = "name, account_id, enabled, folder, filter_from, filter_to, filter_subject, \
    filter_body, filter_attachment_filename_include, filter_attachment_filename_exclude, maximum_age, \
    action, action_parameter, assign_title_from, assign_correspondent_from, assign_correspondent_id, \
    assign_document_type_id, assign_owner_from_rule, \"order\", attachment_type, consumption_scope, \
    pdf_layout, owner_id, stop_processing";

macro_rules! bind_row {
    ($q:expr, $r:expr) => {
        $q.bind(&$r.name)
            .bind($r.account_id)
            .bind($r.enabled)
            .bind(&$r.folder)
            .bind(&$r.filter_from)
            .bind(&$r.filter_to)
            .bind(&$r.filter_subject)
            .bind(&$r.filter_body)
            .bind(&$r.filter_attachment_filename_include)
            .bind(&$r.filter_attachment_filename_exclude)
            .bind($r.maximum_age)
            .bind($r.action)
            .bind(&$r.action_parameter)
            .bind($r.assign_title_from)
            .bind($r.assign_correspondent_from)
            .bind($r.assign_correspondent_id)
            .bind($r.assign_document_type_id)
            .bind($r.assign_owner_from_rule)
            .bind($r.order)
            .bind($r.attachment_type)
            .bind($r.consumption_scope)
            .bind($r.pdf_layout)
            .bind($r.owner_id)
            .bind($r.stop_processing)
    };
}

async fn set_tags_in(tx: &mut sqlx::PgConnection, rule_id: i32, tags: &[i32]) -> ApiResult<()> {
    // ManyRelatedManager.set(): remove missing, add new ones
    sqlx::query("DELETE FROM paperless_mail_mailrule_assign_tags WHERE mailrule_id = $1 AND NOT (tag_id = ANY($2))")
        .bind(rule_id)
        .bind(tags)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO paperless_mail_mailrule_assign_tags (mailrule_id, tag_id) \
         SELECT $1, t FROM UNNEST($2::int[]) AS t \
         WHERE NOT EXISTS (SELECT 1 FROM paperless_mail_mailrule_assign_tags x WHERE x.mailrule_id = $1 AND x.tag_id = t) \
         ON CONFLICT DO NOTHING",
    )
    .bind(rule_id)
    .bind(tags)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Building blocks for other crates (PaperRust pl-automation: save mail rule + workflows together)

/// Read a mail rule like the ViewSet's `get_object` (visibility + object permission for `method`).
pub async fn load_for(db: &PgPool, user: &MailUser, method: &Method, id: i32) -> ApiResult<RuleRow> {
    get_object(db, user, method, &id.to_string()).await
}

/// Representation like `GET /api/mail_rules/{id}/` (`full_perms` like the query parameter).
pub async fn render(db: &PgPool, user: &MailUser, full_perms: bool, r: &RuleRow) -> ApiResult<Value> {
    let pf = PermFields::load(db, user, MAIL_RULE, full_perms, &[(r.id, r.owner_id)]).await?;
    let tags = tags_for(db, &[r.id]).await?;
    Ok(represent(r, tags.get(&r.id).map(Vec::as_slice).unwrap_or(&[]), &pf))
}

/// Create (`existing = None`) or change in the caller's transaction: same validation
/// (`MailRuleSerializer`) and same write steps as `POST`/`PUT`/`PATCH /api/mail_rules/`.
/// The caller checks the model permission ([`perms::require_model_perm`]).
pub async fn save_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    db: &PgPool,
    user: &MailUser,
    data: &Map<String, Value>,
    existing: Option<RuleRow>,
    partial: bool,
) -> ApiResult<RuleRow> {
    match existing {
        None => {
            let inp = validate(db, user, data, false, None).await?;
            let Got::Set(account_id) = inp.account else {
                return Err(ApiError::internal("account missing after validation"));
            };
            let mut row = default_row(account_id);
            apply(&mut row, &inp)?;
            // OwnedObjectSerializer.create: owner missing in the request → request.user
            if !data.contains_key("owner") {
                row.owner_id = Some(user.id);
            }
            if owner_name_exists(db, row.owner_id, &row.name, None).await? {
                return Err(unique_together_error());
            }
            let placeholders = (1..=24).map(|i| format!("${i}")).collect::<Vec<_>>().join(", ");
            let sql = format!("INSERT INTO paperless_mail_mailrule ({WRITE_COLS}) VALUES ({placeholders}) RETURNING id");
            let (id,): (i32,) = bind_row!(sqlx::query_as(&sql), row).fetch_one(&mut **tx).await?;
            row.id = id;
            if let Got::Set(sp) = &inp.set_permissions {
                perms::apply_set_permissions(tx, MAIL_RULE, id, sp).await?;
            }
            if let Got::Set(tags) = &inp.assign_tags
                && !tags.is_empty()
            {
                set_tags_in(tx, id, tags).await?;
            }
            Ok(row)
        }
        Some(mut row) => {
            let inp = validate(db, user, data, partial, Some(&row)).await?;
            // OwnedObjectSerializer.update
            let owner_changes = matches!(&inp.owner, Got::Set(o) if *o != row.owner_id);
            let has_sp = matches!(inp.set_permissions, Got::Set(_));
            let is_owner = row.owner_id == Some(user.id);
            if (owner_changes || has_sp) && !(user.is_superuser || is_owner || row.owner_id.is_none()) {
                return Err(ApiError::detail(StatusCode::FORBIDDEN, "Insufficient permissions."));
            }
            if let Got::Set(sp) = &inp.set_permissions {
                perms::apply_set_permissions(tx, MAIL_RULE, row.id, sp).await?;
            }
            if let Got::Set(owner) = &inp.owner {
                let name = match &inp.name {
                    Got::Set(n) => n.clone(),
                    Got::Skip => row.name.clone(),
                };
                if owner_name_exists(db, *owner, &name, Some(row.id)).await? {
                    return Err(unique_together_error());
                }
            }
            apply(&mut row, &inp)?;
            let sets = WRITE_COLS
                .split(", ")
                .enumerate()
                .map(|(i, c)| format!("{} = ${}", c.trim(), i + 2))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!("UPDATE paperless_mail_mailrule SET {sets} WHERE id = $1");
            bind_row!(sqlx::query(&sql).bind(row.id), row).execute(&mut **tx).await?;
            if let Got::Set(tags) = &inp.assign_tags {
                set_tags_in(tx, row.id, tags).await?;
            }
            Ok(row)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Handler

async fn list(State(st): State<AppState>, info: ReqInfo, user: MailUser) -> ApiResult<Response> {
    let db = &st.inner.db;
    perms::require_model_perm(db, &user, &Method::GET, MAIL_RULE).await?;
    let ct = perms::content_type_id(db, MAIL_RULE).await?;
    let (cond, binds) = crate::util::visible_where(&user, "r", MAIL_RULE, 1);
    let count_sql = format!("SELECT COUNT(*) FROM paperless_mail_mailrule r WHERE {cond}");
    let count: (i64,) = crate::util::bind_vis(sqlx::query_as(&count_sql), &user, binds, MAIL_RULE, ct)
        .fetch_one(db)
        .await?;
    let page = listing::paginate(&info, count.0)?;
    let sql = format!(
        "SELECT {COLS} FROM paperless_mail_mailrule r WHERE {cond} ORDER BY r.\"order\" ASC LIMIT {} OFFSET {}",
        page.size,
        page.offset()
    );
    let rows: Vec<RuleRow> = crate::util::bind_vis(sqlx::query_as(&sql), &user, binds, MAIL_RULE, ct)
        .fetch_all(db)
        .await?;
    let all_ids = if info.api_version < 10 {
        let sql = format!("SELECT r.id FROM paperless_mail_mailrule r WHERE {cond} ORDER BY r.\"order\" ASC");
        let ids: Vec<(i32,)> = crate::util::bind_vis(sqlx::query_as(&sql), &user, binds, MAIL_RULE, ct)
            .fetch_all(db)
            .await?;
        Some(ids.into_iter().map(|r| r.0).collect())
    } else {
        None
    };
    let objs: Vec<(i32, Option<i32>)> = rows.iter().map(|r| (r.id, r.owner_id)).collect();
    let ids: Vec<i32> = rows.iter().map(|r| r.id).collect();
    let pf = PermFields::load(db, &user, MAIL_RULE, info.full_perms(), &objs).await?;
    let tags = tags_for(db, &ids).await?;
    let results = rows
        .iter()
        .map(|r| represent(r, tags.get(&r.id).map(Vec::as_slice).unwrap_or(&[]), &pf))
        .collect();
    Ok(json_response(StatusCode::OK, &listing::paginated(&info, &page, results, all_ids)))
}

async fn retrieve(State(st): State<AppState>, info: ReqInfo, user: MailUser, Path(id): Path<String>) -> ApiResult<Response> {
    let db = &st.inner.db;
    perms::require_model_perm(db, &user, &Method::GET, MAIL_RULE).await?;
    let row = get_object(db, &user, &Method::GET, &id).await?;
    Ok(json_response(StatusCode::OK, &represent_one(db, &user, &info, &row).await?))
}

async fn create(State(st): State<AppState>, info: ReqInfo, user: MailUser, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    let db = &st.inner.db;
    perms::require_model_perm(db, &user, &Method::POST, MAIL_RULE).await?;
    let data = as_object(parse_body(&headers, &body)?)?;
    // PaperRust: rule, permissions and tags in one transaction (previously three separate steps)
    let mut tx = db.begin().await?;
    let row = save_in(&mut tx, db, &user, &data, None, false).await?;
    tx.commit().await?;
    Ok(json_response(StatusCode::CREATED, &represent_one(db, &user, &info, &row).await?))
}

async fn update_impl(st: AppState, info: ReqInfo, user: MailUser, id: String, headers: HeaderMap, body: Bytes, method: Method) -> ApiResult<Response> {
    let db = &st.inner.db;
    let partial = method == Method::PATCH;
    perms::require_model_perm(db, &user, &method, MAIL_RULE).await?;
    let row = get_object(db, &user, &method, &id).await?;
    let data = as_object(parse_body(&headers, &body)?)?;
    // PaperRust: permissions, rule and tags in one transaction (previously three separate steps)
    let mut tx = db.begin().await?;
    let row = save_in(&mut tx, db, &user, &data, Some(row), partial).await?;
    tx.commit().await?;
    Ok(json_response(StatusCode::OK, &represent_one(db, &user, &info, &row).await?))
}

async fn update(State(st): State<AppState>, info: ReqInfo, user: MailUser, Path(id): Path<String>, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    update_impl(st, info, user, id, headers, body, Method::PUT).await
}

async fn partial_update(State(st): State<AppState>, info: ReqInfo, user: MailUser, Path(id): Path<String>, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    update_impl(st, info, user, id, headers, body, Method::PATCH).await
}

async fn destroy(State(st): State<AppState>, _info: ReqInfo, user: MailUser, Path(id): Path<String>) -> ApiResult<Response> {
    let db = &st.inner.db;
    perms::require_model_perm(db, &user, &Method::DELETE, MAIL_RULE).await?;
    let row = get_object(db, &user, &Method::DELETE, &id).await?;
    let mut tx = db.begin().await?;
    delete_rule_cascade(&mut tx, row.id).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Django's collector for `MailRule.delete()` (review 2 M3). All FKs on
/// `paperless_mail_mailrule` according to `reference/schema/postgres.sql`:
/// - `documents_workflowtrigger.filter_mailrule_id` → `on_delete=SET_NULL`
///   (`documents/models.py::WorkflowTrigger.filter_mailrule`)
/// - `paperless_mail_processedmail.rule_id` → `CASCADE`
/// - `paperless_mail_mailrule_assign_tags.mailrule_id` → M2M (join table)
///
/// Order like Django: first SET NULL (`field_updates`), then deletions.
pub(crate) const RULE_DELETE_SQL: &[&str] = &[
    "UPDATE documents_workflowtrigger SET filter_mailrule_id = NULL WHERE filter_mailrule_id = $1",
    "DELETE FROM paperless_mail_processedmail WHERE rule_id = $1",
    "DELETE FROM paperless_mail_mailrule_assign_tags WHERE mailrule_id = $1",
    "DELETE FROM paperless_mail_mailrule WHERE id = $1",
];

/// Runs [`RULE_DELETE_SQL`] in the caller's transaction.
pub async fn delete_rule_cascade(conn: &mut sqlx::PgConnection, rule_id: i32) -> Result<(), sqlx::Error> {
    for sql in RULE_DELETE_SQL {
        sqlx::query(sql).bind(rule_id).execute(&mut *conn).await?;
    }
    Ok(())
}

pub fn routes() -> Vec<pl_core::ApiRoute> {
    use axum::routing::{delete, get, patch, post, put};
    use pl_core::ApiRoute;
    use pl_core::route::{drf_view, ALLOW_MODEL_DETAIL, ALLOW_MODEL_LIST};
    const SRC: &str = "src/paperless_mail/views.py::MailRuleViewSet";
    vec![
        ApiRoute { id: "mail_rules.list", method: Method::GET, path: "/api/mail_rules/", upstream: SRC, handler: drf_view(get(list), ALLOW_MODEL_LIST) },
        ApiRoute { id: "mail_rules.create", method: Method::POST, path: "/api/mail_rules/", upstream: SRC, handler: drf_view(post(create), ALLOW_MODEL_LIST) },
        ApiRoute { id: "mail_rules.retrieve", method: Method::GET, path: "/api/mail_rules/{id}/", upstream: SRC, handler: drf_view(get(retrieve), ALLOW_MODEL_DETAIL) },
        ApiRoute { id: "mail_rules.update", method: Method::PUT, path: "/api/mail_rules/{id}/", upstream: SRC, handler: drf_view(put(update), ALLOW_MODEL_DETAIL) },
        ApiRoute { id: "mail_rules.partial_update", method: Method::PATCH, path: "/api/mail_rules/{id}/", upstream: SRC, handler: drf_view(patch(partial_update), ALLOW_MODEL_DETAIL) },
        ApiRoute { id: "mail_rules.destroy", method: Method::DELETE, path: "/api/mail_rules/{id}/", upstream: SRC, handler: drf_view(delete(destroy), ALLOW_MODEL_DETAIL) },
    ]
}

#[cfg(test)]
mod tests {
    use super::RULE_DELETE_SQL;

    /// Review 2 M3: the workflow trigger filter is cleared before the rule is deleted.
    #[test]
    fn rule_delete_nulls_workflow_trigger_before_delete() {
        let upd = RULE_DELETE_SQL
            .iter()
            .position(|s| s.starts_with("UPDATE documents_workflowtrigger SET filter_mailrule_id = NULL"))
            .expect("SET NULL on workflowtrigger");
        let del = RULE_DELETE_SQL
            .iter()
            .position(|s| s.starts_with("DELETE FROM paperless_mail_mailrule WHERE"))
            .expect("rule delete");
        assert!(upd < del);
        assert_eq!(del, RULE_DELETE_SQL.len() - 1);
    }
}
