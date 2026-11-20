//! Embedded exporter descriptor (from Django `_meta`, `tests/parity/pl-cli/gen_descriptor.py`).

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct Descriptor {
    pub version: String,
    pub excluded_usernames: Vec<String>,
    pub crypt_fields: Vec<CryptFields>,
    pub crypt_defaults: CryptDefaults,
    pub keys: Vec<ManifestKey>,
    pub split: BTreeMap<String, SplitQuery>,
    pub documents_stream_sql: String,
    pub models: BTreeMap<String, Model>,
    pub fk_refs: Vec<FkRef>,
}

#[derive(Debug, Deserialize)]
pub struct CryptFields {
    pub exporter_key: String,
    pub model_name: String,
    pub fields: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct CryptDefaults {
    pub algorithm: String,
    pub iterations: u32,
    pub key_size: usize,
    pub salt_size: usize,
}

#[derive(Debug, Deserialize)]
pub struct ManifestKey {
    pub key: String,
    pub model: String,
    pub sql: String,
    pub params: Vec<Value>,
    pub pk_sql: String,
    pub pk_params: Vec<Value>,
    pub audit_only: bool,
}

#[derive(Debug, Deserialize)]
pub struct SplitQuery {
    pub model: String,
    pub sql: String,
    pub params: Vec<Value>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Default_ {
    pub kind: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub value: Option<Value>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Field {
    pub name: String,
    pub attname: String,
    pub column: String,
    pub kind: String,
    pub internal_type: String,
    pub db_type: String,
    pub serialize: bool,
    pub primary_key: bool,
    pub generated: bool,
    pub auto_now: bool,
    pub auto_now_add: bool,
    pub null: bool,
    pub default: Default_,
    pub related_model: Option<String>,
    pub on_delete: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct M2m {
    pub name: String,
    pub serialize: bool,
    pub auto_created: bool,
    pub through_table: String,
    pub src_column: String,
    pub dst_column: String,
    pub related_model: String,
    pub related_sql: String,
    pub related_params: Vec<Value>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Model {
    pub label: String,
    #[serde(default)]
    pub object_name: String,
    pub table: String,
    pub pk_column: String,
    pub pk_attname: String,
    pub pk_internal_type: String,
    pub fields: Vec<Field>,
    pub m2m: Vec<M2m>,
    pub has_global_objects: bool,
}

impl Model {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name || f.attname == name)
    }

    pub fn m2m_field(&self, name: &str) -> Option<&M2m> {
        self.m2m.iter().find(|f| f.name == name)
    }

    pub fn pk_field(&self) -> Option<&Field> {
        self.fields.iter().find(|f| f.primary_key)
    }

    /// AutoField/BigAutoField primary key (reset the sequence after the import).
    pub fn has_auto_pk(&self) -> bool {
        matches!(self.pk_internal_type.as_str(), "AutoField" | "BigAutoField" | "SmallAutoField")
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct FkRef {
    pub table: String,
    pub column: String,
    pub pk_column: String,
    pub target: String,
    pub on_delete: Option<String>,
}

pub fn get() -> &'static Descriptor {
    static D: OnceLock<Descriptor> = OnceLock::new();
    D.get_or_init(|| match serde_json::from_str(crate::baseline::EXPORT_DESCRIPTOR_JSON) {
        Ok(d) => d,
        // The descriptor is embedded and checked in the tests; an error here is a build error.
        Err(e) => panic!("embedded export descriptor invalid: {e}"),
    })
}

/// Parameters of the descriptor (JSON values) bound as text.
pub fn bind_params<'q>(
    mut q: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    params: &'q [Value],
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    for p in params {
        q = match p {
            Value::Null => q.bind(None::<String>),
            Value::Bool(b) => q.bind(*b),
            Value::Number(n) => match n.as_i64() {
                Some(i) => q.bind(i32::try_from(i).unwrap_or(i32::MAX)),
                None => q.bind(n.to_string()),
            },
            Value::String(s) => q.bind(s.as_str()),
            other => q.bind(other.to_string()),
        };
    }
    q
}

#[cfg(test)]
mod tests {
    #[test]
    fn descriptor_loads() {
        let d = super::get();
        assert_eq!(d.version, "3.4.2");
        assert_eq!(d.keys.len(), 33);
        assert_eq!(d.keys[0].key, "correspondents");
        let doc = &d.models["documents.document"];
        assert_eq!(doc.table, "documents_document");
        assert!(doc.field("content_length").is_some_and(|f| f.generated && !f.serialize));
        assert_eq!(doc.m2m[0].name, "tags");
    }
}
