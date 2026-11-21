//! Tests for DocumentsOrderingFilter + OrderingFilter (upstream
//! test_api_documents.py::test_document_custom_fields_sorting*).

mod common;

use common::*;
use pl_docfilter::{CustomFieldDef, FilterError};
use serde_json::json;

#[test]
fn default_ordering() {
    let db = MockDb::default();
    let expected = r#"ORDER BY "documents_document"."created" DESC, "documents_document"."id" DESC"#;
    assert_eq!(order_sql_with(None, &db).unwrap(), expected);
    assert_eq!(order_sql_with(Some(""), &db).unwrap(), expected);
    assert_eq!(order_sql_with(Some("bogus,-nope"), &db).unwrap(), expected);
    assert_eq!(order_sql_with(Some("--title"), &db).unwrap(), expected);
}

#[test]
fn plain_ordering_fields() {
    let db = MockDb::default();
    assert_eq!(
        order_sql_with(Some(" title , -bogus,-added"), &db).unwrap(),
        r#"ORDER BY "documents_document"."title" ASC, "documents_document"."added" DESC"#
    );
    assert_eq!(order_sql_with(Some("-owner"), &db).unwrap(), r#"ORDER BY "documents_document"."owner_id" DESC"#);
    assert_eq!(
        order_sql_with(Some("correspondent__name"), &db).unwrap(),
        r#"LEFT JOIN "documents_correspondent" AS "oc" ON "oc"."id" = "documents_document"."correspondent_id" ORDER BY "oc"."name" ASC"#
    );
    // Phase 6: LEFT JOIN like Django, one join per table even with a duplicate term
    assert_eq!(
        order_sql_with(Some("-document_type__name,storage_path__name,document_type__name"), &db).unwrap(),
        r#"LEFT JOIN "documents_documenttype" AS "odt" ON "odt"."id" = "documents_document"."document_type_id" LEFT JOIN "documents_storagepath" AS "osp" ON "osp"."id" = "documents_document"."storage_path_id" ORDER BY "odt"."name" DESC, "osp"."name" ASC, "odt"."name" ASC"#
    );
    assert_eq!(
        order_sql_with(Some("-num_notes"), &db).unwrap(),
        r#"LEFT JOIN (SELECT "nn"."document_id", COUNT(*) AS "n" FROM "documents_note" AS "nn" WHERE "nn"."deleted_at" IS NULL GROUP BY "nn"."document_id") AS "onn" ON "onn"."document_id" = "documents_document"."id" ORDER BY COALESCE("onn"."n", 0) DESC"#
    );
    for f in ["id", "created", "modified", "archive_serial_number", "page_count"] {
        assert_eq!(order_sql_with(Some(f), &db).unwrap(), format!(r#"ORDER BY "documents_document"."{f}" ASC"#));
    }
}

fn db_with(data_type: &str, extra: Option<serde_json::Value>) -> MockDb {
    MockDb { fields: vec![CustomFieldDef { id: 5, name: "cf".into(), data_type: data_type.into(), extra_data: extra }], links: vec![] }
}

#[test]
fn custom_field_ordering() {
    let db = db_with("string", None);
    assert_eq!(
        order_sql_with(Some("-custom_field_5"), &db).unwrap(),
        r#"ORDER BY EXISTS(SELECT 1 FROM "documents_customfieldinstance" AS "cfh" WHERE "cfh"."document_id" = "documents_document"."id" AND "cfh"."field_id" = 5 AND "cfh"."deleted_at" IS NULL) DESC, (SELECT "cfo"."value_text" FROM "documents_customfieldinstance" AS "cfo" WHERE "cfo"."document_id" = "documents_document"."id" AND "cfo"."field_id" = 5 AND "cfo"."deleted_at" IS NULL ORDER BY "cfo"."created" ASC LIMIT 1) DESC"#
    );
    for (t, c) in [
        ("integer", "value_int"),
        ("float", "value_float"),
        ("date", "value_date"),
        ("monetary", "value_monetary_amount"),
        ("documentlink", "value_document_ids"),
        ("url", "value_url"),
        ("boolean", "value_bool"),
        ("longtext", "value_text"),
    ] {
        let sql = order_sql_with(Some("custom_field_5"), &db_with(t, None)).unwrap();
        assert!(sql.contains(&format!(r#"SELECT "cfo"."{c}""#)), "{t}: {sql}");
        assert!(sql.ends_with(") ASC"), "{sql}");
    }
}

#[test]
fn custom_field_select_ordering_by_label_index() {
    let db = db_with(
        "select",
        Some(json!({"select_options": [
            {"label": "Option 3", "id": "ghi-789"},
            {"label": "Option 1", "id": "abc-123"},
            {"label": "Option 2", "id": "def-456"}
        ]})),
    );
    let sql = order_sql_with(Some("custom_field_5"), &db).unwrap();
    assert!(sql.contains(r#"CASE WHEN ("cfs"."field_id" = 5 AND "cfs"."value_select" = 'abc-123') THEN 0 WHEN ("cfs"."field_id" = 5 AND "cfs"."value_select" = 'def-456') THEN 1 WHEN ("cfs"."field_id" = 5 AND "cfs"."value_select" = 'ghi-789') THEN 2 WHEN ("cfs"."field_id" = 5 AND "cfs"."value_select" IS NULL) THEN 3 ELSE 0 END"#), "{sql}");
    assert!(sql.starts_with("ORDER BY EXISTS("), "{sql}");
    // extra_data None → AttributeError in upstream
    assert!(matches!(order_sql_with(Some("custom_field_5"), &db_with("select", None)), Err(FilterError::Internal(_))));
}

#[test]
fn custom_field_ordering_errors() {
    let db = db_with("string", None);
    assert_eq!(validation(order_sql_with(Some("custom_field_999"), &db)), json!({"custom_field_999": ["Custom field not found"]}));
    // str(int("007")) == "7"
    assert_eq!(validation(order_sql_with(Some("-custom_field_007"), &db)), json!({"custom_field_7": ["Custom field not found"]}));
    // upstream ValueError/FieldError → 500; PaperRust 400 with field message (B-305/D-039)
    for bad in ["custom_field_abc", "custom_field_", "custom_field_5,title", "title,custom_field_5", "xcustom_field_5"] {
        assert_eq!(validation(order_sql_with(Some(bad), &db)), json!({"ordering": [pl_docfilter::ordering::ORDERING_CF_INVALID]}), "{bad}");
    }
    // unknown data type → ValueError
    assert!(matches!(order_sql_with(Some("custom_field_5"), &db_with("foo", None)), Err(FilterError::Internal(_))));
}

/// B-305/D-039: the AST path too (`parse_ordering`, document list) answers 400 instead of 500.
#[test]
fn parse_ordering_bad_custom_field_is_validation() {
    let db = db_with("string", None);
    for bad in ["custom_field_abc", "-custom_field_x", "custom_field_5,title"] {
        let r = futures::executor::block_on(pl_docfilter::ordering::parse_ordering(Some(bad), &db));
        match r {
            Err(FilterError::Validation(v)) => assert_eq!(v, json!({"ordering": [pl_docfilter::ordering::ORDERING_CF_INVALID]}), "{bad}"),
            other => panic!("{bad}: {other:?}"),
        }
    }
}
