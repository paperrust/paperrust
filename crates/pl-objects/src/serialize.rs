//! Output of CorrespondentSerializer, DocumentTypeSerializer, StoragePathSerializer and TagSerializer.
//!
//! upstream: src/documents/serialisers.py@v3.4.2 (MatchingModelSerializer, OwnedObjectSerializer,
//!           CorrespondentSerializer, DocumentTypeSerializer, TagSerializer, StoragePathSerializer)

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};

use pl_authz::User;
use pl_authz::owned::ObjectGrantees as Grantees;

use crate::model::{Kind, ObjectRow};
use crate::text::{slugify, text_color};

/// How `user_can_change` is resolved (see `OwnedObjectSerializer.get_user_can_change`).
pub enum ChangeSource<'a> {
    /// List action: page-level batch from `BulkPermissionMixin`; objects missing from it → false.
    Batch(&'a HashMap<i64, Grantees>),
    /// Other actions: per-object guardian check, precomputed by the caller.
    PerObject(&'a HashSet<i64>),
}

pub struct SerCtx<'a> {
    pub user: &'a User,
    pub full_perms: bool,
    pub change: ChangeSource<'a>,
    /// Grantees for `permissions` (full_perms); must contain every serialized pk.
    pub grantees: &'a HashMap<i64, Grantees>,
    /// Tag children by parent id (already filtered and ordered).
    pub children: Option<&'a HashMap<i64, Vec<ObjectRow>>>,
}

fn user_can_change(ctx: &SerCtx, row: &ObjectRow) -> bool {
    match &ctx.change {
        ChangeSource::Batch(b) => pl_authz::owned::user_can_change(Some(ctx.user), row.owner_id, b.get(&row.id)),
        ChangeSource::PerObject(set) => {
            if row.owner_id.is_none() || row.owner_id == Some(ctx.user.id) {
                return true;
            }
            if ctx.user.is_active && ctx.user.is_superuser {
                return true;
            }
            set.contains(&row.id)
        }
    }
}

/// Serialize one object (and, for tags, its children recursively).
pub fn serialize(kind: Kind, row: &ObjectRow, ctx: &SerCtx) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(row.id));
    m.insert("slug".into(), json!(slugify(&row.name)));
    m.insert("name".into(), json!(row.name));
    if kind == Kind::StoragePath {
        m.insert("path".into(), json!(row.path.clone().unwrap_or_default()));
    }
    if let (Kind::Tag, Some(t)) = (kind, &row.tag) {
        m.insert("color".into(), json!(t.color));
        m.insert("text_color".into(), json!(text_color(&t.color)));
    }
    m.insert("match".into(), json!(row.r#match));
    m.insert("matching_algorithm".into(), json!(row.matching_algorithm));
    m.insert("is_insensitive".into(), json!(row.is_insensitive));
    if let (Kind::Tag, Some(t)) = (kind, &row.tag) {
        m.insert("is_inbox_tag".into(), json!(t.is_inbox_tag));
    }
    if let Some(c) = row.document_count {
        m.insert("document_count".into(), json!(c));
    }
    if kind == Kind::Correspondent
        && let Some(lc) = &row.last_correspondence
    {
        m.insert(
            "last_correspondence".into(),
            match lc {
                Some(d) => json!(d.format("%Y-%m-%d").to_string()),
                None => Value::Null,
            },
        );
    }
    m.insert("owner".into(), json!(row.owner_id));
    if ctx.full_perms {
        let p = ctx.grantees.get(&row.id).cloned().unwrap_or_default();
        m.insert("permissions".into(), p.to_json());
    } else {
        m.insert("user_can_change".into(), json!(user_can_change(ctx, row)));
    }
    if let (Kind::Tag, Some(t)) = (kind, &row.tag) {
        m.insert("parent".into(), json!(t.tn_parent_id));
        let children: Vec<Value> = ctx
            .children
            .and_then(|c| c.get(&row.id))
            .map(|kids| kids.iter().map(|k| serialize(kind, k, ctx)).collect())
            .unwrap_or_default();
        m.insert("children".into(), Value::Array(children));
    }
    Value::Object(m)
}

/// All pks reachable from `rows` through `children` (for batch permission lookups).
pub fn all_pks(rows: &[ObjectRow], children: Option<&HashMap<i64, Vec<ObjectRow>>>) -> Vec<i64> {
    let mut out = vec![];
    let mut seen = HashSet::new();
    let mut stack: Vec<&ObjectRow> = rows.iter().collect();
    while let Some(r) = stack.pop() {
        if !seen.insert(r.id) {
            continue;
        }
        out.push(r.id);
        if let Some(kids) = children.and_then(|c| c.get(&r.id)) {
            stack.extend(kids.iter());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TagExtra;

    fn user() -> User {
        User {
            id: 2,
            username: "u".into(),
            password: String::new(),
            first_name: String::new(),
            last_name: String::new(),
            email: String::new(),
            is_active: true,
            is_staff: false,
            is_superuser: false,
            last_login: None,
            date_joined: chrono::Utc::now(),
            groups: vec![7],
            permissions: Default::default(),
        }
    }

    #[test]
    fn correspondent_field_order() {
        let row = ObjectRow {
            id: 1,
            name: "Bank AG".into(),
            matching_algorithm: 1,
            is_insensitive: true,
            document_count: Some(3),
            last_correspondence: Some(None),
            owner_id: Some(5),
            ..Default::default()
        };
        let g = HashMap::new();
        let batch: HashMap<i64, Grantees> =
            [(1, Grantees { change: pl_authz::owned::Grantees { users: vec![], groups: vec![7] }, ..Default::default() })].into();
        let u = user();
        let ctx = SerCtx { user: &u, full_perms: false, change: ChangeSource::Batch(&batch), grantees: &g, children: None };
        let v = serialize(Kind::Correspondent, &row, &ctx);
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "id",
                "slug",
                "name",
                "match",
                "matching_algorithm",
                "is_insensitive",
                "document_count",
                "last_correspondence",
                "owner",
                "user_can_change"
            ]
        );
        assert_eq!(v["slug"], "bank-ag");
        assert_eq!(v["user_can_change"], true);
        assert_eq!(v["last_correspondence"], Value::Null);
    }

    #[test]
    fn tag_with_children_and_full_perms() {
        let tag = |id, parent| ObjectRow {
            id,
            name: format!("t{id}"),
            tag: Some(TagExtra { color: "#000000".into(), tn_parent_id: parent, ..Default::default() }),
            ..Default::default()
        };
        let children: HashMap<i64, Vec<ObjectRow>> = [(1, vec![tag(2, Some(1))])].into();
        let g: HashMap<i64, Grantees> = [
            (1, Grantees::default()),
            (2, Grantees { view: pl_authz::owned::Grantees { users: vec![3], groups: vec![] }, ..Default::default() }),
        ]
        .into();
        let u = user();
        let empty = HashSet::new();
        let ctx = SerCtx { user: &u, full_perms: true, change: ChangeSource::PerObject(&empty), grantees: &g, children: Some(&children) };
        let v = serialize(Kind::Tag, &tag(1, None), &ctx);
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "id",
                "slug",
                "name",
                "color",
                "text_color",
                "match",
                "matching_algorithm",
                "is_insensitive",
                "is_inbox_tag",
                "owner",
                "permissions",
                "parent",
                "children"
            ]
        );
        assert_eq!(v["children"][0]["id"], 2);
        assert_eq!(v["children"][0]["permissions"]["view"]["users"], json!([3]));
        assert_eq!(v["text_color"], "#ffffff");
    }
}
