//! Document filter (pl-docfilter): arbitrary query strings through FilterSet, search and sorting
//! (SQL generation without DB, custom field DSL against a fake DB) as well as the date parsers.
#![no_main]
use std::collections::HashSet;

use libfuzzer_sys::fuzz_target;
include!("sancov_coff.rs");
use pl_docfilter::{CustomFieldDef, FilterContext, FilterDb, FilterError, QueryParams, User};

struct Db;

fn fields() -> Vec<CustomFieldDef> {
    let mk = |id: i64, name: &str, t: &str, extra: Option<serde_json::Value>| CustomFieldDef {
        id,
        name: name.into(),
        data_type: t.into(),
        extra_data: extra,
    };
    vec![
        mk(1, "s", "string", None),
        mk(2, "d", "date", None),
        mk(3, "i", "integer", None),
        mk(4, "f", "float", None),
        mk(5, "m", "monetary", None),
        mk(6, "b", "boolean", None),
        mk(7, "l", "documentlink", None),
        mk(8, "sel", "select", Some(serde_json::json!({"select_options": [{"id": "a1", "label": "A"}, {"id": "b2", "label": "B"}]}))),
        mk(9, "u", "url", None),
        mk(10, "t", "longtext", None),
    ]
}

impl FilterDb for Db {
    async fn custom_field_by_id(&self, id: i64) -> Result<Option<CustomFieldDef>, FilterError> {
        Ok(fields().into_iter().find(|f| f.id == id))
    }
    async fn custom_field_by_name(&self, name: &str) -> Result<Option<CustomFieldDef>, FilterError> {
        Ok(fields().into_iter().find(|f| f.name == name))
    }
    async fn custom_fields_lookup(&self, ids: &[i64], names: &[String]) -> Result<Vec<CustomFieldDef>, FilterError> {
        Ok(fields().into_iter().filter(|f| ids.contains(&f.id) || names.contains(&f.name)).collect())
    }
    async fn custom_fields_extra_data_icontains(&self, needle: &str) -> Result<Vec<CustomFieldDef>, FilterError> {
        Ok(fields()
            .into_iter()
            .filter(|f| f.extra_data.as_ref().is_some_and(|e| e.to_string().to_lowercase().contains(&needle.to_lowercase())))
            .collect())
    }
    async fn doc_link_instances(&self, _field_id: i64, document_ids: &[i64]) -> Result<Vec<(i64, serde_json::Value)>, FilterError> {
        Ok(document_ids.iter().take(3).map(|d| (*d, serde_json::json!([1, 2]))).collect())
    }
}

fn user() -> User {
    User {
        id: 3,
        username: "fuzz".into(),
        password: String::new(),
        first_name: String::new(),
        last_name: String::new(),
        email: String::new(),
        is_active: true,
        is_staff: false,
        is_superuser: false,
        last_login: None,
        date_joined: chrono::Utc::now(),
        groups: vec![1, 2],
        permissions: HashSet::from(["documents.view_document".to_string()]),
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(raw) = std::str::from_utf8(data) else { return };
    // Date parsers directly
    let _ = pl_docfilter::dates::drf_parse_date(raw);
    let _ = pl_docfilter::dates::django_form_date(raw);
    let _ = pl_docfilter::dates::django_parse_datetime(raw);
    let _ = pl_docfilter::dates::py_datetime_fromisoformat(raw);
    let _ = pl_docfilter::dates::iso_datetime_field(raw, chrono_tz::Europe::Zurich);
    let now = pl_docfilter::dates::now_local(chrono::Utc::now(), chrono_tz::Europe::Zurich);
    let _ = pl_docfilter::dates::parse_relative_date(raw, now, None);
    let _ = pl_docfilter::search::search_smart_split(raw);
    // whole filter chain
    let params = QueryParams::parse(raw);
    let u = user();
    let ctx = FilterContext::new(chrono_tz::Europe::Zurich);
    let mut q = sea_query::Query::select();
    q.column(sea_query::Asterisk).from(sea_query::Alias::new("documents_document"));
    futures::executor::block_on(async {
        if pl_docfilter::apply(&mut q, &params, &u, &ctx, &Db).await.is_ok() {
            let _ = pl_docfilter::apply_ordering(&mut q, params.get("ordering"), &Db).await;
            let _ = q.to_string(sea_query::PostgresQueryBuilder);
        }
    });
});
