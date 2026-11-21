//! ASN label API (`/rs/api/v2/asn/…`, crate pl-labels): permissions (`documents.change_document`),
//! next free ASN (incl. trash + printed ranges), collisions (documents/prints),
//! PDF + log `paperrust_asn_print`, release, separator sheet, ApplicationConfiguration precedence.
//!
//! Against a **dedicated copy** (never `paperless`):
//! ```text
//! psql -h 127.0.0.1 -p 54329 -U paperless -d postgres -c "CREATE DATABASE pl_asn_api_test TEMPLATE paperless_snapshot"
//! PAPERRUST_ASN_TEST_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/pl_asn_api_test \
//!   cargo test -p pl-api-v2 --test asn_api -j 2 -- --ignored --test-threads 1
//! psql … -c "DROP DATABASE pl_asn_api_test"
//! ```

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use pl_core::AppState;
use pl_core::config::{Config, Env};
use serde_json::{Value, json};
use tower::ServiceExt;

const SU: &str = "plasnapitestsu00000000000000000000000000";
const EDITOR: &str = "plasnapitested0000000000000000000000000";
const EDITOR2: &str = "plasnapitested2000000000000000000000000";
const VIEWER: &str = "plasnapitestvw0000000000000000000000000";
const INACTIVE: &str = "plasnapitestin0000000000000000000000000";

fn url() -> String {
    let url = std::env::var("PAPERRUST_ASN_TEST_DATABASE_URL").expect("PAPERRUST_ASN_TEST_DATABASE_URL");
    let db = url.trim_end_matches('/').rsplit('/').next().unwrap_or("").split('?').next().unwrap_or("").to_string();
    assert!(db.starts_with("pl_asn"), "refusing to run against a non-test database ({db})");
    url
}

async fn user(pool: &sqlx::PgPool, name: &str, su: bool, active: bool, perms: &[&str], token: &str) {
    let id: i32 = sqlx::query_scalar(
        "INSERT INTO auth_user (password, is_superuser, username, first_name, last_name, email, is_staff, is_active, date_joined)
         VALUES ('!', $1, $2, '', '', '', $1, $3, now()) RETURNING id",
    )
    .bind(su)
    .bind(name)
    .bind(active)
    .fetch_one(pool)
    .await
    .unwrap();
    for p in perms {
        sqlx::query(
            "INSERT INTO auth_user_user_permissions (user_id, permission_id)
             SELECT $1, p.id FROM auth_permission p JOIN django_content_type ct ON ct.id = p.content_type_id
             WHERE ct.app_label = 'documents' AND p.codename = $2",
        )
        .bind(id)
        .bind(p)
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query("INSERT INTO authtoken_token (key, created, user_id) VALUES ($1, now(), $2)")
        .bind(token)
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
}

async fn setup() -> (Router, sqlx::PgPool) {
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(4).connect(&url()).await.expect("connect");
    sqlx::raw_sql(
        "DROP TABLE IF EXISTS paperrust_asn_print;
         -- Stand der Testdaten wiederherstellen (ASN 1001–1200, keine Barcode-Overrides)
         UPDATE documents_document SET archive_serial_number = NULL WHERE archive_serial_number > 1200;
         UPDATE paperless_applicationconfiguration SET barcode_asn_prefix = NULL, barcode_string = NULL,
                barcodes_enabled = NULL, barcode_enable_asn = NULL;
         DELETE FROM authtoken_token WHERE key LIKE 'plasnapitest%';
         DELETE FROM auth_user_user_permissions WHERE user_id IN (SELECT id FROM auth_user WHERE username LIKE 'pl_asn_api_%');
         DELETE FROM auth_user WHERE username LIKE 'pl_asn_api_%';",
    )
    .execute(&pool)
    .await
    .unwrap();
    user(&pool, "pl_asn_api_su", true, true, &[], SU).await;
    user(&pool, "pl_asn_api_editor", false, true, &["view_document", "change_document"], EDITOR).await;
    user(&pool, "pl_asn_api_editor2", false, true, &["change_document"], EDITOR2).await;
    user(&pool, "pl_asn_api_viewer", false, true, &["view_document", "add_document"], VIEWER).await;
    user(&pool, "pl_asn_api_inactive", false, false, &["change_document"], INACTIVE).await;
    let cfg = Config::from_env_map(Env::from_pairs([
        ("PAPERLESS_SECRET_KEY", "pl-asn-api-tests-only-0123456789abcdefghijklmnopqrstu"),
        ("PAPERLESS_DBHOST", "127.0.0.1"),
    ]))
    .expect("config");
    let state = AppState::with_pool(cfg.clone(), pool.clone()).expect("state");
    let authz = pl_authz::Authz::new(pool.clone(), pl_authz::AuthConfig::from_core(&cfg));
    let r: Router<AppState> = pl_api_v2::asn::routes(Router::new());
    (r.with_state(state).layer(axum::Extension(authz)), pool)
}

struct Resp {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    bytes: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.bytes).unwrap_or(Value::Null)
    }
}

async fn call(router: &Router, method: &str, uri: &str, token: &str, body: Option<Value>) -> Resp {
    let mut b = Request::builder().method(method).uri(uri).header("host", "localhost").header("authorization", format!("Token {token}"));
    let body = match body {
        Some(v) => {
            b = b.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = router.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 26).await.unwrap().to_vec();
    Resp { status, headers, bytes }
}

#[tokio::test]
#[ignore]
async fn asn_api_end_to_end() {
    let (r, pool) = setup().await;

    // --- Permissions: change_document required, inactive users never ---
    for t in [VIEWER, INACTIVE] {
        let s = call(&r, "GET", "/asn/presets", t, None).await.status;
        assert!(s == StatusCode::FORBIDDEN || s == StatusCode::UNAUTHORIZED, "{t}: {s}");
    }
    assert_eq!(call(&r, "GET", "/asn/presets", "wrongtoken", None).await.status, StatusCode::UNAUTHORIZED);
    let p = call(&r, "GET", "/asn/presets", EDITOR, None).await;
    assert_eq!(p.status, StatusCode::OK);
    let pj = p.json();
    assert_eq!(pj["default"], "avery-l4731");
    assert!(pj["items"].as_array().unwrap().len() >= 10);

    // --- next ASN: test data ASN 1001–1200, 1195–1200 in the trash ---
    let (live, any): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(max(archive_serial_number) FILTER (WHERE deleted_at IS NULL), 0)::bigint, COALESCE(max(archive_serial_number), 0)::bigint FROM documents_document",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let c = call(&r, "GET", "/asn/config", EDITOR, None).await;
    assert_eq!(c.status, StatusCode::OK);
    let cj = c.json();
    assert_eq!(cj["next_asn"]["documents_next"], live + 1, "same as v1 /api/documents/next_asn/");
    assert_eq!(cj["next_asn"]["suggested"], any + 1, "trashed documents keep their (unique) ASN");
    assert_eq!(cj["max_asn"], 2_147_483_647u64);
    assert_eq!(cj["barcode"]["prefix"], "ASN");
    assert_eq!(cj["barcode"]["separator_string"], "PATCHT");

    // --- preview with default start: no collision ---
    let pv = call(&r, "POST", "/asn/preview", EDITOR, Some(json!({"preset": "avery-4780", "skip": [0, 1]}))).await;
    assert_eq!(pv.status, StatusCode::OK, "{}", String::from_utf8_lossy(&pv.bytes));
    let pvj = pv.json();
    assert_eq!(pvj["first_asn"], any + 1);
    assert_eq!(pvj["count"], 38);
    assert_eq!(pvj["first_content"], format!("ASN{:05}", any + 1));
    assert!(pvj["svg"].as_str().unwrap().starts_with("<svg"));
    assert_eq!(pvj["collisions"]["documents_total"], 0);
    assert_eq!(pvj["skip"], json!([0, 1]));

    // --- preview over existing numbers: collisions ---
    let pv = call(&r, "POST", "/asn/preview", EDITOR, Some(json!({"preset": "avery-4780", "start": 1190, "count": 20}))).await;
    let pvj = pv.json();
    let existing: Vec<i64> = sqlx::query_scalar(
        "SELECT archive_serial_number::bigint FROM documents_document WHERE archive_serial_number BETWEEN 1190 AND 1209 ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(!existing.is_empty());
    assert_eq!(pvj["collisions"]["documents_total"], existing.len(), "incl. trash");
    assert_eq!(pvj["collisions"]["documents"], json!(existing));

    // --- invalid inputs ---
    let bad = call(&r, "POST", "/asn/preview", EDITOR, Some(json!({"preset": "nope"}))).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert!(bad.json()["error"]["fields"]["preset"].is_array());
    let bad = call(&r, "POST", "/asn/preview", EDITOR, Some(json!({"unknown": 1}))).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    let bad = call(&r, "POST", "/asn/preview?page=3", EDITOR, Some(json!({}))).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);

    // --- PDF over a collision: 409, with allow_collisions 200 ---
    let l = call(&r, "POST", "/asn/labels", EDITOR, Some(json!({"preset": "avery-4780", "start": 1190, "count": 20}))).await;
    assert_eq!(l.status, StatusCode::CONFLICT);
    assert_eq!(l.json()["error"]["code"], "asn_collision");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM paperrust_asn_print").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0, "nothing recorded on conflict");

    // --- PDF with default start: stored ---
    let l = call(&r, "POST", "/asn/labels", EDITOR, Some(json!({"preset": "avery-l4731", "code": "data_matrix"}))).await;
    assert_eq!(l.status, StatusCode::OK, "{}", String::from_utf8_lossy(&l.bytes));
    assert_eq!(l.headers["content-type"], "application/pdf");
    assert!(l.bytes.starts_with(b"%PDF-1.4"));
    let first = any + 1;
    let last = first + 188;
    assert_eq!(l.headers["x-paperrust-asn-first"], first.to_string().as_str());
    assert_eq!(l.headers["x-paperrust-asn-last"], last.to_string().as_str());
    assert!(l.headers["content-disposition"].to_str().unwrap().contains(&format!("asn-labels-ASN{first:05}-ASN{last:05}.pdf")));
    let print_id: i64 = l.headers["x-paperrust-print-id"].to_str().unwrap().parse().unwrap();

    // next suggestion lies after the printed range
    let cj = call(&r, "GET", "/asn/config", EDITOR, None).await.json();
    assert_eq!(cj["next_asn"]["suggested"], last + 1);
    assert_eq!(cj["next_asn"]["max_printed"], last);

    // overlapping print → 409 with the earlier range
    let l2 = call(&r, "POST", "/asn/labels", EDITOR2, Some(json!({"preset": "avery-4780", "start": last - 5, "count": 10}))).await;
    assert_eq!(l2.status, StatusCode::CONFLICT);
    let l2j = l2.json();
    assert_eq!(l2j["error"]["collisions"]["printed"][0]["id"], print_id);
    assert_eq!(l2j["error"]["collisions"]["printed"][0]["username"], "pl_asn_api_editor");
    // test print (record=false) without collision: not stored
    let t = call(&r, "POST", "/asn/labels?record=false", EDITOR2, Some(json!({"preset": "avery-4780", "outline": true, "count": 4}))).await;
    assert_eq!(t.status, StatusCode::OK);
    assert!(t.headers.get("x-paperrust-print-id").is_none());

    // --- History ---
    let h = call(&r, "GET", "/asn/printed", EDITOR2, None).await.json();
    let items = h["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["first_asn"], first);
    assert_eq!(items[0]["last_asn"], last);
    assert_eq!(items[0]["label_count"], 189);
    assert_eq!(items[0]["code_type"], "data_matrix");
    assert_eq!(items[0]["sheet"], "avery-l4731");
    assert_eq!(items[0]["used"], 0);

    // "used" counts documents that now have a number from the range
    let doc: i64 = sqlx::query_scalar("SELECT id::bigint FROM documents_document WHERE archive_serial_number IS NULL ORDER BY id LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE documents_document SET archive_serial_number = $1 WHERE id = $2")
        .bind(first as i32)
        .bind(doc as i32)
        .execute(&pool)
        .await
        .unwrap();
    let h = call(&r, "GET", "/asn/printed", EDITOR, None).await.json();
    assert_eq!(h["items"][0]["used"], 1);

    // --- Release: only creator or superuser ---
    assert_eq!(call(&r, "DELETE", &format!("/asn/printed/{print_id}"), EDITOR2, None).await.status, StatusCode::FORBIDDEN);
    assert_eq!(call(&r, "DELETE", &format!("/asn/printed/{print_id}"), EDITOR, None).await.status, StatusCode::NO_CONTENT);
    assert_eq!(call(&r, "DELETE", &format!("/asn/printed/{print_id}"), SU, None).await.status, StatusCode::NOT_FOUND);
    assert_eq!(call(&r, "GET", "/asn/printed", EDITOR, None).await.json()["items"], json!([]));

    // --- Separator sheet ---
    let s = call(&r, "POST", "/asn/separator", EDITOR, Some(json!({"lang": "de", "copies": 2}))).await;
    assert_eq!(s.status, StatusCode::OK);
    assert!(s.bytes.starts_with(b"%PDF"));
    assert!(String::from_utf8_lossy(&s.bytes).contains("/Count 2"));
    assert_eq!(call(&r, "POST", "/asn/separator", EDITOR, Some(json!({"value": "OTHER"}))).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(call(&r, "POST", "/asn/separator", SU, Some(json!({"value": "OTHER"}))).await.status, StatusCode::OK);
    assert_eq!(call(&r, "POST", "/asn/separator", VIEWER, Some(json!({}))).await.status, StatusCode::FORBIDDEN);

    // --- ApplicationConfiguration takes precedence over the environment (BarcodeConfig.__post_init__) ---
    sqlx::query("UPDATE paperless_applicationconfiguration SET barcode_asn_prefix = 'DOC-', barcode_string = 'SPLITHERE', barcodes_enabled = true, barcode_enable_asn = true")
        .execute(&pool)
        .await
        .unwrap();
    let cj = call(&r, "GET", "/asn/config", EDITOR, None).await.json();
    assert_eq!(cj["barcode"]["prefix"], "DOC-");
    assert_eq!(cj["barcode"]["separator_string"], "SPLITHERE");
    assert_eq!(cj["split_on_asn"], true);
    assert_eq!(cj["hints"], json!([]));
    let pvj = call(&r, "POST", "/asn/preview", EDITOR, Some(json!({"count": 1, "digits": 7}))).await.json();
    // the test document's ASN `first` (set above) is now the largest → suggestion first + 1
    assert_eq!(pvj["first_content"], format!("DOC-{:07}", first + 1));
    let pvj = call(&r, "POST", "/asn/preview", EDITOR, Some(json!({"count": 1, "prefix": "ASN"}))).await.json();
    assert!(pvj["warnings"].as_array().unwrap().iter().any(|w| w["code"] == "prefix_differs"));
}
