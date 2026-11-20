//! Request bodies of `acknowledge` and `run` (DRF parsers + serializer fields).
//!
//! upstream: rest_framework/request.py@3.18.3 (`Request._parse`), rest_framework/parsers.py
//! (`JSONParser`, `FormParser`, `MultiPartParser`), rest_framework/fields.py (`ListField`,
//! `IntegerField`, `BooleanField`, `ChoiceField`), src/documents/serialisers.py@v3.4.2
//! (`AcknowledgeTasksViewSerializer`, `RunTaskSerializer`).

use axum::extract::Request;
use axum::http::header::CONTENT_TYPE;
use pl_core::{ApiError, ValidationErrors};
use serde_json::{Map, Value};

use crate::error::{Error, Result};

/// `request.data`.
#[derive(Debug, Clone, PartialEq)]
pub enum Data {
    /// JSON (any value; usually an object).
    Json(Value),
    /// `QueryDict` from a form or multipart body.
    Form(Vec<(String, String)>),
}

/// Parse the body like DRF with the default parsers (JSON, form, multipart).
pub async fn parse_request(req: Request) -> Result<Data> {
    let ct = req.headers().get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_string);
    // Django DATA_UPLOAD_MAX_MEMORY_SIZE (too large -> 400 like RequestDataTooBig).
    let bytes = pl_core::body::read_body(req.into_body()).await.map_err(Error::Api)?;
    parse_bytes(ct.as_deref(), &bytes)
}

/// Body bytes + `Content-Type` → `request.data`.
pub fn parse_bytes(content_type: Option<&str>, bytes: &[u8]) -> Result<Data> {
    let media = content_type.unwrap_or("");
    let main = media.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let is_form = main == "application/x-www-form-urlencoded" || main == "multipart/form-data";
    if bytes.is_empty() {
        // stream is None (CONTENT_LENGTH 0): QueryDict for form types, {} otherwise
        return Ok(if is_form { Data::Form(Vec::new()) } else { Data::Json(Value::Object(Map::new())) });
    }
    match main.as_str() {
        "application/json" => match serde_json::from_slice::<Value>(bytes) {
            Ok(v) => Ok(Data::Json(v)),
            Err(e) => Err(Error::Api(ApiError::json_parse_error(
                &crate::pyjson::decode_error(bytes).unwrap_or_else(|| python_json_error(&e)),
            ))),
        },
        "application/x-www-form-urlencoded" => {
            let s = String::from_utf8_lossy(bytes);
            Ok(Data::Form(pl_core::urlenc::parse_qsl(&s, true)))
        }
        "multipart/form-data" => Ok(Data::Form(parse_multipart(media, bytes))),
        _ => Err(Error::Api(ApiError::unsupported_media_type(media))),
    }
}

/// Approximation of Python's `json.JSONDecodeError` message (`msg: line L column C (char N)`).
fn python_json_error(e: &serde_json::Error) -> String {
    let msg = match e.classify() {
        serde_json::error::Category::Eof => "Expecting value",
        _ => "Expecting value",
    };
    format!("{msg}: line {} column {} (char {})", e.line(), e.column().max(1), e.column().saturating_sub(1))
}

/// Minimal multipart reader for text fields (files are ignored; these views take none).
fn parse_multipart(content_type: &str, body: &[u8]) -> Vec<(String, String)> {
    let Some(boundary) = content_type
        .split(';')
        .map(str::trim)
        .find_map(|p| p.strip_prefix("boundary="))
        .map(|b| b.trim_matches('"').to_string())
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(body);
    let delim = format!("--{boundary}");
    let mut out = Vec::new();
    for part in text.split(&delim).skip(1) {
        if part.starts_with("--") {
            break;
        }
        let part = part.strip_prefix("\r\n").unwrap_or(part);
        let Some((head, value)) = part.split_once("\r\n\r\n") else { continue };
        let value = value.strip_suffix("\r\n").unwrap_or(value);
        let disp = head.lines().find(|l| l.to_ascii_lowercase().starts_with("content-disposition:"));
        let Some(disp) = disp else { continue };
        if disp.contains("filename=") {
            continue;
        }
        let name = disp.split(';').map(str::trim).find_map(|p| p.strip_prefix("name=")).map(|n| n.trim_matches('"'));
        if let Some(n) = name {
            out.push((n.to_string(), value.to_string()));
        }
    }
    out
}

/// Python `type(x).__name__` of a JSON value.
fn py_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `IntegerField.to_internal_value` (`int(re.sub(r'\.0*\s*$', '', str(data)))`).
fn integer_field(v: &Value) -> std::result::Result<i64, &'static str> {
    if let Value::String(s) = v
        && s.chars().count() > 1000
    {
        return Err("String value too large.");
    }
    let s = match v {
        Value::Null => return Err("This field may not be null."),
        Value::String(s) => s.clone(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() => pl_core::json::python_float_repr(f),
            _ => n.to_string(),
        },
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        other => pl_core::pyrepr::repr_value(other),
    };
    let trimmed = strip_decimal_zeros(&s);
    crate::py::int(trimmed)
        .and_then(|i| i64::try_from(i).ok())
        .ok_or("A valid integer is required.")
}

/// `re.compile(r'\.0*\s*$').sub('', s)` (only the first match at the end).
fn strip_decimal_zeros(s: &str) -> &str {
    let t = s.trim_end_matches(crate::py::is_py_space);
    let t2 = t.trim_end_matches('0');
    match t2.strip_suffix('.') {
        Some(base) => base,
        None => s,
    }
}

/// `BooleanField.to_internal_value` (`allow_null=False`).
fn boolean_field(v: &Value) -> std::result::Result<bool, &'static str> {
    // DRF 3.18.3 `BooleanField._lower_if_str`: strings are lowercased before the lookup.
    const TRUE: &[&str] = &["t", "T", "y", "Y", "yes", "Yes", "YES", "true", "True", "TRUE", "on", "On", "ON", "1"];
    const FALSE: &[&str] = &["f", "F", "n", "N", "no", "No", "NO", "false", "False", "FALSE", "off", "Off", "OFF", "0"];
    match v {
        Value::Null => Err("This field may not be null."),
        Value::Bool(b) => Ok(*b),
        Value::Number(n) => match n.as_f64() {
            Some(1.0) => Ok(true),
            Some(0.0) => Ok(false),
            _ => Err("Must be a valid boolean."),
        },
        Value::String(s) if TRUE.contains(&s.to_lowercase().as_str()) => Ok(true),
        Value::String(s) if FALSE.contains(&s.to_lowercase().as_str()) => Ok(false),
        _ => Err("Must be a valid boolean."),
    }
}

/// Parsed fields of `AcknowledgeTasksViewSerializer` (before the DB count check).
#[derive(Debug, Clone, PartialEq)]
pub struct AckFields {
    /// `Some(Ok(ids))` = valid list, `Some(Err(_))` = field error, `None` = not given.
    pub tasks: Option<std::result::Result<Vec<i64>, Value>>,
    pub all: std::result::Result<bool, String>,
}

/// What to acknowledge after validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AckTarget {
    All,
    Tasks(Vec<i64>),
}

fn form_get_list<'a>(pairs: &'a [(String, String)], key: &str) -> Vec<&'a str> {
    pairs.iter().filter(|(k, _)| k == key).map(|(_, v)| v.as_str()).collect()
}

/// `html.parse_html_list(dictionary, prefix)` (`tasks[0]=..&tasks[1]=..`).
fn parse_html_list(pairs: &[(String, String)], prefix: &str) -> Option<Vec<String>> {
    let mut items: Vec<(u64, String)> = Vec::new();
    for (k, v) in pairs {
        if let Some(rest) = k.strip_prefix(prefix).and_then(|r| r.strip_prefix('['))
            && let Some((idx, tail)) = rest.split_once(']')
            && tail.is_empty()
            && let Ok(i) = idx.parse::<u64>()
        {
            items.retain(|(j, _)| *j != i);
            items.push((i, v.clone()));
        }
    }
    if items.is_empty() {
        return None;
    }
    items.sort_by_key(|(i, _)| *i);
    Some(items.into_iter().map(|(_, v)| v).collect())
}

/// Field-level validation of `AcknowledgeTasksViewSerializer`.
pub fn acknowledge_fields(data: &Data) -> std::result::Result<AckFields, ApiError> {
    let (tasks_raw, all_raw): (Option<Value>, Option<Value>) = match data {
        Data::Json(Value::Object(m)) => (m.get("tasks").cloned(), m.get("all").cloned()),
        Data::Json(other) => {
            let mut e = ValidationErrors::new();
            e.add_non_field(format!("Invalid data. Expected a dictionary, but got {}.", py_type(other)));
            return Err(e.into());
        }
        Data::Form(pairs) => {
            let list = form_get_list(pairs, "tasks");
            let tasks = if !list.is_empty() {
                Some(Value::Array(list.into_iter().map(|s| Value::from(s.to_string())).collect()))
            } else {
                parse_html_list(pairs, "tasks").map(|v| Value::Array(v.into_iter().map(Value::from).collect()))
            };
            // BooleanField.default_empty_html = False; QueryDict.get → last value
            let all = pairs.iter().rev().find(|(k, _)| k == "all").map(|(_, v)| Value::from(v.clone()));
            (tasks, all)
        }
    };
    let tasks = tasks_raw.map(|t| match t {
        Value::Null => Err(Value::Array(vec![Value::from("This field may not be null.")])),
        Value::Array(items) => {
            let mut errs = Map::new();
            let mut ids = Vec::new();
            for (i, it) in items.iter().enumerate() {
                match integer_field(it) {
                    Ok(v) => ids.push(v),
                    Err(m) => {
                        errs.insert(i.to_string(), Value::Array(vec![Value::from(m)]));
                    }
                }
            }
            if errs.is_empty() { Ok(ids) } else { Err(Value::Object(errs)) }
        }
        other => Err(Value::Array(vec![Value::from(format!(
            "Expected a list of items but got type \"{}\".",
            py_type(&other)
        ))])),
    });
    let all = match all_raw {
        None => Ok(false),
        Some(v) => boolean_field(&v).map_err(str::to_string),
    };
    Ok(AckFields { tasks, all })
}

/// Combine field errors (incl. the DB check result for `tasks`) and run `validate()`.
pub fn acknowledge_finish(fields: AckFields, tasks_db_error: Option<&str>) -> std::result::Result<AckTarget, ApiError> {
    let mut errs = Map::new();
    let tasks = match fields.tasks {
        Some(Err(e)) => {
            errs.insert("tasks".into(), e);
            None
        }
        Some(Ok(ids)) => {
            if let Some(msg) = tasks_db_error {
                errs.insert("tasks".into(), Value::Array(vec![Value::from(msg)]));
            }
            Some(ids)
        }
        None => None,
    };
    let all = match fields.all {
        Ok(b) => b,
        Err(m) => {
            errs.insert("all".into(), Value::Array(vec![Value::from(m)]));
            false
        }
    };
    if !errs.is_empty() {
        return Err(ApiError::validation(Value::Object(errs)));
    }
    match (all, tasks) {
        (true, Some(_)) => Err(ApiError::non_field_error("Set either all or tasks, not both.")),
        (false, None) => Err(ApiError::non_field_error("Either all must be true or tasks must be provided.")),
        (true, None) => Ok(AckTarget::All),
        (false, Some(ids)) => Ok(AckTarget::Tasks(ids)),
    }
}

/// `RunTaskSerializer` → `task_type`.
pub fn run_task_type(data: &Data, choices: &[(&str, &str)]) -> std::result::Result<String, ApiError> {
    let raw: Option<Value> = match data {
        Data::Json(Value::Object(m)) => m.get("task_type").cloned(),
        Data::Json(other) => {
            return Err(ApiError::non_field_error(format!(
                "Invalid data. Expected a dictionary, but got {}.",
                py_type(other)
            )));
        }
        Data::Form(pairs) => pairs.iter().rev().find(|(k, _)| k == "task_type").map(|(_, v)| Value::from(v.clone())),
    };
    let Some(v) = raw else { return Err(ApiError::field_error("task_type", "This field is required.")) };
    let s = match &v {
        Value::Null => return Err(ApiError::field_error("task_type", "This field may not be null.")),
        Value::String(s) => s.clone(),
        other => crate::tasks::serialize::py_str(other),
    };
    // ChoiceField: '' with allow_blank=False → "This field may not be blank."? No: ChoiceField
    // checks choice_strings_to_values[str(data)] → invalid_choice for ''.
    if choices.iter().any(|(c, _)| *c == s) {
        Ok(s)
    } else {
        Err(ApiError::field_error("task_type", format!("\"{s}\" is not a valid choice.")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review 1 N4 (checked against DRF 3.18.3 of the reference): `BooleanField` compares lowercased,
    /// `"tRuE"`/`"yES"` are valid, unknown values are not.
    #[test]
    fn drf_bool_is_case_insensitive_like_drf_3_18() {
        use serde_json::json;
        assert_eq!(boolean_field(&json!("yES")), Ok(true));
        assert_eq!(boolean_field(&json!("FaLsE")), Ok(false));
        assert!(boolean_field(&json!("maybe")).is_err());
    }
    use serde_json::json;

    fn ack(v: Value, db_err: Option<&str>) -> std::result::Result<AckTarget, Value> {
        let f = acknowledge_fields(&Data::Json(v)).map_err(|e| e.body_json().cloned().unwrap())?;
        acknowledge_finish(f, db_err).map_err(|e| e.body_json().cloned().unwrap())
    }

    #[test]
    fn acknowledge_validation() {
        assert_eq!(ack(json!({"tasks": [1, "2", 3.0]}), None), Ok(AckTarget::Tasks(vec![1, 2, 3])));
        assert_eq!(ack(json!({"all": true}), None), Ok(AckTarget::All));
        assert_eq!(
            ack(json!({}), None).unwrap_err().to_string(),
            r#"{"non_field_errors":["Either all must be true or tasks must be provided."]}"#
        );
        assert_eq!(
            ack(json!({"all": true, "tasks": [1]}), None).unwrap_err().to_string(),
            r#"{"non_field_errors":["Set either all or tasks, not both."]}"#
        );
        assert_eq!(
            ack(json!({"tasks": "1"}), None).unwrap_err().to_string(),
            r#"{"tasks":["Expected a list of items but got type \"str\"."]}"#
        );
        assert_eq!(
            ack(json!({"tasks": [1, "x", 2.5]}), None).unwrap_err().to_string(),
            r#"{"tasks":{"1":["A valid integer is required."],"2":["A valid integer is required."]}}"#
        );
        assert_eq!(
            ack(json!({"tasks": [9], "all": "maybe"}), Some("Some tasks in tasks don't exist or were specified twice."))
                .unwrap_err()
                .to_string(),
            r#"{"tasks":["Some tasks in tasks don't exist or were specified twice."],"all":["Must be a valid boolean."]}"#
        );
        assert_eq!(
            ack(json!([1]), None).unwrap_err().to_string(),
            r#"{"non_field_errors":["Invalid data. Expected a dictionary, but got list."]}"#
        );
    }

    #[test]
    fn form_bodies() {
        let d = parse_bytes(Some("application/x-www-form-urlencoded"), b"tasks=1&tasks=2").unwrap();
        let f = acknowledge_fields(&d).unwrap();
        assert_eq!(acknowledge_finish(f, None).unwrap(), AckTarget::Tasks(vec![1, 2]));
        let d = parse_bytes(Some("application/x-www-form-urlencoded"), b"all=true").unwrap();
        assert_eq!(acknowledge_finish(acknowledge_fields(&d).unwrap(), None).unwrap(), AckTarget::All);
        assert!(parse_bytes(Some("text/plain"), b"x").is_err());
        assert_eq!(parse_bytes(Some("application/json"), b"").unwrap(), Data::Json(json!({})));
    }

    #[test]
    fn run_validation() {
        let choices = crate::labels::TASK_TYPE;
        assert_eq!(run_task_type(&Data::Json(json!({"task_type": "sanity_check"})), choices).unwrap(), "sanity_check");
        assert_eq!(
            run_task_type(&Data::Json(json!({})), choices).unwrap_err().body_json().unwrap().to_string(),
            r#"{"task_type":["This field is required."]}"#
        );
        assert_eq!(
            run_task_type(&Data::Json(json!({"task_type": "bogus"})), choices)
                .unwrap_err()
                .body_json()
                .unwrap()
                .to_string(),
            r#"{"task_type":["\"bogus\" is not a valid choice."]}"#
        );
    }

    #[test]
    fn decimal_strip() {
        assert_eq!(strip_decimal_zeros("5.0"), "5");
        assert_eq!(strip_decimal_zeros("5.00 "), "5");
        assert_eq!(strip_decimal_zeros("5.5"), "5.5");
        assert_eq!(strip_decimal_zeros("50"), "50");
    }
}
