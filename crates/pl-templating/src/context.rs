//! Document context for filename/storage path templates.
//!
//! upstream: src/documents/templating/filepath.py@v3.4.2 (create_dummy_document,
//! get_creation_date_context, get_added_date_context, get_basic_metadata_context,
//! get_safe_document_context, get_tags_context, get_custom_fields_context).

use chrono::{DateTime, NaiveDate, Utc};
use minijinja::value::Value;
use unicode_normalization::UnicodeNormalization;

use crate::error::TemplateError;
use crate::pathvalidate::sanitize_filename;
use crate::pystr::{name_stem, path_name, to_local};
use crate::pyvalue::{date_iso, date_value, datetime_value, placeholder, MatchingModel};
use crate::strftime::{strftime, TzInfo};

/// Linked object with a name (correspondent, document type).
#[derive(Debug, Clone, PartialEq)]
pub struct NamedRef {
    /// `None` only for the placeholder document of the validation.
    pub id: Option<i64>,
    pub name: String,
}

/// Storage path of the document.
#[derive(Debug, Clone, PartialEq)]
pub struct StoragePathRef {
    pub id: Option<i64>,
    pub name: String,
    /// Template (`StoragePath.path`).
    pub path: String,
}

/// Owner.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnerRef {
    pub id: i64,
    pub username: String,
}

/// Tag (order of the list = `document.tags.order_by("name")`).
#[derive(Debug, Clone, PartialEq)]
pub struct TagRef {
    pub id: Option<i64>,
    pub name: String,
}

/// Select option of a select field (`extra_data["select_options"]`).
#[derive(Debug, Clone, PartialEq)]
pub struct SelectOption {
    pub id: String,
    pub label: String,
}

/// Value of a `CustomFieldInstance` (`CustomFieldInstance.value`).
#[derive(Debug, Clone, PartialEq)]
pub enum CustomFieldValue {
    Null,
    /// string, url, longtext, monetary, select (Options-ID)
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Date(NaiveDate),
    /// documentlink
    IntList(Vec<i64>),
}

/// A `CustomFieldInstance` with its field.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomFieldInstance {
    pub field_name: String,
    /// `CustomField.data_type` ("string", "url", "date", "boolean", "integer", "float",
    /// "monetary", "documentlink", "select", "longtext").
    pub data_type: String,
    /// `extra_data["select_options"]` (select only; `None` = null/missing).
    pub select_options: Option<Vec<SelectOption>>,
    pub value: CustomFieldValue,
}

/// Everything that upstream puts from `Document` into the template context.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentContext {
    pub pk: i64,
    pub title: String,
    pub content: String,
    pub page_count: Option<i64>,
    pub created: NaiveDate,
    pub added: DateTime<Utc>,
    pub modified: DateTime<Utc>,
    pub archive_serial_number: Option<i64>,
    pub mime_type: String,
    pub checksum: String,
    pub archive_checksum: Option<String>,
    /// Stored filename (relative to ORIGINALS_DIR).
    pub filename: Option<String>,
    pub archive_filename: Option<String>,
    pub original_filename: Option<String>,
    pub owner: Option<OwnerRef>,
    pub correspondent: Option<NamedRef>,
    pub document_type: Option<NamedRef>,
    pub storage_path: Option<StoragePathRef>,
    /// Sorted by name, like `document.tags.order_by("name")`.
    pub tags: Vec<TagRef>,
    /// `CustomFieldInstance.global_objects.filter(document=document)` (incl. deleted).
    pub custom_fields: Vec<CustomFieldInstance>,
    /// `Document.file_type` (e.g. ".pdf"), see [`crate::default_file_extension`].
    pub file_type: String,
    /// Versions: root document (`root_document`), whose data fill the template.
    pub root_document: Option<Box<DocumentContext>>,
    /// `version_index` of a version.
    pub version_index: Option<i64>,
}

impl DocumentContext {
    /// Minimally populated document (for tests and callers that know only a few fields).
    pub fn new(pk: i64, title: impl Into<String>, mime_type: impl Into<String>) -> DocumentContext {
        let now = Utc::now();
        let mime: String = mime_type.into();
        DocumentContext {
            pk,
            title: title.into(),
            content: String::new(),
            page_count: None,
            created: now.date_naive(),
            added: now,
            modified: now,
            archive_serial_number: None,
            file_type: crate::default_file_extension(&mime).to_string(),
            mime_type: mime,
            checksum: String::new(),
            archive_checksum: None,
            filename: None,
            archive_filename: None,
            original_filename: None,
            owner: None,
            correspondent: None,
            document_type: None,
            storage_path: None,
            tags: Vec::new(),
            custom_fields: Vec::new(),
            root_document: None,
            version_index: None,
        }
    }
}

/// `created` is a `datetime` (timezone.now()) for the placeholder document, otherwise a `date`.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Created {
    Date(NaiveDate),
    DateTime(DateTime<Utc>),
}

fn nfc(s: &str) -> String {
    s.nfc().collect()
}

fn san(s: &str) -> String {
    sanitize_filename(&nfc(s), "-")
}

fn opt_str(v: &Option<String>) -> Value {
    v.clone().map(Value::from).unwrap_or(Value::from(()))
}

fn opt_i64(v: Option<i64>) -> Value {
    v.map(Value::from).unwrap_or(Value::from(()))
}

/// `get_custom_fields_context`.
pub(crate) fn custom_fields_context(fields: &[CustomFieldInstance]) -> Result<Value, TemplateError> {
    let mut entries: Vec<(String, Value)> = Vec::new();
    for fi in fields {
        let type_ = sanitize_filename(&fi.data_type, "-");
        let value = match &fi.value {
            CustomFieldValue::Null => Value::from(()),
            v if matches!(fi.data_type.as_str(), "monetary" | "string" | "url" | "longtext") => {
                let s = match v {
                    CustomFieldValue::Str(s) => s.clone(),
                    other => cf_plain(other).to_string(),
                };
                Value::from(san(&s))
            }
            CustomFieldValue::Str(id) if fi.data_type == "select" && fi.select_options.is_some() => {
                let opts = fi.select_options.as_deref().unwrap_or_default();
                match opts.iter().find(|o| &o.id == id) {
                    Some(o) => Value::from(san(&o.label)),
                    // upstream: next(...) without default -> StopIteration
                    None => {
                        return Err(TemplateError::Context(format!(
                            "select option {id:?} of custom field {:?} not found",
                            fi.field_name
                        )))
                    }
                }
            }
            other => cf_plain(other),
        };
        let map: Vec<(Value, Value)> = vec![(Value::from("type"), Value::from(type_)), (Value::from("value"), value)];
        let key = san(&fi.field_name);
        let obj = Value::from_iter(map);
        if let Some(e) = entries.iter_mut().find(|(k, _)| *k == key) {
            e.1 = obj;
        } else {
            entries.push((key, obj));
        }
    }
    Ok(Value::from_iter(entries.into_iter().map(|(k, v)| (Value::from(k), v))))
}

fn cf_plain(v: &CustomFieldValue) -> Value {
    match v {
        CustomFieldValue::Null => Value::from(()),
        CustomFieldValue::Str(s) => Value::from(s.clone()),
        CustomFieldValue::Int(i) => Value::from(*i),
        CustomFieldValue::Float(f) => Value::from(*f),
        CustomFieldValue::Bool(b) => Value::from(*b),
        CustomFieldValue::Date(d) => date_value(*d),
        CustomFieldValue::IntList(l) => Value::from(l.iter().map(|x| Value::from(*x)).collect::<Vec<_>>()),
    }
}

fn utc_value(dt: &DateTime<Utc>) -> Value {
    datetime_value(dt.naive_utc(), chrono::FixedOffset::east_opt(0))
}

/// `PurePath(original_filename).with_suffix("").name`.
fn original_name(original: &str) -> Result<String, TemplateError> {
    let name = path_name(original);
    if name.is_empty() {
        return Err(TemplateError::Context(format!("PurePosixPath({original:?}) has an empty name")));
    }
    Ok(nfc(name_stem(name)))
}

/// Context for `validate_filepath_template_and_render`.
pub(crate) fn filepath_context(
    doc: &DocumentContext,
    created: Created,
    tz: &chrono_tz::Tz,
) -> Result<Value, TemplateError> {
    let mut ctx: Vec<(&'static str, Value)> = Vec::new();

    // {"document": get_safe_document_context(document, tags)}
    let created_v = match created {
        Created::Date(d) => date_value(d),
        Created::DateTime(dt) => utc_value(&dt),
    };
    let tags: Vec<Value> = doc
        .tags
        .iter()
        .map(|t| Value::from_iter([("name", Value::from(t.name.clone())), ("id", opt_i64(t.id))]))
        .collect();
    let named = |r: &Option<NamedRef>| -> Value {
        match r {
            Some(r) => Value::from_object(MatchingModel { id: r.id, name: r.name.clone(), path: None }),
            None => Value::from(()),
        }
    };
    let document = Value::from_iter([
        ("id", Value::from(doc.pk)),
        ("pk", Value::from(doc.pk)),
        ("title", Value::from(doc.title.clone())),
        ("content", Value::from(doc.content.clone())),
        ("page_count", opt_i64(doc.page_count)),
        ("created", created_v),
        ("added", utc_value(&doc.added)),
        ("modified", utc_value(&doc.modified)),
        ("archive_serial_number", opt_i64(doc.archive_serial_number)),
        ("mime_type", Value::from(doc.mime_type.clone())),
        ("checksum", Value::from(doc.checksum.clone())),
        ("archive_checksum", opt_str(&doc.archive_checksum)),
        ("filename", opt_str(&doc.filename)),
        ("archive_filename", opt_str(&doc.archive_filename)),
        ("original_filename", opt_str(&doc.original_filename)),
        (
            "owner",
            match &doc.owner {
                Some(o) => Value::from_iter([("username", Value::from(o.username.clone())), ("id", Value::from(o.id))]),
                None => Value::from(()),
            },
        ),
        ("tags", Value::from(tags)),
        ("correspondent", named(&doc.correspondent)),
        ("document_type", named(&doc.document_type)),
        (
            "storage_path",
            match &doc.storage_path {
                Some(s) => Value::from_object(MatchingModel { id: s.id, name: s.name.clone(), path: Some(s.path.clone()) }),
                None => Value::from(()),
            },
        ),
    ]);
    ctx.push(("document", document));

    // get_basic_metadata_context(document, no_value_default=NO_VALUE_PLACEHOLDER)
    ctx.push(("title", Value::from(san(&doc.title))));
    ctx.push(("correspondent", doc.correspondent.as_ref().map(|c| Value::from(san(&c.name))).unwrap_or_else(placeholder)));
    ctx.push(("document_type", doc.document_type.as_ref().map(|c| Value::from(san(&c.name))).unwrap_or_else(placeholder)));
    ctx.push((
        "asn",
        match doc.archive_serial_number {
            Some(a) if a != 0 => Value::from(a.to_string()),
            _ => placeholder(),
        },
    ));
    ctx.push(("owner_username", doc.owner.as_ref().map(|o| Value::from(o.username.clone())).unwrap_or_else(placeholder)));
    ctx.push((
        "original_name",
        match doc.original_filename.as_deref() {
            Some(o) if !o.is_empty() => Value::from(original_name(o)?),
            _ => placeholder(),
        },
    ));
    ctx.push(("doc_pk", Value::from(format!("{:07}", doc.pk))));

    // get_creation_date_context(document)
    let (c_naive, c_tz, c_iso) = match created {
        Created::Date(d) => (d.and_time(chrono::NaiveTime::MIN), TzInfo::Naive, date_iso(d)),
        Created::DateTime(dt) => {
            let off = chrono::Offset::fix(&chrono::Utc);
            (
                dt.naive_utc(),
                TzInfo::Fixed(off),
                crate::pyvalue::PyDateTime { naive: dt.naive_utc(), offset: Some(off) }.isoformat("T"),
            )
        }
    };
    let cf = |f: &str| Value::from(strftime(&c_naive, c_tz, f));
    ctx.push(("created", Value::from(c_iso)));
    ctx.push(("created_year", cf("%Y")));
    ctx.push(("created_year_short", cf("%y")));
    ctx.push(("created_month", cf("%m")));
    ctx.push(("created_month_name", cf("%B")));
    ctx.push(("created_month_name_short", cf("%b")));
    ctx.push(("created_day", cf("%d")));

    // get_added_date_context(document): timezone.localdate(document.added)
    let local_added = to_local(&doc.added, tz).date_naive();
    let an = local_added.and_time(chrono::NaiveTime::MIN);
    let af = |f: &str| Value::from(strftime(&an, TzInfo::Naive, f));
    ctx.push(("added", Value::from(date_iso(local_added))));
    ctx.push(("added_year", af("%Y")));
    ctx.push(("added_year_short", af("%y")));
    ctx.push(("added_month", af("%m")));
    ctx.push(("added_month_name", af("%B")));
    ctx.push(("added_month_name_short", af("%b")));
    ctx.push(("added_day", af("%d")));

    // get_tags_context(tags)
    let mut names: Vec<String> = doc.tags.iter().map(|t| nfc(&t.name)).collect();
    let tag_name_list: Vec<Value> = names.iter().map(|n| Value::from(n.clone())).collect();
    names.sort();
    ctx.push(("tag_list", Value::from(sanitize_filename(&names.join(","), "-"))));
    ctx.push(("tag_name_list", Value::from(tag_name_list)));

    // get_custom_fields_context(custom_fields)
    ctx.push(("custom_fields", custom_fields_context(&doc.custom_fields)?));

    Ok(Value::from_iter(ctx))
}

/// `create_dummy_document()` with tags and custom field of the validation.
pub(crate) fn dummy_document() -> (DocumentContext, Created) {
    let now = Utc::now();
    let doc = DocumentContext {
        pk: 1,
        title: "Sample Title".into(),
        content: "This is some sample document content.".into(),
        page_count: Some(5),
        created: now.date_naive(),
        added: now,
        modified: now,
        archive_serial_number: Some(12345),
        mime_type: "application/pdf".into(),
        checksum: "dummychecksum12345678901234567890123456789012".into(),
        archive_checksum: Some("dummyarchivechecksum123456789012345678901234".into()),
        filename: Some("/dummy/filename.pdf".into()),
        archive_filename: Some("/dummy/archive_filename.pdf".into()),
        original_filename: Some("original_file.pdf".into()),
        owner: None,
        correspondent: Some(NamedRef { id: None, name: "Sample Correspondent".into() }),
        document_type: Some(NamedRef { id: None, name: "Sample Type".into() }),
        storage_path: Some(StoragePathRef { id: None, name: String::new(), path: "/dummy/path".into() }),
        tags: vec![
            TagRef { id: None, name: "Test Tag 1".into() },
            TagRef { id: None, name: "Another Test Tag".into() },
        ],
        custom_fields: vec![CustomFieldInstance {
            field_name: "Text Custom Field".into(),
            data_type: "string".into(),
            select_options: None,
            value: CustomFieldValue::Str("Some String Text".into()),
        }],
        file_type: ".pdf".into(),
        root_document: None,
        version_index: None,
    };
    (doc, Created::DateTime(now))
}
