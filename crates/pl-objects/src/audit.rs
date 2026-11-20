//! django-auditlog entries for Correspondent, Tag and DocumentType (local until pl-core offers an
//! audit writer, see CORE-REQUEST).
//!
//! upstream: django-auditlog 3.4.1 (receivers.log_create/log_update/log_delete, diff.model_instance_diff,
//!           diff.get_field_value, models.LogEntryManager.log_create, middleware.AuditlogMiddleware),
//!           src/documents/models.py@v3.4.2 (auditlog.register(...)), settings AUDIT_LOG_ENABLED;
//!           Tag fields as in upstream 8aa99a381 (#14612, Tag without TreeNodeModel)

use serde_json::{Map, Value};
use sqlx::{Postgres, Transaction};

use pl_core::ApiError;

use crate::drf::AuditMeta;
use crate::model::{Kind, ObjectRow};
use crate::perms::ctype_sql;

pub const CREATE: i16 = 0;
pub const UPDATE: i16 = 1;
pub const DELETE: i16 = 2;

/// FK value: `smart_str(obj.<fk>_id, strings_only=True)` keeps Python `None` (not `"None"`).
fn fk(v: Option<i64>) -> Option<String> {
    v.map(|x| x.to_string())
}

fn s_bool(b: bool) -> String {
    if b { "True".into() } else { "False".into() }
}

/// `smart_str(get_field_value(obj, field))` for all concrete fields (`_meta.fields`), in definition order.
pub fn concrete_values(kind: Kind, r: &ObjectRow) -> Vec<(&'static str, Option<String>)> {
    let s = |x: String| Some(x);
    let mut v: Vec<(&'static str, Option<String>)> = vec![
        ("id", s(r.id.to_string())),
        ("owner", fk(r.owner_id)),
        ("name", s(r.name.clone())),
        ("match", s(r.r#match.clone())),
        ("matching_algorithm", s(r.matching_algorithm.to_string())),
        ("is_insensitive", s(s_bool(r.is_insensitive))),
    ];
    if let (Kind::Tag, Some(t)) = (kind, &r.tag) {
        // Tag fields after upstream #14612 (no django-treenode bookkeeping): tn_parent, color, is_inbox_tag
        v.extend([("tn_parent", fk(t.tn_parent_id)), ("color", s(t.color.clone())), ("is_inbox_tag", s(s_bool(t.is_inbox_tag)))]);
    }
    if kind == Kind::StoragePath {
        v.push(("path", Some(r.path.clone().unwrap_or_default())));
    }
    v
}

/// Reverse one-to-many relations with an accessor equal to the field name (`get_fields()`):
/// their value is the related manager, `smart_str` → `"<app>.<Model>.None"`.
fn reverse_values(kind: Kind) -> Vec<(&'static str, Option<String>)> {
    match kind {
        Kind::Correspondent | Kind::DocumentType | Kind::StoragePath => vec![("documents", Some("documents.Document.None".into()))],
        Kind::Tag => vec![("tn_children", Some("documents.Tag.None".into()))],
    }
}

/// `model_instance_diff(old, new)` without JSON mode.
pub fn diff(kind: Kind, old: Option<&ObjectRow>, new: Option<&ObjectRow>) -> Option<Map<String, Value>> {
    let mut out = Map::new();
    let with_reverse = old.is_none() || new.is_none();
    let vals = |r: Option<&ObjectRow>| -> Vec<(&'static str, Option<String>)> {
        match r {
            Some(r) => {
                let mut v = concrete_values(kind, r);
                if with_reverse {
                    v.extend(reverse_values(kind));
                }
                v
            }
            None => {
                let template = old.or(new).map(|x| {
                    let mut v = concrete_values(kind, x);
                    v.extend(reverse_values(kind));
                    v
                });
                // get_field_value(None, field) → default → smart_str(None) == "None"
                template.unwrap_or_default().into_iter().map(|(k, _)| (k, Some("None".to_string()))).collect()
            }
        }
    };
    let a = vals(old);
    let b = vals(new);
    for ((k, ov), (_, nv)) in a.iter().zip(b.iter()) {
        if ov != nv {
            let st = |x: &Option<String>| Value::String(x.clone().unwrap_or_else(|| "None".into()));
            out.insert((*k).to_string(), Value::Array(vec![st(ov), st(nv)]));
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Write one `auditlog_logentry` row if `changes` is non-empty (caller checks `AUDIT_LOG_ENABLED`).
pub async fn log(
    tx: &mut Transaction<'_, Postgres>,
    ctx: &AuditMeta,
    kind: Kind,
    action: i16,
    row: &ObjectRow,
    changes: Option<Map<String, Value>>,
) -> Result<(), ApiError> {
    if !kind.audited() {
        return Ok(());
    }
    let Some(changes) = changes else { return Ok(()) };
    let sql = format!(
        "INSERT INTO auditlog_logentry (content_type_id, object_pk, object_id, object_repr, serialized_data, action, \
         changes_text, changes, actor_id, cid, remote_addr, remote_port, timestamp, additional_data, actor_email) \
         VALUES ({}, $1, $2, $3, NULL, $4, '', $5, $6, $7, $8::inet, $9, clock_timestamp(), NULL, $10)",
        ctype_sql(kind)
    );
    sqlx::query(&sql)
        .bind(row.id.to_string())
        .bind(row.id)
        .bind(&row.name)
        .bind(action)
        .bind(Value::Object(changes))
        .bind(ctx.actor_id)
        .bind(&ctx.correlation_id)
        .bind(&ctx.remote_addr)
        .bind(ctx.remote_port)
        .bind(&ctx.actor_email)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_diff_contains_reverse_and_skips_none() {
        let r = ObjectRow { id: 3, name: "c".into(), matching_algorithm: 1, is_insensitive: true, ..Default::default() };
        let d = diff(Kind::Correspondent, None, Some(&r)).unwrap();
        assert_eq!(d["id"], serde_json::json!(["None", "3"]));
        assert_eq!(d["match"], serde_json::json!(["None", ""]));
        assert_eq!(d["documents"], serde_json::json!(["None", "documents.Document.None"]));
        // FK None vs. the "None" default of the missing instance → listed (upstream quirk)
        assert_eq!(d["owner"], serde_json::json!(["None", "None"]));
    }

    #[test]
    fn update_diff_only_changed_concrete() {
        let a = ObjectRow { id: 3, name: "c".into(), ..Default::default() };
        let mut b = a.clone();
        b.name = "d".into();
        b.owner_id = Some(1);
        let d = diff(Kind::Correspondent, Some(&a), Some(&b)).unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(d["owner"], serde_json::json!(["None", "1"]));
        assert!(diff(Kind::Correspondent, Some(&a), Some(&a)).is_none());
    }

    #[test]
    fn tag_fields_without_treenode_bookkeeping() {
        let t = ObjectRow {
            id: 5,
            name: "t".into(),
            tag: Some(crate::model::TagExtra { color: "#000000".into(), is_inbox_tag: false, tn_parent_id: Some(2) }),
            ..Default::default()
        };
        let keys: Vec<&str> = concrete_values(Kind::Tag, &t).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["id", "owner", "name", "match", "matching_algorithm", "is_insensitive", "tn_parent", "color", "is_inbox_tag"]);
        let d = diff(Kind::Tag, None, Some(&t)).unwrap();
        assert_eq!(d["tn_parent"], serde_json::json!(["None", "2"]));
        assert_eq!(d["tn_children"], serde_json::json!(["None", "documents.Tag.None"]));
        assert!(!d.contains_key("tn_ancestors_pks"));
    }
}
