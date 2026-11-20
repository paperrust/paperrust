//! Tests for DocumentFilterSet (form validation + SQL), after upstream
//! src/documents/tests/test_api_documents.py and django-filter behavior.

mod common;

use common::*;
use serde_json::json;

#[test]
fn no_params_no_where() {
    assert_eq!(filter_sql("").unwrap(), "");
    assert_eq!(filter_sql("unknown=1&page=2&page_size=5").unwrap(), "");
}

#[test]
fn number_filter_errors_like_django_forms() {
    assert_eq!(
        validation(filter_sql("archive_serial_number=abc")),
        json!({"archive_serial_number": ["Enter a number."]})
    );
    assert_eq!(validation(filter_sql("id__in=1,x")), json!({"id__in": ["Enter a number."]}));
    assert_eq!(validation(filter_sql("id=NaN")), json!({"id": ["Enter a number."]}));
    assert_eq!(
        validation(filter_sql("archive_serial_number__gt=1e51")),
        json!({"archive_serial_number__gt": ["Ensure this value is less than or equal to 1e+50."]})
    );
    // several errors in form order (id__in before id before title…)
    let v = validation(filter_sql("title__icontains=a%00b&id=x&id__in=y"));
    let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    assert_eq!(keys, vec!["id__in", "id", "title__icontains"]);
    assert_eq!(v["title__icontains"], json!(["Null characters are not allowed."]));
}

#[test]
fn date_and_datetime_errors() {
    assert_eq!(validation(filter_sql("created__gt=bad")), json!({"created__gt": ["Enter a valid date."]}));
    assert_eq!(
        validation(filter_sql("added__date__lte=2024-02-30")),
        json!({"added__date__lte": ["Enter a valid date."]})
    );
    assert_eq!(validation(filter_sql("added__gt=bad")), json!({"added__gt": ["Enter a valid date/time."]}));
    assert_eq!(
        validation(filter_sql("modified__lt=2024-03-31T02:30:00")),
        json!({"modified__lt": ["2024-03-31 02:30:00 couldn\u{2019}t be interpreted in time zone Europe/Berlin; it may be ambiguous or it may not exist."]})
    );
}

#[test]
fn empty_values_are_ignored() {
    for q in [
        "id=",
        "id__in=",
        "title__icontains=",
        "title__icontains=%20%20",
        "created__gt=",
        "added__gt=",
        "is_tagged=",
        "is_tagged=maybe",
        "tags__id__all=",
        "tags__id__all=1,x",
        "tags__id__none=1,",
        "is_in_inbox=yes",
        "title_content=%20",
        "content__icontains=",
        "mime_type=",
        "custom_field_query=",
        "custom_fields__icontains=%20",
        "search=",
        "search=%20,%20",
        "has_duplicates=",
    ] {
        assert_eq!(filter_sql(q).unwrap(), "", "query {q}");
    }
}

#[test]
fn id_and_number_lookups() {
    assert_eq!(filter_sql("id=5").unwrap(), r#"WHERE "documents_document"."id" = 5"#);
    // int(Decimal("1.9")) == 1
    assert_eq!(
        filter_sql("archive_serial_number=1.9").unwrap(),
        r#"WHERE "documents_document"."archive_serial_number" = 1"#
    );
    assert_eq!(
        filter_sql("archive_serial_number__gte=%2010%20").unwrap(),
        r#"WHERE "documents_document"."archive_serial_number" >= 10"#
    );
    // IntegerFieldOverflow: PositiveIntegerField (0..2147483647)
    assert_eq!(filter_sql("archive_serial_number__gt=-1").unwrap(), "");
    assert_eq!(filter_sql("archive_serial_number=99999999999").unwrap(), "WHERE FALSE");
    assert_eq!(filter_sql("archive_serial_number__lt=99999999999").unwrap(), "");
    assert_eq!(filter_sql("archive_serial_number__lte=-5").unwrap(), "WHERE FALSE");
    assert_eq!(
        filter_sql("archive_serial_number__isnull=TRUE").unwrap(),
        r#"WHERE "documents_document"."archive_serial_number" IS NULL"#
    );
    assert_eq!(
        filter_sql("id__in=3,%204,5").unwrap(),
        r#"WHERE "documents_document"."id" IN (3, 4, 5)"#
    );
    // [None, None] → id__in=[] → EmptyResultSet
    assert_eq!(filter_sql("id__in=,").unwrap(), "WHERE FALSE");
    assert_eq!(filter_sql("id__in=1,").unwrap(), r#"WHERE "documents_document"."id" IN (1)"#);
    assert_eq!(
        filter_sql("correspondent__id=3&owner__isnull=0").unwrap(),
        r#"WHERE "documents_document"."correspondent_id" = 3 AND "documents_document"."owner_id" IS NOT NULL"#
    );
}

#[test]
fn tag_filters_use_exists() {
    assert_eq!(
        filter_sql("tags__id__all=1,2").unwrap(),
        r#"WHERE EXISTS(SELECT 1 FROM "documents_document_tags" AS "dt" WHERE "dt"."document_id" = "documents_document"."id" AND "dt"."tag_id" = 1) AND EXISTS(SELECT 1 FROM "documents_document_tags" AS "dt" WHERE "dt"."document_id" = "documents_document"."id" AND "dt"."tag_id" = 2)"#
    );
    assert_eq!(
        filter_sql("tags__id__none=3").unwrap(),
        r#"WHERE NOT EXISTS(SELECT 1 FROM "documents_document_tags" AS "dt" WHERE "dt"."document_id" = "documents_document"."id" AND "dt"."tag_id" = 3)"#
    );
    assert_eq!(
        filter_sql("tags__id__in=1,2").unwrap(),
        r#"WHERE EXISTS(SELECT 1 FROM "documents_document_tags" AS "dt" WHERE "dt"."document_id" = "documents_document"."id" AND "dt"."tag_id" IN (1, 2))"#
    );
    assert_eq!(
        filter_sql("tags__id=4").unwrap(),
        r#"WHERE EXISTS(SELECT 1 FROM "documents_document_tags" AS "dt" WHERE "dt"."document_id" = "documents_document"."id" AND "dt"."tag_id" = 4)"#
    );
    assert_eq!(
        filter_sql("is_tagged=1").unwrap(),
        r#"WHERE EXISTS(SELECT 1 FROM "documents_document_tags" AS "dt" WHERE "dt"."document_id" = "documents_document"."id")"#
    );
    assert_eq!(
        filter_sql("is_tagged=false").unwrap(),
        r#"WHERE NOT EXISTS(SELECT 1 FROM "documents_document_tags" AS "dt" WHERE "dt"."document_id" = "documents_document"."id")"#
    );
    let inbox = filter_sql("is_in_inbox=true").unwrap();
    assert!(inbox.contains(r#""it"."is_inbox_tag" = TRUE"#), "{inbox}");
    assert!(filter_sql("is_in_inbox=false").unwrap().starts_with("WHERE NOT EXISTS"));
    // InboxFilter compares exactly (no lower())
    assert_eq!(filter_sql("is_in_inbox=True").unwrap(), "");
    let name = filter_sql("tags__name__iexact=Inbox").unwrap();
    assert!(name.contains(r#"UPPER(("tn"."name")::text) = UPPER('Inbox')"#), "{name}");
    // invalid IDs → ObjectFilter ignores the value; too-large IDs → no matches
    assert_eq!(filter_sql("tags__id__in=a,b").unwrap(), "");
    assert_eq!(filter_sql("tags__id__all=99999999999").unwrap(), "WHERE FALSE");
    assert_eq!(filter_sql("tags__id__none=99999999999").unwrap(), "");
    assert_eq!(filter_sql("tags__id__in=99999999999").unwrap(), "WHERE FALSE");
}

#[test]
fn fk_none_excludes_like_django() {
    assert_eq!(
        filter_sql("correspondent__id__none=5").unwrap(),
        r#"WHERE NOT ("documents_document"."correspondent_id" = 5 AND "documents_document"."correspondent_id" IS NOT NULL)"#
    );
    assert_eq!(
        filter_sql("owner__id__none=1,2").unwrap(),
        r#"WHERE (NOT ("documents_document"."owner_id" = 1 AND "documents_document"."owner_id" IS NOT NULL)) AND (NOT ("documents_document"."owner_id" = 2 AND "documents_document"."owner_id" IS NOT NULL))"#
    );
}

#[test]
fn char_lookups() {
    assert_eq!(
        filter_sql("title__icontains=%20a_b%25c%20").unwrap(),
        r#"WHERE UPPER(("documents_document"."title")::text) LIKE UPPER(E'%a\\_b\\%c%')"#
    );
    assert_eq!(
        filter_sql("checksum__istartswith=ab").unwrap(),
        r#"WHERE UPPER(("documents_document"."checksum")::text) LIKE UPPER('ab%')"#
    );
    assert_eq!(
        filter_sql("correspondent__name__iendswith=x").unwrap(),
        r#"WHERE "documents_document"."correspondent_id" IN (SELECT "fkn"."id" FROM "documents_correspondent" AS "fkn" WHERE UPPER(("fkn"."name")::text) LIKE UPPER('%x'))"#
    );
    assert_eq!(
        filter_sql("mime_type=%20pdf%20").unwrap(),
        r#"WHERE UPPER(("documents_document"."mime_type")::text) LIKE UPPER('%pdf%')"#
    );
}

#[test]
fn content_filters_use_effective_content() {
    let sql = filter_sql("content__icontains=foo").unwrap();
    assert_eq!(
        sql,
        r#"WHERE UPPER((COALESCE(((SELECT "ec_v"."content" FROM "documents_document" AS "ec_v" WHERE "ec_v"."root_document_id" = "documents_document"."id" AND "ec_v"."deleted_at" IS NULL ORDER BY "ec_v"."version_index" DESC NULLS LAST, "ec_v"."id" DESC LIMIT 1)), "documents_document"."content"))::text) LIKE UPPER('%foo%')"#
    );
    let tc = filter_sql("title_content=foo").unwrap();
    assert!(tc.starts_with(r#"WHERE (UPPER(("documents_document"."title")::text) LIKE UPPER('%foo%')) OR (UPPER((COALESCE("#), "{tc}");
}

#[test]
fn date_filters() {
    assert_eq!(
        filter_sql("created__gt=2024-01-05").unwrap(),
        r#"WHERE "documents_document"."created" > '2024-01-05'"#
    );
    assert_eq!(
        filter_sql("created__date__lte=10/25/2006").unwrap(),
        r#"WHERE "documents_document"."created" <= '2006-10-25'"#
    );
    assert_eq!(
        filter_sql("created__year=2024").unwrap(),
        r#"WHERE "documents_document"."created" BETWEEN '2024-01-01' AND '2024-12-31'"#
    );
    // upstream ValueError → 500; PaperRust 400 (B-305/D-039)
    for q in ["created__year=0", "created__year=-1", "created__year=10000", "created__year=99999999999", "created__year=999999999999999999999999"] {
        assert_eq!(validation(filter_sql(q)), serde_json::json!({"created__year": ["Ensure this value is between 1 and 9999."]}), "{q}");
    }
    assert_eq!(validation(filter_sql("added__year=-1")), serde_json::json!({"added__year": ["Ensure this value is between 1 and 9999."]}));
    assert_eq!(
        filter_sql("created__month=5").unwrap(),
        r#"WHERE (EXTRACT(MONTH FROM "documents_document"."created")) = 5"#
    );
    assert_eq!(filter_sql("created__day=99999999999").unwrap(), "WHERE FALSE");
    // added__date__gt=2024-01-01 in Europe/Berlin ⇔ added >= 2024-01-01T23:00:00Z
    assert_eq!(
        filter_sql("added__date__gt=2024-01-01").unwrap(),
        r#"WHERE "documents_document"."added" >= '2024-01-01 23:00:00.000000 +00:00'"#
    );
    assert_eq!(
        filter_sql("added__date__lt=2024-07-01").unwrap(),
        r#"WHERE "documents_document"."added" < '2024-06-30 22:00:00.000000 +00:00'"#
    );
    assert_eq!(
        filter_sql("added__date__lte=9999-12-31").unwrap(),
        r#"WHERE "documents_document"."added" < '9999-12-31 23:00:00.000000 +00:00'"#
    );
    assert_eq!(
        filter_sql("modified__gte=2024-01-15T10:00:00%2B01:00").unwrap(),
        r#"WHERE "documents_document"."modified" >= '2024-01-15 09:00:00.000000 +00:00'"#
    );
    assert_eq!(
        filter_sql("added__year=2024").unwrap(),
        r#"WHERE "documents_document"."added" >= '2023-12-31 23:00:00.000000 +00:00' AND "documents_document"."added" < '2024-12-31 23:00:00.000000 +00:00'"#
    );
    assert_eq!(
        filter_sql("added__month=5").unwrap(),
        r#"WHERE (EXTRACT(MONTH FROM "documents_document"."added" AT TIME ZONE 'Europe/Berlin')) = 5"#
    );
}

#[test]
fn shared_by_and_duplicates() {
    assert!(is_internal(&filter_sql("shared_by__id=abc")));
    // forms.Field: empty string is not None → int("") → 500 like upstream
    assert!(is_internal(&filter_sql("shared_by__id=")));
    assert_eq!(filter_sql("shared_by__id=99999999999").unwrap(), "WHERE FALSE");
    let s = filter_sql("shared_by__id=5").unwrap();
    assert!(s.starts_with(r#"WHERE "documents_document"."owner_id" = 5 AND (EXISTS(SELECT 1 FROM "guardian_userobjectpermission" AS "sbu""#), "{s}");
    assert!(s.contains(r#""sbu"."object_pk" = (CAST("documents_document"."id" AS varchar))"#), "{s}");
    assert!(s.contains(r#"OR EXISTS(SELECT 1 FROM "guardian_groupobjectpermission" AS "sbg""#));

    let su = filter_sql("has_duplicates=true").unwrap();
    // Candidate prefilter (superset via the checksum indexes), then the exact condition
    let (cand, exact) = su.split_once(" AND EXISTS(").expect("Kandidaten + EXISTS");
    assert!(cand.starts_with(r#"WHERE ("documents_document"."id" = ANY(ARRAY(SELECT "dupc"."id" FROM "documents_document" AS "dupc""#), "{su}");
    assert!(cand.contains(r#"GROUP BY "dupk1"."checksum" HAVING COUNT(*) > 1"#), "{su}");
    assert!(cand.contains(r#"GROUP BY "dupk2"."archive_checksum" HAVING COUNT(*) > 1"#), "{su}");
    assert!(!cand.contains("guardian"));
    let su = format!("WHERE EXISTS({exact}");
    assert_eq!(su,r#"WHERE EXISTS(SELECT 1 FROM "documents_document" AS "dup" WHERE "dup"."root_document_id" IS NULL AND (NOT "dup"."id" = "documents_document"."id") AND ("dup"."checksum" = "documents_document"."checksum" OR "dup"."checksum" = (NULLIF("documents_document"."archive_checksum", '')) OR "dup"."archive_checksum" = "documents_document"."checksum" OR "dup"."archive_checksum" = (NULLIF("documents_document"."archive_checksum", ''))))"#);
    assert!(!su.contains("guardian"));
    let reg = filter_sql_with("has_duplicates=0", &regular_user(), &ctx(), &upstream_fields()).unwrap();
    assert!(reg.starts_with("WHERE NOT EXISTS"), "{reg}");
    assert!(reg.contains(r#""dup"."id" IN (SELECT "pl_authz_obj"."id" FROM "documents_document" AS "pl_authz_obj""#), "{reg}");
    assert!(reg.contains("guardian_userobjectpermission") && reg.contains("'view_document'"), "{reg}");
    let inactive = mk_user(7, false, true);
    let ina = filter_sql_with("has_duplicates=1", &inactive, &ctx(), &upstream_fields()).unwrap();
    assert!(ina.contains("pl_authz_obj") && ina.contains("FALSE"), "{ina}");
}

#[test]
fn custom_field_relation_filters() {
    assert_eq!(
        filter_sql("custom_fields__id__all=1").unwrap(),
        r#"WHERE EXISTS(SELECT 1 FROM "documents_customfieldinstance" AS "ci" WHERE "ci"."document_id" = "documents_document"."id" AND "ci"."field_id" = 1)"#
    );
    assert!(filter_sql("custom_fields__id__none=1").unwrap().starts_with("WHERE NOT EXISTS"));
    assert!(filter_sql("custom_fields__id__in=1,2").unwrap().ends_with(r#""ci"."field_id" IN (1, 2))"#));
    assert_eq!(
        filter_sql("has_custom_fields=true").unwrap(),
        r#"WHERE EXISTS(SELECT 1 FROM "documents_customfieldinstance" AS "ci" WHERE "ci"."document_id" = "documents_document"."id")"#
    );
}

#[test]
fn custom_fields_icontains_matches_select_labels() {
    let sql = filter_sql("custom_fields__icontains=b").unwrap();
    // Label "B" matches → option ID def-456
    assert!(sql.contains(r#""ci"."value_select" IN ('def-456')"#), "{sql}");
    assert!(sql.contains(r#"UPPER(("ci"."value_bool")::text) LIKE UPPER('%b%')"#), "{sql}");
    let none = filter_sql("custom_fields__icontains=zzz").unwrap();
    assert!(!none.contains("value_select"), "{none}");
}

#[test]
fn search_filter() {
    let sql = filter_sql("search=foo%20%22bar%20baz%22").unwrap();
    assert!(sql.contains(r#"UPPER(("documents_document"."title")::text) LIKE UPPER('%foo%')"#), "{sql}");
    assert!(sql.contains(r#"LIKE UPPER('%bar baz%')"#), "{sql}");
    assert!(sql.contains(r#""documents_document"."correspondent_id" IN (SELECT "sc"."id" FROM "documents_correspondent" AS "sc""#), "{sql}");
    assert_eq!(
        validation(filter_sql("search=a%00")),
        json!(["Null characters are not allowed."])
    );
}

#[test]
fn filters_only_add_and_conditions() {
    // Several filters → only AND combinations at the top level
    let sql = filter_sql("id=1&title__icontains=x&is_tagged=1").unwrap();
    assert!(sql.starts_with(r#"WHERE "documents_document"."id" = 1 AND (UPPER"#), "{sql}");
}

#[test]
fn filter_names_cover_upstream_filterset() {
    let names = pl_docfilter::filterset::filter_names();
    for n in [
        "id", "id__in", "title__icontains", "archive_serial_number__isnull", "created__year",
        "added__date__gte", "modified__lte", "correspondent__isnull", "correspondent__id__in",
        "tags__id__all", "tags__id__none", "tags__id__in", "is_tagged", "is_in_inbox",
        "title_content", "content__iexact", "owner__id__none", "custom_fields__icontains",
        "custom_fields__id__all", "custom_fields__id__none", "custom_fields__id__in",
        "has_custom_fields", "custom_field_query", "shared_by__id", "mime_type",
        "has_duplicates", "created__date__gt", "storage_path__name__iexact",
    ] {
        assert!(names.contains(&n), "{n}");
    }
    assert_eq!(names.len(), 105);
}
