//! Parity comparison against the reference environment (reference/README.md): Django response of
//! `GET /api/documents/?<filter>&fields=id` vs. the SQL generated here on the same DB.
//! Read-only. Runs only explicitly:
//!
//!   cargo test -p pl-docfilter -j 2 --test parity -- --ignored --nocapture
//!
//! Environment: PL_REF_DB (default postgres://paperless:paperless@127.0.0.1:54329/paperless),
//! PL_REF_DJANGO (default http://127.0.0.1:8000).

use pl_authz::models::DOCUMENT;
use pl_authz::objects::{FilterOpts, permitted_objects_filter};
use pl_core::sqlbind::SqlxBinder;
use pl_docfilter::{FilterContext, FilterError, QueryParams};
use sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::Value;
use sqlx::Row;

const USERS: &[(&str, i64, &str)] =
    &[("admin", 2, "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001"), ("alice", 3, "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002")];

fn queries() -> Vec<String> {
    let mut q: Vec<String> = [
        // Ordering
        "",
        "ordering=title",
        "ordering=-title",
        "ordering=-created",
        "ordering=archive_serial_number",
        "ordering=-num_notes",
        "ordering=owner",
        "ordering=correspondent__name",
        "ordering=-document_type__name",
        "ordering=storage_path__name",
        "ordering=page_count",
        "ordering=added",
        "ordering=-modified",
        "ordering=id",
        "ordering=bogus",
        "ordering=title,-id",
        "ordering=custom_field_999",
        "ordering=-num_notes,id",
        // Filter
        "title__icontains=tax",
        "title__istartswith=a",
        "title__iendswith=e",
        "title__iexact=x",
        "content__icontains=quarterly",
        "content__istartswith=d",
        "title_content=steuer",
        // O2: fast text search (textfold) incl. special characters/non-ASCII/escapes
        "title_content=strasse",
        "title_content=fi",
        "title_content=%C3%9F",
        "title_content=a_b%25",
        "content__iexact=x",
        "content__iendswith=n",
        "content__icontains=SS",
        "search=ss",
        "archive_serial_number__gt=1100",
        "archive_serial_number__lte=1050",
        "archive_serial_number=1005",
        "archive_serial_number__isnull=1",
        "archive_serial_number__gt=-1",
        "created__year=2024",
        "created__month=3",
        "created__day=15",
        "created__gt=2024-06-01",
        "created__date__lte=2024-12-31",
        "created__lt=10/25/2024",
        "added__date__gt=2025-01-10",
        "added__date__lte=2025-01-08",
        "added__gt=2025-01-10T00:00:00Z",
        "added__lt=2025-01-09",
        "modified__year=2025",
        "added__month=1",
        "added__day=7",
        "correspondent__isnull=1",
        "correspondent__isnull=0",
        "correspondent__id=1",
        "correspondent__id__in=1,2",
        "correspondent__id__none=1",
        "correspondent__name__icontains=a",
        "document_type__id__none=1",
        "document_type__name__istartswith=in",
        "storage_path__id__none=1",
        "storage_path__isnull=1",
        "tags__id__all=2,3",
        "tags__id__in=1,2",
        "tags__id__none=1",
        "tags__id=6",
        "tags__name__iexact=paid",
        "tags__name__icontains=tax",
        "is_tagged=0",
        "is_tagged=1",
        "is_in_inbox=true",
        "is_in_inbox=false",
        "owner__id=3",
        "owner__id__in=3,4",
        "owner__id__none=2",
        "owner__isnull=1",
        "shared_by__id=2",
        "shared_by__id=3",
        "shared_by__id=4",
        "mime_type=pdf",
        "mime_type=text",
        "has_custom_fields=1",
        "has_custom_fields=0",
        "custom_fields__id__all=1,3",
        "custom_fields__id__none=1",
        "custom_fields__id__in=1,2",
        "custom_fields__icontains=a",
        "custom_fields__icontains=medium",
        "custom_fields__icontains=CHF",
        "has_duplicates=1",
        "has_duplicates=0",
        "search=tax",
        "search=%22quarterly-tax%22",
        "search=steuer,rechnung",
        "id__in=1,2,3,500",
        "id=5",
        "id__in=,",
        "tags__id__all=1,x",
        "page_count=1",
        // Form errors
        "archive_serial_number=abc",
        "created__gt=bad",
        "added__gt=bad",
        "id__in=1,x&title__icontains=a%00",
        "search=a%00",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for id in 1..=11 {
        q.push(format!("ordering=custom_field_{id}"));
        q.push(format!("ordering=-custom_field_{id}"));
        q.push(cfq(&format!("[{id}, \"exists\", true]")));
        q.push(cfq(&format!("[{id}, \"isnull\", true]")));
        q.push(cfq(&format!("[\"NOT\", [{id}, \"exists\", true]]")));
    }
    for j in [
        r#"["Text", "icontains", "a"]"#,
        r#"["Text", "exact", ""]"#,
        r#"["Website", "istartswith", "https"]"#,
        r#"["Due date", "gte", "2025-01-01"]"#,
        r#"["Due date", "year__exact", 2025]"#,
        r#"["Due date", "month__lt", 6]"#,
        r#"["Due date", "range", ["2024-01-01", "today"]]"#,
        r#"["Paid?", "exact", true]"#,
        r#"["Paid?", "exact", "false"]"#,
        r#"["Count", "gt", 5]"#,
        r#"["Count", "in", [1, 2, 3]]"#,
        r#"["Score", "range", [0, 50.5]]"#,
        r#"["Amount", "gt", "100"]"#,
        r#"["Amount", "exact", "CHF100.00"]"#,
        r#"["Amount (no default)", "lte", "EUR50"]"#,
        r#"["Amount", "icontains", "CHF"]"#,
        r#"["Related docs", "contains", [1]]"#,
        r#"["Related docs", "contains", []]"#,
        r#"["Related docs", "contains", [1, 2]]"#,
        r#"["Priority", "exact", "Medium"]"#,
        r#"["Priority", "in", ["Low", "pr3HighAAAAAAAAA"]]"#,
        r#"["Notes (long)", "icontains", "e"]"#,
        r#"["OR", [["Count", "gt", 5], ["Paid?", "exact", true]]]"#,
        r#"["AND", [["Count", "isnull", false], ["NOT", ["Score", "lt", 10]]]]"#,
        // Errors
        "not json",
        r#"["nope", "exact", 1]"#,
        r#"["Count", "icontains", 1]"#,
        r#"["Count", "exact", "x"]"#,
        r#"["Due date", "exact", "not a date"]"#,
        r#"["AND", []]"#,
        r#"["xor", [["Count", "gt", 1]]]"#,
    ] {
        q.push(cfq(j));
    }
    q
}

fn cfq(json: &str) -> String {
    let enc: String = url::form_urlencoded::byte_serialize(json.as_bytes()).collect();
    format!("custom_field_query={enc}")
}

enum Outcome {
    Ids(Vec<i64>),
    Error(u16, Value),
}

async fn django(client: &reqwest::Client, base: &str, token: &str, q: &str) -> Outcome {
    let url = format!("{base}/api/documents/?{q}&fields=id&page_size=100000");
    let r = client
        .get(&url)
        .header("Authorization", format!("Token {token}"))
        .header("Accept", "application/json; version=9")
        .send()
        .await
        .expect("django request");
    let status = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    if status != 200 {
        return Outcome::Error(status, serde_json::from_str(&text).unwrap_or(Value::String(text)));
    }
    let v: Value = serde_json::from_str(&text).expect("json");
    Outcome::Ids(v["results"].as_array().expect("results").iter().map(|d| d["id"].as_i64().unwrap()).collect())
}

async fn rust(pool: &sqlx::PgPool, user: &pl_authz::User, q: &str) -> Outcome {
    let params = QueryParams::parse(q);
    let ctx = FilterContext::new("Europe/Zurich".parse().unwrap());
    let mut sel = Query::select()
        .column((Alias::new("documents_document"), Alias::new("id")))
        .from(Alias::new("documents_document"))
        .and_where(Expr::col((Alias::new("documents_document"), Alias::new("root_document_id"))).is_null())
        .and_where(Expr::col((Alias::new("documents_document"), Alias::new("deleted_at"))).is_null())
        .to_owned();
    let res: Result<(), FilterError> = async {
        pl_docfilter::apply(&mut sel, &params, user, &ctx, pool).await?;
        pl_docfilter::apply_ordering(&mut sel, params.get("ordering"), pool).await?;
        Ok(())
    }
    .await;
    if let Err(e) = res {
        return match e {
            FilterError::Validation(v) => Outcome::Error(400, v),
            other => Outcome::Error(500, Value::String(other.to_string())),
        };
    }
    sel.cond_where(permitted_objects_filter(
        Some(user),
        &DOCUMENT,
        "documents_document",
        &FilterOpts { include_granted: true, perm_codename: None, parent_field: None },
    ));
    let (sql, values) = sel.build_sqlx(PostgresQueryBuilder);
    match sqlx::query_with(&sql, values).fetch_all(pool).await {
        Ok(rows) => Outcome::Ids(rows.iter().map(|r| r.get::<i32, _>(0) as i64).collect()),
        Err(e) => Outcome::Error(500, Value::String(format!("{e}: {sql}"))),
    }
}

#[tokio::test]
#[ignore = "braucht die laufende Referenzumgebung (reference/README.md)"]
async fn parity_with_django() {
    let db = std::env::var("PL_REF_DB").unwrap_or_else(|_| "postgres://paperless:paperless@127.0.0.1:54329/paperless".into());
    let base = std::env::var("PL_REF_DJANGO").unwrap_or_else(|_| "http://127.0.0.1:8000".into());
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&db).await.expect("db");
    let client = reqwest::Client::new();
    let (mut same, mut tie_only, mut different) = (0, 0, Vec::new());
    for (name, id, token) in USERS {
        let user = pl_authz::user::load_user_by_id(&pool, *id).await.expect("load").expect("user");
        for q in queries() {
            let d = django(&client, &base, token, &q).await;
            let r = rust(&pool, &user, &q).await;
            match (&d, &r) {
                (Outcome::Ids(a), Outcome::Ids(b)) if a == b => same += 1,
                (Outcome::Ids(a), Outcome::Ids(b)) => {
                    let (mut sa, mut sb) = (a.clone(), b.clone());
                    sa.sort_unstable();
                    sb.sort_unstable();
                    if sa == sb && q.contains("ordering=") {
                        // same set, different order on ties (no tiebreaker upstream)
                        tie_only += 1;
                        println!("TIE   {name} {q}");
                    } else {
                        let only_d: Vec<_> = sa.iter().filter(|x| !sb.contains(x)).collect();
                        let only_r: Vec<_> = sb.iter().filter(|x| !sa.contains(x)).collect();
                        different.push(format!(
                            "{name} {q}: django {} / rust {} (nur django {:?}, nur rust {:?})",
                            a.len(),
                            b.len(),
                            only_d,
                            only_r
                        ));
                    }
                }
                (Outcome::Error(sa, ba), Outcome::Error(sb, bb)) if sa == sb && (ba == bb || *sa == 500) => same += 1,
                (d, r) => different.push(format!("{name} {q}: django {} / rust {}", show(d), show(r))),
            }
        }
    }
    println!("same={same} tie_only={tie_only} different={}", different.len());
    for d in &different {
        println!("DIFF  {d}");
    }
    assert!(different.is_empty());
}

fn show(o: &Outcome) -> String {
    match o {
        Outcome::Ids(v) => format!("{} ids", v.len()),
        Outcome::Error(s, b) => format!("{s} {}", b.to_string().chars().take(300).collect::<String>()),
    }
}
