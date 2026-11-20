//! Tests after upstream src/documents/tests/test_api_filter_by_custom_fields.py
//! (error paths and texts exact, SQL form of the atoms).

mod common;

use common::*;
use serde_json::{Value, json};

fn err(query_json: &str) -> Value {
    validation(filter_sql(&cfq(query_json)))
}

fn assert_err(query_json: &str, path: &[&str], keyword: &str) {
    let v = err(query_json);
    let (p, msg) = error_path(&v);
    let expected: Vec<String> = std::iter::once("custom_field_query".to_string())
        .chain(path.iter().map(|s| s.to_string()))
        .collect();
    assert_eq!(p, expected, "path for {query_json}: {v}");
    assert!(msg.contains(keyword), "message {msg:?} lacks {keyword:?}");
}

fn ends(sql: &str, suffix: &str) {
    assert!(sql.ends_with(suffix), "
{sql}
 does not end with
{suffix}");
}

fn has(sql: &str, part: &str) {
    assert!(sql.contains(part), "
{sql}
 does not contain
{part}");
}

fn ok(query_json: &str) -> String {
    filter_sql(&cfq(query_json)).unwrap()
}

#[test]
fn upstream_invalid_queries() {
    assert_eq!(err("not valid json"), json!({"custom_field_query": ["Value must be valid JSON."]}));
    assert_eq!(
        err("\"valid json but not valid expr\""),
        json!({"custom_field_query": ["Invalid custom field query expression"]})
    );
    assert_eq!(
        err(r#"["invalid name", "iexact", "foo"]"#),
        json!({"custom_field_query": {"0": ["'invalid name' is not a valid custom field."]}})
    );
    assert_eq!(
        err(r#"["integer_field", "iexact", "foo"]"#),
        json!({"custom_field_query": {"1": ["integer does not support query expr 'iexact'."]}})
    );
    assert_eq!(
        err(r#"["select_field", "exact", []]"#),
        json!({"custom_field_query": {"2": ["Not a valid string."]}})
    );
    assert_eq!(
        err(r#"["invalid op", ["integer_field", "gt", 0]]"#),
        json!({"custom_field_query": {"0": ["Invalid logical operator 'invalid op'"]}})
    );
    assert_eq!(
        err(r#"["AND", "not a list"]"#),
        json!({"custom_field_query": {"1": ["Invalid expression list. Must be nonempty."]}})
    );
    assert_eq!(
        err(r#"["AND", []]"#),
        json!({"custom_field_query": {"1": ["Invalid expression list. Must be nonempty."]}})
    );
    assert_err(r#"["integer_field", "foo__gt", 0]"#, &["1"], "does not support query expr");
    assert_err(r#"["date_field", "foo__gt", 0]"#, &["1"], "does not support query expr");
    assert_err(r#"["monetary_field", "exact", "USDnotanumber"]"#, &["2"], "valid number");
}

#[test]
fn query_too_deep() {
    let mut q = r#"["string_field", "exact", "paperless"]"#.to_string();
    for _ in 0..10 {
        q = format!(r#"["NOT", {q}]"#);
    }
    let v = err(&q);
    let (path, msg) = error_path(&v);
    let mut expected = vec!["custom_field_query".to_string()];
    expected.extend(std::iter::repeat_n("1".to_string(), 10));
    assert_eq!(path, expected);
    assert_eq!(msg, "Maximum nesting depth exceeded.");
    // 9 NOTs still work
    let mut q = r#"["string_field", "exact", "paperless"]"#.to_string();
    for _ in 0..9 {
        q = format!(r#"["NOT", {q}]"#);
    }
    ok(&q);
}

#[test]
fn query_too_many_atoms() {
    let atom = r#"["string_field", "exact", "paperless"]"#;
    let q = format!(r#"["AND", [{}]]"#, vec![atom; 21].join(", "));
    assert_err(&q, &["1", "20"], "Maximum number of query conditions exceeded");
    let q = format!(r#"["AND", [{}]]"#, vec![atom; 20].join(", "));
    ok(&q);
}

#[test]
fn nested_error_paths() {
    // AND → list "1" → index 1 → operator "1"
    assert_eq!(
        err(r#"["AND", [["string_field", "exact", "a"], ["integer_field", "bad", 1]]]"#),
        json!({"custom_field_query": {"1": {"1": {"1": ["integer does not support query expr 'bad'."]}}}})
    );
    // ListField child error
    assert_eq!(
        err(r#"["integer_field", "in", [1, "x"]]"#),
        json!({"custom_field_query": {"2": {"1": ["A valid integer is required."]}}})
    );
    assert_eq!(
        err(r#"["integer_field", "in", []]"#),
        json!({"custom_field_query": {"2": ["This list may not be empty."]}})
    );
    assert_eq!(
        err(r#"["float_field", "range", [1]]"#),
        json!({"custom_field_query": {"2": ["Ensure this field has at least 2 elements."]}})
    );
    assert_eq!(
        err(r#"["float_field", "range", [1, 2, 3]]"#),
        json!({"custom_field_query": {"2": ["Ensure this field has no more than 2 elements."]}})
    );
    assert_eq!(
        err(r#"["integer_field", "exact", 3000000000]"#),
        json!({"custom_field_query": {"2": ["Ensure this value is less than or equal to 2147483647."]}})
    );
    assert_eq!(
        err(r#"["integer_field", "exact", null]"#),
        json!({"custom_field_query": {"2": ["This field may not be null."]}})
    );
    assert_eq!(
        err(r#"["integer_field", "exact", 1.5]"#),
        json!({"custom_field_query": {"2": ["A valid integer is required."]}})
    );
    assert_eq!(
        err(r#"["boolean_field", "exact", "maybe"]"#),
        json!({"custom_field_query": {"2": ["Must be a valid boolean."]}})
    );
    assert_eq!(
        err(r#"["string_field", "exists", null]"#),
        json!({"custom_field_query": {"2": ["This field may not be null."]}})
    );
    assert_eq!(
        err(r#"["url_field", "exact", ""]"#),
        json!({"custom_field_query": {"2": ["This field may not be blank."]}})
    );
    assert_eq!(
        err(r#"["documentlink_field", "contains", "x"]"#),
        json!({"custom_field_query": {"2": ["Expected a list of items but got type \"str\"."]}})
    );
    assert_eq!(
        err(&format!(r#"["string_field", "exact", "{}"]"#, "x".repeat(129))),
        json!({"custom_field_query": {"2": ["Ensure this field has no more than 128 characters."]}})
    );
    assert_eq!(
        err(r#"[1.5, "exact", "x"]"#),
        json!({"custom_field_query": {"0": ["1.5 is not a valid custom field."]}})
    );
    assert_eq!(
        err(r#"[99, "exact", "x"]"#),
        json!({"custom_field_query": {"0": ["99 is not a valid custom field."]}})
    );
    assert_eq!(
        err(r#"["string_field", 5, "x"]"#),
        json!({"custom_field_query": {"1": ["string does not support query expr 5."]}})
    );
}

#[test]
fn unhandled_upstream_exceptions_are_internal() {
    // op.lower() on non-string → AttributeError
    assert!(is_internal(&filter_sql(&cfq(r#"[5, [["string_field", "exists", true]]]"#))));
    // unhashable field key
    assert!(is_internal(&filter_sql(&cfq(r#"[["a"], "exact", "x"]"#))));
    // year__exists → FieldError
    assert!(is_internal(&filter_sql(&cfq(r#"["date_field", "year__exists", true]"#))));
    // Year outside 1..9999 → upstream ValueError (500); PaperRust 400
    assert_eq!(
        validation(filter_sql(&cfq(r#"["date_field", "year__exact", 0]"#))),
        serde_json::json!({"custom_field_query": ["Ensure this value is between 1 and 9999."]})
    );
}

#[test]
fn date_values() {
    assert_err(r#"["date_field", "exact", "not a date"]"#, &["2"], "date");
    assert_err(r#"["date_field", "exact", "January 2024"]"#, &["2"], "date");
    let huge = format!(r#"["date_field", "exact", "today + {} days"]"#, "9".repeat(5000));
    assert_err(&huge, &["2"], "date");
    assert_err(r#"["date_field", "exact", true]"#, &["2"], "format");
    assert_eq!(
        err(r#"["date_field", "exact", true]"#),
        json!({"custom_field_query": {"2": ["Date has wrong format. Use one of these formats instead: YYYY-MM-DD."]}})
    );
    // relative expressions against ctx.now (2024-08-22 14:00 Europe/Berlin)
    let sql = ok(r#"["date_field", "range", ["today", "2024-11-15"]]"#);
    assert!(sql.contains(r#""cq"."value_date" BETWEEN '2024-08-22' AND '2024-11-15'"#), "{sql}");
    let sql = ok(r#"["date_field", "gte", "in 30 days"]"#);
    assert!(sql.contains(r#""cq"."value_date" >= '2024-09-21'"#), "{sql}");
}

#[test]
fn relative_date_languages() {
    let mut c = ctx();
    for langs in [Some(vec!["de".to_string()]), Some(vec!["zh-Hans".into(), "de".into()]), None] {
        c.date_parser_languages = langs;
        let sql = filter_sql_with(&cfq(r#"["date_field", "exact", "heute"]"#), &superuser(), &c, &upstream_fields()).unwrap();
        assert!(sql.contains(r#""cq"."value_date" = '2024-08-22'"#), "{sql}");
    }
    c.date_parser_languages = Some(vec!["en".into()]);
    let v = validation(filter_sql_with(&cfq(r#"["date_field", "exact", "heute"]"#), &superuser(), &c, &upstream_fields()));
    let (path, msg) = error_path(&v);
    assert_eq!(path, vec!["custom_field_query", "2"]);
    assert!(msg.contains("date"));
}

#[test]
fn atom_sql_shapes() {
    assert_eq!(
        ok(r#"["string_field", "exact", "paperless"]"#),
        r#"WHERE EXISTS(SELECT 1 FROM "documents_customfieldinstance" AS "cq" WHERE "cq"."document_id" = "documents_document"."id" AND "cq"."field_id" = 1 AND "cq"."value_text" = 'paperless')"#
    );
    assert_eq!(
        ok(r#"["string_field", "exists", false]"#),
        r#"WHERE NOT EXISTS(SELECT 1 FROM "documents_customfieldinstance" AS "cq" WHERE "cq"."document_id" = "documents_document"."id" AND "cq"."field_id" = 1)"#
    );
    ends(&ok(r#"["string_field", "isnull", true]"#), r#""cq"."value_text" IS NULL)"#);
    ends(&ok(r#"["string_field", "in", ["paperless", "Paperless"]]"#), r#""cq"."value_text" IN ('paperless', 'Paperless'))"#);
    ends(&ok(r#"["string_field", "icontains", "aper"]"#), r#"(UPPER(("cq"."value_text")::text) LIKE UPPER('%aper%')))"#);
    ends(&ok(r#"["url_field", "istartswith", "https://"]"#), r#"(UPPER(("cq"."value_url")::text) LIKE UPPER('https://%')))"#);
    has(&ok(r#"["longtext_field", "iendswith", "x"]"#), r#""cq"."value_long_text""#);
    // SELECT: Label → Option-ID
    ends(&ok(r#"["select_field", "exact", "B"]"#), r#""cq"."value_select" = 'def-456')"#);
    ends(&ok(r#"["select_field", "exact", "def-456"]"#), r#""cq"."value_select" = 'def-456')"#);
    // date parts
    ends(&ok(r#"["date_field", "year__gte", 2024]"#), r#""cq"."value_date" >= '2024-01-01')"#);
    ends(&ok(r#"["date_field", "year__exact", 2024]"#), r#"("cq"."value_date" BETWEEN '2024-01-01' AND '2024-12-31'))"#);
    ends(&ok(r#"["date_field", "month__lt", 9]"#), r#"(EXTRACT(MONTH FROM "cq"."value_date")) < 9)"#);
    ends(&ok(r#"["date_field", "week_day__exact", 1]"#), r#"(EXTRACT(DOW FROM "cq"."value_date") + 1) = 1)"#);
    // Overflow in date parts like IntegerFieldOverflow
    assert_eq!(ok(r#"["date_field", "month__exact", 99999999999]"#), "WHERE FALSE");
    ends(&ok(r#"["date_field", "month__gt", -99999999999]"#), r#""cq"."field_id" = 3)"#);
    // Money: amount (value_monetary_amount), currency prefix removed
    ends(&ok(r#"["monetary_field", "gt", "USD99.00"]"#), r#""cq"."value_monetary_amount" > (CAST('9900e-2' AS numeric)))"#);
    ends(&ok(r#"["monetary_field", "exact", "100"]"#), r#""cq"."value_monetary_amount" = (CAST('100e0' AS numeric)))"#);
    has(&ok(r#"["monetary_field", "icontains", "USD"]"#), r#""cq"."value_monetary""#);
    assert!(is_internal(&filter_sql(&cfq(r#"["monetary_field", "gt", "-inf"]"#))));
    // Float/Bool/Int
    ends(&ok(r#"["float_field", "range", [-0.05, 0.05]]"#), r#"("cq"."value_float" BETWEEN -0.05 AND 0.05))"#);
    ends(&ok(r#"["boolean_field", "exact", "yes"]"#), r#""cq"."value_bool" = TRUE)"#);
    ends(&ok(r#"["integer_field", "gt", "5.0"]"#), r#""cq"."value_int" > 5)"#);
    // document link exact → JSONB equality
    ends(&ok(r#"["documentlink_field", "exact", [1, 2]]"#), r#""cq"."value_document_ids" = '[1,2]')"#);
}

#[test]
fn logical_combinations() {
    let and = ok(r#"["AND", [["date_field", "year__exact", 2024], ["date_field", "month__lt", 9]]]"#);
    assert!(and.starts_with("WHERE EXISTS(") && and.contains(") AND EXISTS("), "{and}");
    let or = ok(r#"["OR", [["string_field", "exact", ""], ["string_field", "isnull", true]]]"#);
    assert!(or.contains(r#""cq"."value_text" = ''"#) && or.contains(") OR EXISTS("), "{or}");
    let not = ok(r#"["NOT", ["string_field", "exact", "paperless"]]"#);
    assert!(not.starts_with("WHERE NOT EXISTS("), "{not}");
    // Case does not matter
    ok(r#"["oR", [["string_field", "exists", true]]]"#);
}

#[test]
fn document_link_contains() {
    let mut db = upstream_fields();
    // Document 10 links to 1,2,3; document 11 to 1,2; document 12 to 2
    db.links = vec![
        (1, 8, json!([10, 11])),
        (2, 8, json!([10, 11, 12])),
        (6, 8, Value::Null),
    ];
    let run = |q: &str| filter_sql_with(&cfq(q), &superuser(), &ctx(), &db).unwrap();
    assert_eq!(
        run(r#"["documentlink_field", "contains", [1, 2]]"#),
        r#"WHERE "documents_document"."id" IN (10, 11)"#
    );
    assert_eq!(
        run(r#"["documentlink_field", "contains", [2, 1]]"#),
        r#"WHERE "documents_document"."id" IN (10, 11)"#
    );
    // no back references for 7 → empty
    assert_eq!(run(r#"["documentlink_field", "contains", [7]]"#), "WHERE FALSE");
    assert_eq!(
        run(r#"["documentlink_field", "contains", []]"#),
        r#"WHERE EXISTS(SELECT 1 FROM "documents_customfieldinstance" AS "cq" WHERE "cq"."document_id" = "documents_document"."id" AND "cq"."field_id" = 8 AND "cq"."value_document_ids" IS NOT NULL)"#
    );
    // NOT(empty) → no filter
    assert_eq!(run(r#"["NOT", ["documentlink_field", "contains", [7]]]"#), "");
}

#[test]
fn field_lookup_by_id_and_cache() {
    has(&ok(r#"[1, "exact", "x"]"#), r#""cq"."field_id" = 1"#);
    // True == 1 in Python
    has(&ok(r#"[true, "exact", "x"]"#), r#""cq"."field_id" = 1"#);
    assert_eq!(
        err(r#"[false, "exact", "x"]"#),
        json!({"custom_field_query": {"0": ["False is not a valid custom field."]}})
    );
    assert_eq!(
        err(r#"[null, "exact", "x"]"#),
        json!({"custom_field_query": {"0": ["None is not a valid custom field."]}})
    );
}
