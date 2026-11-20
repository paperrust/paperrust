//! Export/import of tags without stored django-treenode fields (migration
//! `pl_objects.0001_paperrust_tag_adjacency`, docs/DATAMODEL-V2.md §1.9/§3.2).
//!
//! * Export: the manifest stays in the paperless-ngx v3.4.2 format (field list from
//!   `baseline/export-descriptor.json`). If the columns are missing in the DB, the query reads placeholders
//!   and the values come from [`pl_objects::treenode::compute`] (from `tn_parent_id`, priority 0) –
//!   paperless-ngx 3.4.2 does not import a manifest without these fields with a valid tree.
//! * Import: exports of paperless-ngx 3.x contain the fields; they are skipped if the
//!   columns are missing (like upstream 8aa99a381 `document_importer._deserialize_record`). `tn_parent`
//!   remains authoritative.

use std::collections::HashMap;

use serde_json::{Map, Value};
use sqlx::PgConnection;

use pl_objects::treenode::{Node, TreeFields, compute};

pub const TAG_MODEL: &str = "documents.tag";

/// Derived django-treenode fields (= column names) of the v3.4.2 tag.
pub const DERIVED: [&str; 13] = [
    "tn_ancestors_pks",
    "tn_ancestors_count",
    "tn_children_pks",
    "tn_children_count",
    "tn_depth",
    "tn_descendants_pks",
    "tn_descendants_count",
    "tn_index",
    "tn_level",
    "tn_priority",
    "tn_order",
    "tn_siblings_pks",
    "tn_siblings_count",
];

pub fn is_derived(model_label: &str, field: &str) -> bool {
    model_label == TAG_MODEL && DERIVED.contains(&field)
}

/// Does `documents_tag` still have the treenode columns (database without the PaperRust migration)?
pub async fn has_columns(conn: &mut PgConnection) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
         WHERE table_schema = current_schema() AND table_name = 'documents_tag' AND column_name = 'tn_ancestors_pks')",
    )
    .fetch_one(&mut *conn)
    .await
}

/// Export query of the tags without the columns: `"documents_tag"."tn_x" AS "tn_x"` → placeholders
/// of the same type (text `''`, number `0`), the field order of the manifest stays.
pub fn placeholder_sql(sql: &str) -> String {
    let mut out = sql.to_string();
    for c in DERIVED {
        let lit = if c.ends_with("_pks") { "''::text" } else { "0::integer" };
        out = out.replace(&format!("\"documents_tag\".\"{c}\" AS \"{c}\""), &format!("{lit} AS \"{c}\""));
    }
    out
}

/// Values per tag id from the adjacency list (one query over all tags).
pub async fn computed(conn: &mut PgConnection) -> Result<HashMap<i64, TreeFields>, sqlx::Error> {
    let rows: Vec<(i32, Option<i32>, String)> =
        sqlx::query_as("SELECT id, tn_parent_id, name FROM documents_tag").fetch_all(&mut *conn).await?;
    let nodes: Vec<Node> = rows
        .into_iter()
        .map(|(id, p, name)| Node { pk: i64::from(id), parent: p.map(i64::from), priority: 0, name })
        .collect();
    Ok(compute(&nodes))
}

/// Sets the computed fields into an export record (`fields` keeps the order).
pub fn fill(rec: &mut Map<String, Value>, tree: &HashMap<i64, TreeFields>) {
    let Some(pk) = rec.get("pk").and_then(Value::as_i64) else { return };
    let Some(t) = tree.get(&pk) else { return };
    let Some(Value::Object(f)) = rec.get_mut("fields") else { return };
    let mut set = |k: &str, v: Value| {
        if f.contains_key(k) {
            f.insert(k.to_string(), v);
        }
    };
    set("tn_ancestors_pks", Value::from(t.tn_ancestors_pks.clone()));
    set("tn_ancestors_count", Value::from(t.tn_ancestors_count));
    set("tn_children_pks", Value::from(t.tn_children_pks.clone()));
    set("tn_children_count", Value::from(t.tn_children_count));
    set("tn_depth", Value::from(t.tn_depth));
    set("tn_descendants_pks", Value::from(t.tn_descendants_pks.clone()));
    set("tn_descendants_count", Value::from(t.tn_descendants_count));
    set("tn_index", Value::from(t.tn_index));
    set("tn_level", Value::from(t.tn_level));
    set("tn_priority", Value::from(0));
    set("tn_order", Value::from(t.tn_order));
    set("tn_siblings_pks", Value::from(t.tn_siblings_pks.clone()));
    set("tn_siblings_count", Value::from(t.tn_siblings_count));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn descriptor_sql_gets_placeholders_in_place() {
        let d = crate::export::descriptor::get();
        let key = d.keys.iter().find(|k| k.model == TAG_MODEL).expect("tags key");
        let sql = placeholder_sql(&key.sql);
        for c in DERIVED {
            assert!(!sql.contains(&format!("\"documents_tag\".\"{c}\"")), "{c} still read from the table");
            assert!(sql.contains(&format!("AS \"{c}\"")), "{c} placeholder missing");
        }
        assert!(sql.contains("\"documents_tag\".\"tn_parent_id\" AS \"tn_parent_id\""));
        // every derived field is in the descriptor model (manifest keeps the v3.4.2 field list)
        let m = &d.models[TAG_MODEL];
        for c in DERIVED {
            assert!(m.fields.iter().any(|f| f.name == c), "{c} not in descriptor");
        }
        assert!(is_derived(TAG_MODEL, "tn_order") && !is_derived(TAG_MODEL, "tn_parent") && !is_derived("documents.note", "tn_order"));
    }

    /// Way back to paperless-ngx 3.4.2: the tag records of the export are, after the migration
    /// (values computed), **identical** to those before the migration (written by django-treenode).
    /// Private copy `pl_cli_tagtree_export_test` of `paperless_snapshot`, is dropped.
    #[tokio::test]
    #[ignore = "needs the reference PostgreSQL (127.0.0.1:54329) with the template paperless_snapshot"]
    async fn export_records_identical_before_and_after_migration() {
        use sqlx::Connection;
        const DB: &str = "pl_cli_tagtree_export_test";
        let admin_url = "postgres://paperless:paperless@127.0.0.1:54329/postgres";
        let mut admin = PgConnection::connect(admin_url).await.expect("admin");
        sqlx::raw_sql(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&mut admin).await.expect("drop");
        sqlx::raw_sql(&format!("CREATE DATABASE {DB} TEMPLATE paperless_snapshot")).execute(&mut admin).await.expect("create");
        let mut c = PgConnection::connect(&format!("postgres://paperless:paperless@127.0.0.1:54329/{DB}")).await.expect("db");

        let d = crate::export::descriptor::get();
        let key = d.keys.iter().find(|k| k.model == TAG_MODEL).expect("tags key");
        let model = &d.models[TAG_MODEL];
        // simple protocol (text), like the exporter (FETCH via a cursor)
        async fn records(c: &mut PgConnection, model: &crate::export::descriptor::Model, sql: &str) -> Vec<Map<String, Value>> {
            let rows = sqlx::raw_sql(sql).fetch_all(&mut *c).await.expect("query");
            let mut out = Vec::new();
            for r in &rows {
                out.push(crate::export::exporter::build_record(c, model, r).await.expect("record"));
            }
            out
        }
        assert!(has_columns(&mut c).await.unwrap());
        let sql = crate::export::exporter::inline_params(&key.sql, &key.params);
        let before = records(&mut c, model, &sql).await;
        assert!(before.iter().any(|r| r["fields"]["tn_parent"] != Value::Null), "snapshot without nested tags");

        sqlx::raw_sql(pl_objects::migrations::TAG_ADJACENCY.1).execute(&mut c).await.expect("migration");
        assert!(!has_columns(&mut c).await.unwrap());
        let tree = computed(&mut c).await.unwrap();
        let mut after = records(&mut c, model, &placeholder_sql(&sql)).await;
        for r in &mut after {
            fill(r, &tree);
        }
        assert_eq!(after.len(), before.len());
        for (a, b) in after.iter().zip(&before) {
            assert_eq!(Value::Object(a.clone()), Value::Object(b.clone()));
            let ka: Vec<&String> = a["fields"].as_object().map(|m| m.keys().collect()).unwrap_or_default();
            let kb: Vec<&String> = b["fields"].as_object().map(|m| m.keys().collect()).unwrap_or_default();
            assert_eq!(ka, kb, "field order of tag {}", a["pk"]);
        }
        c.close().await.ok();
        sqlx::raw_sql(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&mut admin).await.expect("drop");
    }

    #[test]
    fn fill_keeps_field_order_and_values() {
        let nodes = vec![
            Node { pk: 1, parent: None, priority: 0, name: "Finance".into() },
            Node { pk: 2, parent: Some(1), priority: 0, name: "Taxes".into() },
        ];
        let tree = compute(&nodes);
        let mut rec = json!({"model": TAG_MODEL, "pk": 2, "fields": {
            "tn_ancestors_pks": "", "tn_ancestors_count": 0, "tn_children_pks": "", "tn_children_count": 0, "tn_depth": 0,
            "tn_descendants_pks": "", "tn_descendants_count": 0, "tn_index": 0, "tn_level": 0, "tn_parent": 1,
            "tn_priority": 0, "tn_order": 0, "tn_siblings_pks": "", "tn_siblings_count": 0, "name": "Taxes"}})
        .as_object()
        .cloned()
        .unwrap_or_default();
        fill(&mut rec, &tree);
        let f = rec["fields"].as_object().cloned().unwrap_or_default();
        assert_eq!(f["tn_ancestors_pks"], json!("1"));
        assert_eq!(f["tn_level"], json!(2));
        assert_eq!(f["tn_order"], json!(1));
        let keys: Vec<&String> = f.keys().collect();
        assert_eq!(keys[9], "tn_parent");
        assert_eq!(keys.last().map(|k| k.as_str()), Some("name"));
    }
}
