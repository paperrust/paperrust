#![allow(dead_code)]

use chrono::{TimeZone, Utc};
use pl_docfilter::{CustomFieldDef, FilterContext, FilterDb, FilterError, QueryParams, User};
use sea_query::{Alias, PostgresQueryBuilder, Query, SelectStatement};
use serde_json::{Value, json};

/// In-memory replacement for the database queries of the filters.
#[derive(Default)]
pub struct MockDb {
    pub fields: Vec<CustomFieldDef>,
    /// (document_id, field_id, value_document_ids)
    pub links: Vec<(i64, i64, Value)>,
}

impl FilterDb for MockDb {
    async fn custom_field_by_id(&self, id: i64) -> Result<Option<CustomFieldDef>, FilterError> {
        Ok(self.fields.iter().find(|f| f.id == id).cloned())
    }
    async fn custom_field_by_name(&self, name: &str) -> Result<Option<CustomFieldDef>, FilterError> {
        Ok(self.fields.iter().find(|f| f.name == name).cloned())
    }
    async fn custom_fields_lookup(&self, ids: &[i64], names: &[String]) -> Result<Vec<CustomFieldDef>, FilterError> {
        Ok(self.fields.iter().filter(|f| ids.contains(&f.id) || names.contains(&f.name)).cloned().collect())
    }
    async fn custom_fields_extra_data_icontains(&self, needle: &str) -> Result<Vec<CustomFieldDef>, FilterError> {
        let n = needle.to_uppercase();
        Ok(self
            .fields
            .iter()
            .filter(|f| f.extra_data.as_ref().map(|e| e.to_string().to_uppercase().contains(&n)).unwrap_or(false))
            .cloned()
            .collect())
    }
    async fn doc_link_instances(&self, field_id: i64, document_ids: &[i64]) -> Result<Vec<(i64, Value)>, FilterError> {
        Ok(self.links.iter().filter(|(d, f, _)| *f == field_id && document_ids.contains(d)).map(|(d, _, v)| (*d, v.clone())).collect())
    }
}

/// One field per data type, named `<type>_field`, IDs 1.. (like the upstream test setup).
pub fn upstream_fields() -> MockDb {
    let types = ["string", "url", "date", "boolean", "integer", "float", "monetary", "documentlink", "select", "longtext"];
    let mut fields = Vec::new();
    for (i, t) in types.iter().enumerate() {
        fields.push(CustomFieldDef {
            id: i as i64 + 1,
            name: format!("{t}_field"),
            data_type: t.to_string(),
            extra_data: if *t == "select" {
                Some(json!({"select_options": [
                    {"label": "A", "id": "abc-123"},
                    {"label": "B", "id": "def-456"},
                    {"label": "C", "id": "ghi-789"}
                ]}))
            } else {
                None
            },
        });
    }
    MockDb { fields, links: Vec::new() }
}

pub fn mk_user(id: i64, is_active: bool, is_superuser: bool) -> User {
    User {
        id,
        username: format!("user{id}"),
        password: String::new(),
        first_name: String::new(),
        last_name: String::new(),
        email: String::new(),
        is_active,
        is_staff: false,
        is_superuser,
        last_login: None,
        date_joined: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        groups: vec![],
        permissions: Default::default(),
    }
}

pub fn superuser() -> User {
    mk_user(1, true, true)
}

pub fn regular_user() -> User {
    mk_user(7, true, false)
}

pub fn ctx() -> FilterContext {
    FilterContext {
        tz: "Europe/Berlin".parse().unwrap(),
        now: Utc.with_ymd_and_hms(2024, 8, 22, 12, 0, 0).unwrap(),
        date_parser_languages: None,
    }
}

pub fn base() -> SelectStatement {
    Query::select().column((Alias::new("documents_document"), Alias::new("id"))).from(Alias::new("documents_document")).to_owned()
}

pub const BASE_SQL: &str = r#"SELECT "documents_document"."id" FROM "documents_document""#;

/// Applies filters and returns the WHERE part (without the base SELECT).
pub fn filter_sql_with(query: &str, user: &User, c: &FilterContext, db: &MockDb) -> Result<String, FilterError> {
    let mut q = base();
    let params = QueryParams::parse(query);
    futures::executor::block_on(pl_docfilter::apply(&mut q, &params, user, c, db))?;
    let sql = q.to_string(PostgresQueryBuilder);
    Ok(sql.strip_prefix(BASE_SQL).unwrap_or(&sql).trim().to_string())
}

pub fn filter_sql(query: &str) -> Result<String, FilterError> {
    filter_sql_with(query, &superuser(), &ctx(), &upstream_fields())
}

pub fn order_sql_with(ordering: Option<&str>, db: &MockDb) -> Result<String, FilterError> {
    let mut q = base();
    futures::executor::block_on(pl_docfilter::apply_ordering(&mut q, ordering, db))?;
    let sql = q.to_string(PostgresQueryBuilder);
    Ok(sql.strip_prefix(BASE_SQL).unwrap_or(&sql).trim().to_string())
}

pub fn validation(r: Result<String, FilterError>) -> Value {
    match r {
        Err(FilterError::Validation(v)) => v,
        other => panic!("expected validation error, got {other:?}"),
    }
}

pub fn is_internal(r: &Result<String, FilterError>) -> bool {
    matches!(r, Err(FilterError::Internal(_)))
}

/// URL-encodes a custom_field_query value.
pub fn cfq(json: &str) -> String {
    let enc: String = url::form_urlencoded::byte_serialize(json.as_bytes()).collect();
    format!("custom_field_query={enc}")
}

/// Follows the error path like `_assert_validation_error` in upstream.
pub fn error_path(v: &Value) -> (Vec<String>, String) {
    let mut path = Vec::new();
    let mut d = v;
    loop {
        match d {
            Value::Object(m) => {
                let (k, next) = m.iter().next().unwrap();
                path.push(k.clone());
                d = next;
            }
            Value::Array(a) => {
                let msgs: Vec<String> = a.iter().map(|x| x.as_str().unwrap().to_string()).collect();
                return (path, msgs.join(" "));
            }
            other => panic!("unexpected {other}"),
        }
    }
}
