//! Tests against the reference environment (reference/README.md): PostgreSQL 127.0.0.1:54329 and
//! Django 127.0.0.1:8000. Read-only or in a transaction with rollback; only GET requests and
//! POST requests that fail at validation go to Django (no writes).
//!
//! Run: `cargo test -p pl-workflows -j 2 -- --ignored`

use pl_workflows::drf::{Ctx, VErr};
use pl_workflows::schema::*;
use pl_workflows::{load, serializers, write};
use sea_query::{Alias, Order};
use serde_json::{Map, Value, json};
use sqlx::{Connection, PgConnection};

const DB: &str = "postgres://paperless:paperless@127.0.0.1:54329/paperless";
const DJANGO: &str = "http://127.0.0.1:8000";
const ADMIN_TOKEN: &str = "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001";

async fn conn() -> PgConnection {
    PgConnection::connect(DB).await.expect("reference db")
}

async fn django(method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
    let client = reqwest::Client::new();
    let url = format!("{DJANGO}{path}");
    let mut req = match method {
        "GET" => client.get(&url),
        "POST" => client.post(&url),
        "PATCH" => client.patch(&url),
        "PUT" => client.put(&url),
        other => panic!("method {other}"),
    }
    .header("Authorization", format!("Token {ADMIN_TOKEN}"))
    .header("Accept", "application/json; version=10");
    if let Some(b) = body {
        req = req.json(b);
    }
    let resp = req.send().await.expect("django");
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

#[tokio::test]
#[ignore = "braucht Referenz-DB und Django (reference/README.md)"]
async fn render_matches_django() {
    let mut c = conn().await;
    for (path, table, kind) in [
        ("/api/workflow_triggers/", T_TRIGGER, 0),
        ("/api/workflow_actions/", T_ACTION, 1),
        ("/api/workflows/", T_WORKFLOW, 2),
    ] {
        let (st, dj) = django("GET", &format!("{path}?page_size=1000"), None).await;
        assert_eq!(st, 200);
        let mut sel = load::select_rows(table);
        if kind == 2 {
            sel.order_by((Alias::new(T_WORKFLOW), Alias::new("order")), Order::Asc);
        }
        let rows = load::fetch_rows(&mut c, &sel).await.unwrap();
        let mine: Vec<Value> = match kind {
            0 => load::render_triggers(&mut c, &rows).await,
            1 => load::render_actions(&mut c, &rows).await,
            _ => load::render_workflows(&mut c, &rows, true).await,
        }
        .unwrap()
        .into_iter()
        .map(Value::Object)
        .collect();
        assert_eq!(dj["results"], Value::Array(mine.clone()), "{path}");
        // Detail views
        for item in &mine {
            let id = item["id"].as_i64().unwrap();
            let (st, d) = django("GET", &format!("{path}{id}/"), None).await;
            assert_eq!(st, 200);
            assert_eq!(&d, item, "{path}{id}/");
        }
    }
}

fn detail(r: Result<impl std::fmt::Debug, VErr>) -> Value {
    match r {
        Err(VErr::Detail(d)) => d,
        Err(VErr::Server(s)) => json!({"__server": s}),
        Ok(v) => json!({"__ok": format!("{v:?}")}),
    }
}

/// Invalid inputs: Rust validation == Django response (400). Django writes nothing in the process.
#[tokio::test]
#[ignore = "braucht Referenz-DB und Django (reference/README.md)"]
async fn validation_matches_django() {
    let mut c = conn().await;
    let triggers = [
        json!({"sources": [2]}),
        json!({"type": 1, "sources": [2]}),
        json!({"type": 1, "filter_filename": ""}),
        json!({"type": "x", "sources": "1", "filter_has_tags": [999999], "filter_has_correspondent": "abc",
               "schedule_recurring_interval_days": 0, "matching_algorithm": 6, "is_insensitive": "maybe",
               "filter_mailrule": true, "schedule_date_field": "foo", "match": [5], "filter_path": "x".repeat(300),
               "schedule_offset_days": 1.5, "filter_has_not_tags": {"1": 1}, "filter_has_all_tags": ["a", null],
               "filter_has_storage_path": 99999999999i64, "id": "z"}),
        json!({"type": 2, "filter_custom_field_query": "[not json"}),
        json!({"type": 2, "filter_custom_field_query": "[\"AND\", [[999999, \"exact\", 1], \"x\"]]"}),
        json!({"type": 2, "filter_custom_field_query": "[\"XOR\", []]"}),
        json!({"type": 2, "filter_custom_field_query": "[\"OR\", []]"}),
        json!({"type": 2, "filter_custom_field_query": "[\"NOT\", [5, \"gt\", \"x\"]]"}),
        json!({"type": 2, "filter_custom_field_query": "[\"AND\", [[5, \"in\", []], [5, \"range\", [1]], [3, \"gt\", \"no date\"], [3, \"year__gt\", \"x\"]]]"}),
        json!({"type": 2, "filter_custom_field_query": "[\"AND\", [[4, \"gt\", 1], [1, \"icontains\", null], [2, \"exact\", \"\"], [10, \"exact\", \"Nope-very-long-option\"], [7, \"gt\", \"EURx\"], [6, \"exact\", \"abc\"], [9, \"contains\", \"1\"]]]"}),
        json!({"type": 2, "filter_custom_field_query": "[\"Text\", \"foo__exact\", \"x\"]"}),
        json!({"type": 2, "filter_custom_field_query": "[\"nofield\", \"exact\", \"x\"]"}),
        json!({"type": 2, "filter_custom_field_query": "[1.5, \"exact\", \"x\"]"}),
        json!({"type": 2, "filter_custom_field_query": "{\"a\": 1}"}),
        json!({"type": 2, "filter_custom_field_query": "[\"NOT\", [\"NOT\", [\"NOT\", [\"NOT\", [\"NOT\", [\"NOT\", [\"NOT\", [\"NOT\", [\"NOT\", [\"NOT\", [1, \"exists\", true]]]]]]]]]]]"}),
        json!([1, 2]),
        json!("text"),
    ];
    for t in &triggers {
        let (st, dj) = django("POST", "/api/workflow_triggers/", Some(t)).await;
        assert_eq!(st, 400, "django accepted {t}");
        let mine = detail(serializers::validate_trigger(&mut c, Ctx { partial: false }, t, None).await);
        assert_eq!(dj, mine, "trigger {t}");
    }
    let actions = [
        json!({"assign_title": "{{created_year}"}),
        json!({"assign_title": "x\n{{ created_year | foo }}"}),
        json!({"assign_title": "{{ creation_year }}"}),
        json!({"type": 3}),
        json!({"type": 4}),
        json!({"type": 3, "email": {"subject": ""}}),
        json!({"type": 3, "email": "x"}),
        json!({"type": 4, "webhook": {"url": "ftp://example.com/x"}}),
        json!({"type": 4, "webhook": {"url": "example.com"}}),
        json!({"type": 5}),
        json!({"type": 5, "passwords": ""}),
        json!({"type": 5, "passwords": ["", "pw"]}),
        json!({"type": 5, "passwords": [1]}),
        json!({"type": 8, "ai_suggestion_fields": []}),
        json!({"type": 8, "ai_suggestion_fields": ["title", "nope"]}),
        json!({"type": 0, "assign_tags": "1", "assign_owner": 999999, "assign_view_users": [999999],
               "remove_all_tags": 2, "assign_correspondent": [1], "email": null}),
    ];
    for a in &actions {
        let (st, dj) = django("POST", "/api/workflow_actions/", Some(a)).await;
        assert_eq!(st, 400, "django accepted {a}");
        let mine = detail(serializers::validate_action(&mut c, Ctx { partial: false }, a).await);
        assert_eq!(dj, mine, "action {a}");
    }
    let workflows = [
        json!({}),
        json!({"name": "", "order": "x", "enabled": "?", "triggers": [{"type": 2}, {"type": 9}], "actions": {"a": 1}}),
        json!({"name": "Invoice intake", "triggers": [], "actions": []}),
        json!({"name": "ocr", "triggers": [{"type": 2}], "actions": [{"type": 7}]}),
        json!({"name": "ai", "triggers": [{"type": 1, "filter_filename": "*"}], "actions": [{"type": 8, "ai_suggestion_fields": ["title"]}]}),
        json!({"name": "n", "triggers": null, "actions": [null, 5]}),
        json!({"name": "n", "triggers": [{"type": 1}], "actions": [{"assign_title": "{{ x }}"}]}),
    ];
    for w in &workflows {
        let (st, dj) = django("POST", "/api/workflows/", Some(w)).await;
        assert_eq!(st, 400, "django accepted {w}");
        let mine = detail(serializers::validate_workflow(&mut c, Ctx { partial: false }, w, None).await);
        assert_eq!(dj, mine, "workflow {w}");
    }
    // PATCH validation (partial) against existing objects, without success (validation error)
    let (st, dj) = django("PATCH", "/api/workflows/1/", Some(&json!({"name": "Invoice intake 2x", "order": "x"}))).await;
    assert_eq!(st, 400);
    let mine = detail(
        serializers::validate_workflow(&mut c, Ctx { partial: true }, &json!({"name": "Invoice intake 2x", "order": "x"}), Some(1)).await,
    );
    assert_eq!(dj, mine);
}

async fn get(c: &mut PgConnection, table: &'static str, id: i64) -> Map<String, Value> {
    load::get_row(c, table, id).await.unwrap().expect("row")
}

async fn count(c: &mut PgConnection, table: &'static str) -> i64 {
    load::count(c, table).await.unwrap()
}

/// Port of the write tests from `documents/tests/test_api_workflows.py` (in a transaction).
#[tokio::test]
#[ignore = "braucht Referenz-DB (reference/README.md)"]
async fn write_flows_rollback() {
    let mut root = conn().await;
    let mut tx = root.begin().await.unwrap();
    let c: &mut PgConnection = &mut tx;
    let ctx = Ctx { partial: false };
    let wf_before = count(c, T_WORKFLOW).await;

    // test_api_create_workflow_nested
    let input = json!({
        "name": "Workflow 2", "order": 1,
        "triggers": [{"sources": [2], "type": 1, "filter_filename": "*", "filter_path": "*/samples/*",
                      "filter_has_tags": [1], "filter_has_all_tags": [2], "filter_has_not_tags": [3],
                      "filter_has_any_correspondents": [1], "filter_has_correspondent": 1,
                      "filter_custom_field_query": "[\"AND\", [[1, \"exact\", \"value\"]]]"}],
        "actions": [
            {"assign_title": "Action Title", "assign_tags": [2], "assign_owner": 3, "assign_view_users": [4, 3],
             "assign_custom_fields": [5, 1]},
            {"type": 2, "remove_tags": [3], "remove_owners": [3]},
            {"id": 1, "type": 3, "email": {"subject": "S", "body": "B", "to": "a@example.com"}},
            {"type": 4, "webhook": {"url": "https://examplewithouttld:3000/path", "params": {"a": "b"}}}
        ]
    });
    let v = serializers::validate_workflow(c, ctx, &input, None).await.unwrap();
    let id = write::workflow_create(c, &v).await.unwrap();
    assert_eq!(count(c, T_WORKFLOW).await, wf_before + 1);
    let row = get(c, T_WORKFLOW, id).await;
    let out = load::render_workflows(c, &[row], false).await.unwrap();
    let wf = &out[0];
    assert_eq!(wf["name"], json!("Workflow 2"));
    let t = &wf["triggers"][0];
    assert_eq!(t["sources"], json!([2]));
    assert_eq!(t["filter_has_tags"], json!([1]));
    assert_eq!(t["filter_custom_field_query"], json!("[\"AND\", [[1, \"exact\", \"value\"]]]"));
    let acts = wf["actions"].as_array().unwrap();
    assert_eq!(acts.len(), 4);
    // Action id is removed on create → new action, seed action 1 unchanged
    assert!(acts.iter().all(|a| a["id"] != json!(1)));
    let email_action = acts.iter().find(|a| a["type"] == json!(3)).unwrap();
    assert_eq!(email_action["email"]["to"], json!("a@example.com"));
    let first = acts.iter().find(|a| a["assign_title"] == json!("Action Title")).unwrap();
    assert_eq!(first["assign_custom_fields"], json!([1, 5]), "ORDER BY customfield.created");

    // test_api_update_workflow_no_trigger_actions: PATCH without triggers/actions
    let row = get(c, T_WORKFLOW, id).await;
    let v = serializers::validate_workflow(c, Ctx { partial: true }, &json!({"name": "Workflow Updated", "order": 1}), Some(id))
        .await
        .unwrap();
    write::workflow_update(c, &row, &v).await.unwrap();
    let row = get(c, T_WORKFLOW, id).await;
    let out = load::render_workflows(c, std::slice::from_ref(&row), true).await.unwrap();
    assert_eq!(out[0]["actions"].as_array().unwrap().len(), 4);
    // actions: [] → actions removed and orphaned ones deleted (prune)
    let act_before = count(c, T_ACTION).await;
    let v = serializers::validate_workflow(c, Ctx { partial: true }, &json!({"actions": []}), Some(id)).await.unwrap();
    write::workflow_update(c, &row, &v).await.unwrap();
    assert_eq!(count(c, T_ACTION).await, act_before - 4);
    let emails_orphan = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM documents_workflowactionemail e WHERE NOT EXISTS (SELECT 1 FROM documents_workflowaction a WHERE a.email_id = e.id)",
    )
    .fetch_one(&mut *c)
    .await
    .unwrap();
    assert_eq!(emails_orphan, 0);

    // test_api_partial_update_cannot_remove_remote_ocr_trigger variant: Remote OCR needs Consumption
    let v = serializers::validate_workflow(c, Ctx { partial: true }, &json!({"actions": [{"type": 7}]}), Some(id)).await.unwrap();
    write::workflow_update(c, &row, &v).await.unwrap();
    let e = detail(serializers::validate_workflow(c, Ctx { partial: true }, &json!({"triggers": [{"type": 3}]}), Some(id)).await);
    assert_eq!(e, json!({"non_field_errors": ["Remote OCR actions require a consumption started trigger"]}));

    // PATCH with an incomplete email sub-object → error of the re-validation at top level
    let v = serializers::validate_workflow(c, Ctx { partial: true }, &json!({"actions": [{"type": 3, "email": {"subject": "x"}}]}), Some(id))
        .await
        .unwrap();
    let e = detail(write::workflow_update(c, &row, &v).await);
    assert_eq!(e, json!({"body": ["This field is required."], "to": ["This field is required."]}));

    // Trigger directly: create, PATCH, delete
    let tattrs = serializers::validate_trigger(c, ctx, &json!({"type": 1, "sources": [2], "filter_filename": "*", "filter_path": ""}), None)
        .await
        .unwrap();
    assert_eq!(tattrs["filter_path"], Value::Null);
    let tid = write::model_create(c, true, &tattrs).await.unwrap();
    let trow = get(c, T_TRIGGER, tid).await;
    assert_eq!(trow["sources"], json!("2"));
    let p = serializers::validate_trigger(c, Ctx { partial: true }, &json!({"filter_filename": "patched.pdf"}), Some(1)).await.unwrap();
    write::model_update(c, true, &trow, &p).await.unwrap();
    assert_eq!(get(c, T_TRIGGER, tid).await["filter_filename"], json!("patched.pdf"));
    write::delete_triggers(c, &[tid]).await.unwrap();
    assert!(load::get_row(c, T_TRIGGER, tid).await.unwrap().is_none());

    // Action with a nested email object via /api/workflow_actions/ → 500 (DRF AssertionError)
    let a = serializers::validate_action(c, ctx, &json!({"type": 3, "email": {"subject": "s", "body": "b", "to": "t"}})).await.unwrap();
    assert!(matches!(write::model_create(c, false, &a).await, Err(VErr::Server(_))));

    // Delete workflow
    write::delete_workflow(c, id).await.unwrap();
    assert_eq!(count(c, T_WORKFLOW).await, wf_before);

    tx.rollback().await.unwrap();
}
