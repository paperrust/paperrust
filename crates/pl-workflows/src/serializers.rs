//! Input validation (to_internal_value + validate) of the workflow serializers.
//! upstream: src/documents/serialisers.py@v3.4.2 (WorkflowTriggerSerializer.validate,
//! WorkflowActionWebhookSerializer.validate_url, WorkflowActionSerializer.validate,
//! WorkflowSerializer.validate), src/documents/validators.py@v3.4.2 (uri_validator/url_validator),
//! DRF 3.18.3 (Serializer.run_validation, ListSerializer.to_internal_value, UniqueValidator).

use crate::drf::{self, Ctx, FieldSpec, Out, VErr, msg, non_field};
use crate::schema::*;
use crate::{cfquery, jinja, pyfmt};
use pl_core::sqlbind::SqlxBinder;
use sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Map, Value, json};
use sqlx::PgConnection;

pub type Attrs = Map<String, Value>;

fn specs(fields: &'static [Field]) -> Vec<FieldSpec> {
    fields.iter().map(|f| f.spec).collect()
}

/// Result of `run_validation` of a nested field / serializer.
fn wrap_validate_err(e: VErr) -> VErr {
    match e {
        VErr::Detail(d) => VErr::Detail(drf::as_serializer_error(d)),
        other => other,
    }
}

/// `Serializer.run_validation` for a (nested) object with `allow_null = false`.
/// Returns the DRF list `["This field may not be null."]` for `null`.
fn expect_child_object(data: &Value) -> Result<&Map<String, Value>, VErr> {
    if data.is_null() {
        return Err(VErr::Detail(msg("This field may not be null.")));
    }
    drf::expect_object(data)
}

// ---------------------------------------------------------------- Trigger

/// `WorkflowTriggerSerializer.run_validation`. `instance_type`: type of the existing instance
/// (only for `/api/workflow_triggers/{id}/`).
pub async fn validate_trigger(db: &mut PgConnection, ctx: Ctx, data: &Value, instance_type: Option<i64>) -> Result<Attrs, VErr> {
    let obj = expect_child_object(data)?;
    let mut attrs = drf::validate_fields(db, ctx, &specs(TRIGGER_FIELDS), obj).await?;
    trigger_validate(db, &mut attrs, instance_type).await.map_err(wrap_validate_err)?;
    Ok(attrs)
}

async fn trigger_validate(db: &mut PgConnection, attrs: &mut Attrs, instance_type: Option<i64>) -> Result<(), VErr> {
    for key in ["filter_filename", "filter_path", "filter_custom_field_query"] {
        if let Some(Value::String(s)) = attrs.get(key)
            && s.is_empty()
        {
            attrs.insert(key.into(), Value::Null);
        }
    }
    if let Some(Value::String(q)) = attrs.get("filter_custom_field_query") {
        cfquery::validate(db, "filter_custom_field_query", q).await?;
    }
    let trigger_type = match attrs.get("type") {
        Some(v) => v.as_i64(),
        None => instance_type,
    };
    let absent_or_null = |k: &str| attrs.get(k).is_none_or(Value::is_null);
    if trigger_type == Some(TYPE_CONSUMPTION)
        && !attrs.contains_key("filter_mailrule")
        && absent_or_null("filter_filename")
        && absent_or_null("filter_path")
    {
        return Err(non_field("File name, path or mail rule filter are required"));
    }
    Ok(())
}

// ---------------------------------------------------------------- Email / webhook

/// Python `urllib.parse.urlparse` (scheme/netloc/path only) + `uri_validator`.
pub fn url_validator(value: &str) -> Result<(), String> {
    let unable = || format!("Unable to parse URI {value}");
    // urlsplit: strip leading C0 control characters/spaces, remove \t\r\n everywhere
    let cleaned: String =
        value.trim_start_matches(|c: char| (c as u32) <= 0x20).chars().filter(|c| !matches!(c, '\t' | '\r' | '\n')).collect();
    let mut scheme = String::new();
    let mut rest = cleaned.as_str();
    if let Some(i) = cleaned.find(':') {
        let cand = &cleaned[..i];
        if i > 0
            && cand.as_bytes()[0].is_ascii_alphabetic()
            && cand.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            scheme = cand.to_ascii_lowercase();
            rest = &cleaned[i + 1..];
        }
    }
    let mut netloc = "";
    let mut path_part = rest;
    if let Some(r) = rest.strip_prefix("//") {
        let end = r.find(['/', '?', '#']).unwrap_or(r.len());
        netloc = &r[..end];
        path_part = &r[end..];
        if netloc.contains('[') != netloc.contains(']') {
            return Err(unable());
        }
    }
    let path_end = path_part.find(['?', '#']).unwrap_or(path_part.len());
    let path = &path_part[..path_end];
    // Python 3.12 urlparse: ";params" is split off the last path segment
    let path = match path.rfind('/') {
        Some(slash) => match path[slash..].find(';') {
            Some(semi) => &path[..slash + semi],
            None => path,
        },
        None => match path.find(';') {
            Some(semi) => &path[..semi],
            None => path,
        },
    };
    if scheme.is_empty() {
        return Err(format!("Unable to parse URI {value}, missing scheme"));
    } else if netloc.is_empty() && path.is_empty() {
        return Err(format!("Unable to parse URI {value}, missing net location or path"));
    }
    if scheme != "http" && scheme != "https" {
        return Err(format!("URI scheme '{scheme}' is not allowed. Allowed schemes: http, https"));
    }
    Ok(())
}

/// `WorkflowActionEmailSerializer` / `WorkflowActionWebhookSerializer` `.run_validation`.
pub async fn validate_nested(db: &mut PgConnection, ctx: Ctx, name: &str, data: &Value) -> Result<Attrs, VErr> {
    let obj = drf::expect_object(data)?;
    let fields = if name == "email" { EMAIL_FIELDS } else { WEBHOOK_FIELDS };
    let mut ret = Map::new();
    let mut errors = Map::new();
    for spec in specs(fields) {
        let r = drf::run_field(db, ctx, &spec, obj.get(spec.name)).await;
        match r {
            Ok(Out::Val(v)) => {
                if name == "webhook"
                    && spec.name == "url"
                    && let Value::String(u) = &v
                    && let Err(m) = url_validator(u)
                {
                    errors.insert("url".into(), msg(m));
                    continue;
                }
                ret.insert(spec.name.into(), v);
            }
            Ok(Out::Skip) => {}
            Err(VErr::Detail(d)) => {
                errors.insert(spec.name.into(), d);
            }
            Err(e) => return Err(e),
        }
    }
    if !errors.is_empty() {
        return Err(VErr::Detail(Value::Object(errors)));
    }
    Ok(ret)
}

/// Re-validation on save (`WorkflowActionEmailSerializer(data=email_data)`, not partial).
/// The data is already validated; at most required fields are missing (PATCH).
pub fn revalidate_nested(name: &str, data: &Attrs) -> Result<(), VErr> {
    let fields = if name == "email" { EMAIL_FIELDS } else { WEBHOOK_FIELDS };
    let mut errors = Map::new();
    for f in fields {
        if f.spec.required && !data.contains_key(f.spec.name) {
            errors.insert(f.spec.name.into(), msg("This field is required."));
        }
    }
    if errors.is_empty() { Ok(()) } else { Err(VErr::Detail(Value::Object(errors))) }
}

// ---------------------------------------------------------------- Action

pub async fn validate_action(db: &mut PgConnection, ctx: Ctx, data: &Value) -> Result<Attrs, VErr> {
    let obj = expect_child_object(data)?;
    let mut ret = Map::new();
    let mut errors = Map::new();
    for f in ACTION_FIELDS {
        let spec = f.spec;
        let r = if matches!(f.col, Col::Nested(_)) {
            match obj.get(spec.name) {
                None => Ok(Out::Skip),
                Some(Value::Null) => Ok(Out::Val(Value::Null)),
                Some(v) => validate_nested(db, ctx, spec.name, v).await.map(|a| Out::Val(Value::Object(a))),
            }
        } else {
            drf::run_field(db, ctx, &spec, obj.get(spec.name)).await
        };
        match r {
            Ok(Out::Val(v)) => {
                ret.insert(spec.name.into(), v);
            }
            Ok(Out::Skip) => {}
            Err(VErr::Detail(d)) => {
                errors.insert(spec.name.into(), d);
            }
            Err(e) => return Err(e),
        }
    }
    if !errors.is_empty() {
        return Err(VErr::Detail(Value::Object(errors)));
    }
    action_validate(&mut ret).map_err(wrap_validate_err)?;
    Ok(ret)
}

fn action_validate(attrs: &mut Attrs) -> Result<(), VErr> {
    if let Some(Value::String(t)) = attrs.get("assign_title") {
        if t.is_empty() {
            attrs.insert("assign_title".into(), Value::Null);
        } else {
            match jinja::validate_template(t) {
                Ok(()) => {}
                Err(jinja::TemplateError::Invalid(m)) => return Err(VErr::Detail(json!({ "assign_title": m }))),
                Err(jinja::TemplateError::Internal(m)) => return Err(VErr::Server(m)),
            }
        }
    }
    if let Some(v) = attrs.get("assign_custom_fields_values")
        && py_truthy(v)
    {
        match v {
            Value::Object(o) => {
                let mapped: Map<String, Value> = o
                    .iter()
                    .map(|(k, val)| {
                        let nv = if val.as_str() == Some("") { Value::Null } else { val.clone() };
                        (k.clone(), nv)
                    })
                    .collect();
                attrs.insert("assign_custom_fields_values".into(), Value::Object(mapped));
            }
            // upstream: AttributeError (`.items()` on a non-dict) → 500
            _ => return Err(VErr::Server("assign_custom_fields_values is not a dict".into())),
        }
    }
    let ty = attrs.get("type").and_then(Value::as_i64);
    if ty == Some(ACTION_EMAIL) && !attrs.contains_key("email") {
        return Err(non_field("Email data is required for email actions"));
    }
    if ty == Some(ACTION_WEBHOOK) && !attrs.contains_key("webhook") {
        return Err(non_field("Webhook data is required for webhook actions"));
    }
    if ty == Some(ACTION_PASSWORD_REMOVAL) {
        let ok = match attrs.get("passwords") {
            Some(Value::Array(a)) => !a.is_empty() && a.iter().all(|p| p.as_str().is_some_and(|s| !pyfmt::py_strip(s).is_empty())),
            _ => false,
        };
        if !ok {
            return Err(non_field("Passwords are required for password removal actions"));
        }
    }
    if ty == Some(ACTION_AI) {
        let ok = match attrs.get("ai_suggestion_fields") {
            Some(Value::Array(a)) => {
                if a.iter().any(|v| v.is_array() || v.is_object()) {
                    // upstream: TypeError (unhashable) in `in set` → 500
                    if a.iter().position(|v| v.is_array() || v.is_object())
                        <= a.iter().position(|v| !v.as_str().is_some_and(|s| AI_FIELDS.contains(&s)))
                    {
                        return Err(VErr::Server("unhashable ai_suggestion_fields entry".into()));
                    }
                }
                !a.is_empty() && a.iter().all(|v| v.as_str().is_some_and(|s| AI_FIELDS.contains(&s)))
            }
            _ => false,
        };
        if !ok {
            let mut sorted: Vec<&str> = AI_FIELDS.to_vec();
            sorted.sort_unstable();
            let list: Vec<String> = sorted.iter().map(|s| pyfmt::str_repr(s)).collect();
            return Err(non_field(format!(
                "At least one valid field is required for apply AI suggestions actions, options are: [{}]",
                list.join(", ")
            )));
        }
    }
    Ok(())
}

/// Python truth value of a JSON value.
pub fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

// ---------------------------------------------------------------- Workflow

/// PaperRust: validate an action list like `WorkflowSerializer.actions` (for the "else" branch,
/// pl-automation). `None`/missing → empty list.
pub async fn validate_actions(db: &mut PgConnection, data: Option<&Value>) -> Result<Vec<Attrs>, VErr> {
    match data {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(v) => Ok(validate_list(db, Ctx { partial: false }, Some(v), false).await?.unwrap_or_default()),
    }
}

/// `ListSerializer.run_validation` for `triggers` / `actions`.
async fn validate_list(db: &mut PgConnection, ctx: Ctx, data: Option<&Value>, is_trigger: bool) -> Result<Option<Vec<Attrs>>, VErr> {
    let Some(data) = data else {
        if ctx.partial {
            return Ok(None);
        }
        return Err(VErr::Detail(msg("This field is required.")));
    };
    let items = match data {
        Value::Null => return Err(VErr::Detail(msg("This field may not be null."))),
        Value::Array(a) => a,
        other => {
            return Err(VErr::Detail(json!({"non_field_errors": [format!(
                "Expected a list of items but got type \"{}\".",
                pyfmt::type_name(other)
            )]})));
        }
    };
    // DRF 3.18: LIST_SERIALIZER_ERRORS_AS_DICT = True → {index: detail} only for faulty entries
    let mut ret = Vec::new();
    let mut errors = Map::new();
    for (index, item) in items.iter().enumerate() {
        let r = if is_trigger { validate_trigger(db, ctx, item, None).await } else { validate_action(db, ctx, item).await };
        match r {
            Ok(a) => ret.push(a),
            Err(VErr::Detail(d)) => {
                errors.insert(index.to_string(), d);
            }
            Err(e) => return Err(e),
        }
    }
    if !errors.is_empty() {
        return Err(VErr::Detail(Value::Object(errors)));
    }
    Ok(Some(ret))
}

#[derive(Debug)]
pub struct WorkflowInput {
    pub attrs: Attrs,
    pub triggers: Option<Vec<Attrs>>,
    pub actions: Option<Vec<Attrs>>,
}

/// `WorkflowSerializer.run_validation`. `instance`: id on update.
pub async fn validate_workflow(db: &mut PgConnection, ctx: Ctx, data: &Value, instance: Option<i64>) -> Result<WorkflowInput, VErr> {
    let obj = drf::expect_object(data)?;
    let mut attrs = Map::new();
    let mut errors = Map::new();
    for f in WORKFLOW_FIELDS {
        match drf::run_field(db, ctx, &f.spec, obj.get(f.spec.name)).await {
            Ok(Out::Val(v)) => {
                if f.spec.name == "name"
                    && let Value::String(n) = &v
                    && name_taken(db, n, instance).await?
                {
                    errors.insert("name".into(), msg("workflow with this name already exists."));
                    continue;
                }
                attrs.insert(f.spec.name.into(), v);
            }
            Ok(Out::Skip) => {}
            Err(VErr::Detail(d)) => {
                errors.insert(f.spec.name.into(), d);
            }
            Err(e) => return Err(e),
        }
    }
    let mut triggers = None;
    let mut actions = None;
    match validate_list(db, ctx, obj.get("triggers"), true).await {
        Ok(t) => triggers = t,
        Err(VErr::Detail(d)) => {
            errors.insert("triggers".into(), d);
        }
        Err(e) => return Err(e),
    }
    match validate_list(db, ctx, obj.get("actions"), false).await {
        Ok(a) => actions = a,
        Err(VErr::Detail(d)) => {
            errors.insert("actions".into(), d);
        }
        Err(e) => return Err(e),
    }
    if !errors.is_empty() {
        return Err(VErr::Detail(Value::Object(errors)));
    }
    let input = WorkflowInput { attrs, triggers, actions };
    workflow_validate(db, &input, instance).await.map_err(wrap_validate_err)?;
    Ok(input)
}

async fn name_taken(db: &mut PgConnection, name: &str, instance: Option<i64>) -> Result<bool, VErr> {
    let mut q = Query::select();
    q.expr(Expr::val(1)).from(Alias::new(T_WORKFLOW)).and_where(Expr::col(Alias::new("name")).eq(name)).limit(1);
    if let Some(id) = instance {
        q.and_where(Expr::col(Alias::new("id")).ne(id));
    }
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    Ok(sqlx::query_with(&sql, values).fetch_optional(&mut *db).await?.is_some())
}

/// Is there a child row of the workflow with `type = ty` (or `!= ty`)?
async fn child_type_exists(
    db: &mut PgConnection,
    table: &'static str,
    through: &'static str,
    other: &'static str,
    wf: i64,
    ty: i64,
    negate: bool,
) -> Result<bool, VErr> {
    let cond = if negate {
        Expr::col((Alias::new(table), Alias::new("type"))).ne(ty)
    } else {
        Expr::col((Alias::new(table), Alias::new("type"))).eq(ty)
    };
    let (sql, values) = Query::select()
        .expr(Expr::val(1))
        .from(Alias::new(table))
        .inner_join(Alias::new(through), Expr::col((Alias::new(table), Alias::new("id"))).equals((Alias::new(through), Alias::new(other))))
        .and_where(Expr::col((Alias::new(through), Alias::new("workflow_id"))).eq(wf))
        .and_where(cond)
        .limit(1)
        .build_sqlx(PostgresQueryBuilder);
    Ok(sqlx::query_with(&sql, values).fetch_optional(&mut *db).await?.is_some())
}

async fn workflow_validate(db: &mut PgConnection, input: &WorkflowInput, instance: Option<i64>) -> Result<(), VErr> {
    let ty_of = |a: &Attrs| a.get("type").and_then(Value::as_i64);
    let (has_remote_ocr, has_ai) = match &input.actions {
        Some(acts) => (acts.iter().any(|a| ty_of(a) == Some(ACTION_REMOTE_OCR)), acts.iter().any(|a| ty_of(a) == Some(ACTION_AI))),
        None => match instance {
            Some(wf) => (
                child_type_exists(db, T_ACTION, T_WF_ACTIONS, "workflowaction_id", wf, ACTION_REMOTE_OCR, false).await?,
                child_type_exists(db, T_ACTION, T_WF_ACTIONS, "workflowaction_id", wf, ACTION_AI, false).await?,
            ),
            None => (false, false),
        },
    };
    let (has_consumption, has_non_consumption) = match &input.triggers {
        Some(ts) => (ts.iter().any(|t| ty_of(t) == Some(TYPE_CONSUMPTION)), ts.iter().any(|t| ty_of(t) != Some(TYPE_CONSUMPTION))),
        None => match instance {
            Some(wf) => (
                child_type_exists(db, T_TRIGGER, T_WF_TRIGGERS, "workflowtrigger_id", wf, TYPE_CONSUMPTION, false).await?,
                child_type_exists(db, T_TRIGGER, T_WF_TRIGGERS, "workflowtrigger_id", wf, TYPE_CONSUMPTION, true).await?,
            ),
            None => (false, false),
        },
    };
    if has_remote_ocr && !has_consumption {
        return Err(non_field("Remote OCR actions require a consumption started trigger"));
    }
    if has_ai && !has_non_consumption {
        return Err(non_field("Apply AI suggestions actions require a trigger other than consumption started"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert!(url_validator("https://examplewithouttld:3000/path").is_ok());
        assert_eq!(url_validator("file:///etc/passwd/path").unwrap_err(), "URI scheme 'file' is not allowed. Allowed schemes: http, https");
        assert_eq!(url_validator("example.com").unwrap_err(), "Unable to parse URI example.com, missing scheme");
        assert_eq!(url_validator("http:").unwrap_err(), "Unable to parse URI http:, missing net location or path");
        assert!(url_validator("http:foo").is_ok());
        assert_eq!(url_validator("http://[::1/x").unwrap_err(), "Unable to parse URI http://[::1/x");
    }

    #[test]
    fn action_rules() {
        let mut a: Attrs = serde_json::from_value(json!({"type": 5, "passwords": ["", "x"]})).unwrap();
        match action_validate(&mut a) {
            Err(VErr::Detail(d)) => assert_eq!(d, json!({"non_field_errors": ["Passwords are required for password removal actions"]})),
            _ => panic!(),
        }
        let mut a: Attrs = serde_json::from_value(json!({"type": 8, "ai_suggestion_fields": ["x"]})).unwrap();
        match action_validate(&mut a) {
            Err(VErr::Detail(d)) => assert_eq!(
                d["non_field_errors"][0],
                json!(
                    "At least one valid field is required for apply AI suggestions actions, options are: ['correspondent', 'created', 'document_type', 'storage_path', 'tags', 'title']"
                )
            ),
            _ => panic!(),
        }
        let mut a: Attrs = serde_json::from_value(json!({"assign_title": "", "assign_custom_fields_values": {"1": "", "2": 0}})).unwrap();
        assert!(action_validate(&mut a).is_ok());
        assert_eq!(a["assign_title"], Value::Null);
        assert_eq!(a["assign_custom_fields_values"], json!({"1": null, "2": 0}));
    }
}
