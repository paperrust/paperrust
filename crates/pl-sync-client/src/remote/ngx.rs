//! Restricted counterpart: unmodified paperless-ngx server via the v1 API
//! (docs/SYNC.md §7, ROADMAP-NEXT P6). Login with v1 token (`POST /api/token/`, `Token <key>`),
//! `Accept: application/json; version=9` (because of `all`).
//!
//! Mapping into the same replica as PaperRust: v1 objects are translated into the v2 form
//! ([`doc_v2`], [`obj_v2`]); keys are **synthetic UUIDs** from kind + ID ([`ngx_uuid`]),
//! so offline queries (`DocQuery`), gateway and v1 adapter work without a special case
//! (deviation from SYNC.md §4.2 `ngx:<id>`, contract "Known differences").

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;
use zeroize::Zeroizing;

use super::RawResponse;
use super::paperrust::{map_reqwest, normalize_base};
use crate::error::{Error, Result};
use crate::tls::{TlsOptions, client_config};

/// v1 version with `all` in lists (`upstream/src/paperless/views.py: StandardPagination`).
pub const ACCEPT: &str = "application/json; version=9";
const TIMEOUT: Duration = Duration::from_secs(60);

/// Synthetic UUID for a paperless-ngx object (kind code like `pl_domain::Kind::code`).
pub fn ngx_uuid(code: i16, id: i64) -> String {
    format!("00000000-0000-4000-8{:03x}-{:012x}", code as u16 & 0xfff, id as u64 & 0xffff_ffff_ffff)
}

/// Kind code of a v1 object kind.
pub fn code_of(kind: &str) -> i16 {
    pl_domain::Kind::parse(kind).map(|k| k.code()).unwrap_or(0)
}

pub struct NgxRemote {
    base: Url,
    client: reqwest::Client,
    token: RwLock<Option<Zeroizing<String>>>,
}

impl std::fmt::Debug for NgxRemote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NgxRemote").field("base", &self.base.as_str()).finish()
    }
}

async fn v1_error(r: reqwest::Response) -> Error {
    let status = r.status();
    let body: Value = r.json().await.unwrap_or(Value::Null);
    let message = body["detail"]
        .as_str()
        .map(str::to_string)
        .or_else(|| body["non_field_errors"][0].as_str().map(str::to_string))
        .unwrap_or_else(|| body.to_string());
    match status {
        StatusCode::UNAUTHORIZED => Error::Unauthenticated,
        _ => Error::Api { status: status.as_u16(), code: if status == StatusCode::BAD_REQUEST { "invalid".into() } else { "error".into() }, message },
    }
}

impl NgxRemote {
    pub fn new(base: &str, tls: &TlsOptions) -> Result<Self> {
        let base = normalize_base(base)?;
        let cfg = client_config(tls).map_err(Error::Tls)?;
        let client = reqwest::Client::builder()
            .use_preconfigured_tls(cfg)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .user_agent(super::paperrust::USER_AGENT)
            .build()
            .map_err(|e| Error::Tls(e.to_string()))?;
        Ok(Self { base, client, token: RwLock::new(None) })
    }

    pub fn set_token(&self, t: Option<Zeroizing<String>>) {
        if let Ok(mut g) = self.token.write() {
            *g = t;
        }
    }

    pub fn has_token(&self) -> bool {
        self.token.read().map(|t| t.is_some()).unwrap_or(false)
    }

    fn url(&self, pq: &str) -> Result<Url> {
        let u = self.base.join(pq.trim_start_matches('/')).map_err(|e| Error::Invalid(e.to_string()))?;
        if u.origin() != self.base.origin() {
            return Err(Error::Invalid("path leaves the server origin".into()));
        }
        Ok(u)
    }

    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.token.read().ok().and_then(|t| t.clone()) {
            Some(t) => {
                let mut v = HeaderValue::from_str(&format!("Token {}", t.as_str())).unwrap_or(HeaderValue::from_static(""));
                v.set_sensitive(true);
                rb.header(reqwest::header::AUTHORIZATION, v)
            }
            None => rb,
        }
    }

    /// GET (JSON) of a path with query (relative to the base).
    pub async fn get_json(&self, pq: &str) -> Result<Value> {
        let rb = self.client.get(self.url(pq)?).header("Accept", ACCEPT).timeout(TIMEOUT);
        let r = self.auth(rb).send().await.map_err(map_reqwest)?;
        if !r.status().is_success() {
            return Err(v1_error(r).await);
        }
        r.json().await.map_err(map_reqwest)
    }

    /// Fetch v1 token (`POST /api/token/`, user/password/`code` with MFA).
    pub async fn token(&self, username: &str, password: &str, code: Option<&str>) -> Result<Zeroizing<String>> {
        let mut body = json!({"username": username, "password": password});
        if let Some(c) = code.filter(|c| !c.trim().is_empty()) {
            body["code"] = json!(c.trim());
        }
        let bytes = Zeroizing::new(serde_json::to_vec(&body)?);
        let r = self
            .client
            .post(self.url("api/token/")?)
            .header("Content-Type", "application/json")
            .header("Accept", ACCEPT)
            .timeout(TIMEOUT)
            .body(bytes.to_vec())
            .send()
            .await
            .map_err(map_reqwest)?;
        if !r.status().is_success() {
            return Err(v1_error(r).await);
        }
        let v: Value = r.json().await.map_err(map_reqwest)?;
        v["token"].as_str().map(|t| Zeroizing::new(t.to_string())).ok_or_else(|| Error::Protocol("token missing".into()))
    }

    /// All objects of a list (`page_size=100000`, follows `next`).
    pub async fn list_all(&self, endpoint: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut page = 1;
        loop {
            let v = self.get_json(&format!("api/{endpoint}/?page={page}&page_size=100000")).await?;
            out.extend(v["results"].as_array().cloned().unwrap_or_default());
            if v["next"].is_null() || page > 100 {
                return Ok(out);
            }
            page += 1;
        }
    }

    /// All visible document IDs for a filter (`page_size=1`, field `all`).
    pub async fn doc_ids(&self, params: &[(String, String)]) -> Result<Vec<i64>> {
        // do not hold the serializer across `await` (not Send)
        let q = {
            let mut ser = url::form_urlencoded::Serializer::new(String::new());
            for (k, v) in params {
                ser.append_pair(k, v);
            }
            ser.append_pair("page_size", "1");
            ser.append_pair("fields", "id");
            ser.finish()
        };
        let v = self.get_json(&format!("api/documents/?{q}")).await?;
        let all = v["all"].as_array().ok_or_else(|| Error::Protocol("paperless-ngx answered without `all` (API version < 9?)".into()))?;
        Ok(all.iter().filter_map(Value::as_i64).collect())
    }

    /// Documents by ID (≤ 100 per request), optionally only certain fields.
    pub async fn docs_by_id(&self, ids: &[i64], fields: Option<&str>) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for chunk in ids.chunks(100) {
            let list: Vec<String> = chunk.iter().map(i64::to_string).collect();
            let mut pq = format!("api/documents/?id__in={}&page_size=100&ordering=id", list.join(","));
            if let Some(f) = fields {
                pq.push_str("&fields=");
                pq.push_str(f);
            }
            let v = self.get_json(&pq).await?;
            out.extend(v["results"].as_array().cloned().unwrap_or_default());
        }
        Ok(out)
    }

    /// IDs of the documents changed since `since` (RFC 3339) (`modified__gt`, ascending).
    pub async fn modified_since(&self, since: &str) -> Result<Vec<(i64, String)>> {
        let mut out = Vec::new();
        let mut page = 1;
        loop {
            let q = {
                let mut ser = url::form_urlencoded::Serializer::new(String::new());
                ser.append_pair("modified__gt", since);
                ser.append_pair("ordering", "modified");
                ser.append_pair("page", &page.to_string());
                ser.append_pair("page_size", "100");
                ser.append_pair("fields", "id,modified");
                ser.finish()
            };
            let v = self.get_json(&format!("api/documents/?{q}")).await?;
            for d in v["results"].as_array().cloned().unwrap_or_default() {
                if let (Some(id), Some(m)) = (d["id"].as_i64(), d["modified"].as_str()) {
                    out.push((id, m.to_string()));
                }
            }
            if v["next"].is_null() || page > 1000 {
                return Ok(out);
            }
            page += 1;
        }
    }

    /// `/api/documents/<id>/metadata/` (checksums, sizes).
    pub async fn metadata(&self, id: i64) -> Result<Value> {
        self.get_json(&format!("api/documents/{id}/metadata/")).await
    }

    /// File: `original` → `download/?original=true`, `archive` → `download/`, `thumbnail` → `thumb/`.
    /// paperless-ngx cannot do Range (Django `FileResponse`) → resuming reloads.
    pub async fn file(&self, id: i64, variant: &str, if_none_match: Option<&str>) -> Result<reqwest::Response> {
        let pq = match variant {
            "original" => format!("api/documents/{id}/download/?original=true"),
            "archive" | "preview" => format!("api/documents/{id}/download/"),
            _ => format!("api/documents/{id}/thumb/"),
        };
        let mut h = HeaderMap::new();
        if let Some(e) = if_none_match.and_then(|e| HeaderValue::from_str(e).ok()) {
            h.insert(reqwest::header::IF_NONE_MATCH, e);
        }
        let rb = self.client.get(self.url(&pq)?).headers(h).timeout(Duration::from_secs(600));
        let r = self.auth(rb).send().await.map_err(map_reqwest)?;
        if r.status().is_success() || r.status() == StatusCode::NOT_MODIFIED {
            Ok(r)
        } else {
            Err(v1_error(r).await)
        }
    }

    /// Rohweiterleitung (Gateway, Online-Modus).
    pub async fn raw(&self, method: &str, path_and_query: &str, headers: &[(String, String)], body: Vec<u8>) -> Result<RawResponse> {
        let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| Error::Invalid("method".into()))?;
        let mut rb = self.client.request(method, self.url(path_and_query)?).timeout(Duration::from_secs(120));
        for (k, v) in headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        if !body.is_empty() {
            rb = rb.body(body);
        }
        let r = self.auth(rb).send().await.map_err(map_reqwest)?;
        let status = r.status().as_u16();
        let headers = r.headers().iter().filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_string(), v.to_string()))).collect();
        let body = r.bytes().await.map_err(map_reqwest)?;
        Ok(RawResponse { status, headers, body })
    }
}

// ---------------------------------------------------------------------------------- Abbildung

/// User ID → name (for `owner`/notes).
pub type Users = HashMap<i64, String>;

fn uref(id: &Value, users: &Users) -> Value {
    match id.as_i64() {
        Some(i) => json!({"uuid": ngx_uuid(8, i), "username": users.get(&i).cloned().unwrap_or_default()}),
        None => Value::Null,
    }
}

fn r(code: i16, v: &Value) -> Value {
    v.as_i64().map(|i| json!(ngx_uuid(code, i))).unwrap_or(Value::Null)
}

fn matching(o: &Value) -> Value {
    let alg = o["matching_algorithm"].as_i64().unwrap_or(0);
    json!({"algorithm": pl_domain::model::matching_name(alg as i16), "pattern": o["match"].as_str().unwrap_or(""),
           "is_insensitive": o["is_insensitive"].as_bool().unwrap_or(true)})
}

/// v1 object → v2 form (like `pl_domain::model`).
pub fn obj_v2(kind: &str, o: &Value, users: &Users) -> Value {
    let id = o["id"].as_i64().unwrap_or(0);
    let code = code_of(kind);
    let can = o["user_can_change"].as_bool().unwrap_or(true);
    let access = json!({"can_view": true, "can_change": can, "can_delete": can});
    let mut v = match kind {
        "tag" => json!({"name": o["name"], "color": o["color"], "text_color": o["text_color"], "is_inbox_tag": o["is_inbox_tag"],
                        "matching": matching(o), "parent": r(2, &o["parent"]), "owner": uref(&o["owner"], users), "access": access,
                        "document_count": o["document_count"]}),
        "correspondent" | "document_type" => json!({"name": o["name"], "matching": matching(o), "owner": uref(&o["owner"], users),
                                                    "access": access, "document_count": o["document_count"]}),
        "storage_path" => json!({"name": o["name"], "path": o["path"], "matching": matching(o), "owner": uref(&o["owner"], users),
                                 "access": access, "document_count": o["document_count"]}),
        "custom_field" => json!({"name": o["name"], "data_type": o["data_type"], "extra_data": o["extra_data"], "created": null}),
        "saved_view" => json!({"name": o["name"], "sort_field": o["sort_field"], "sort_reverse": o["sort_reverse"],
                               "page_size": o["page_size"], "display_mode": o["display_mode"], "display_fields": o["display_fields"],
                               "icon": null, "filter_rules": o["filter_rules"], "owner": uref(&o["owner"], users), "access": access}),
        "user" => json!({"username": o["username"], "first_name": o["first_name"], "last_name": o["last_name"]}),
        _ => json!({"name": o["name"]}),
    };
    v["uuid"] = json!(ngx_uuid(code, id));
    v["legacy_id"] = json!(id);
    v["rev"] = json!(0);
    v
}

/// Fingerprint of a document from the fields that can change without `modified`
/// (SYNC.md §7: bulk tags, notes …), SHA-256 hex.
pub fn fingerprint(d: &Value) -> String {
    let mut tags: Vec<i64> = d["tags"].as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
    tags.sort();
    let notes: Vec<Value> = d["notes"]
        .as_array()
        .map(|a| a.iter().map(|n| json!([n["id"], n["note"]])).collect())
        .unwrap_or_default();
    let fp = json!([d["id"], d["modified"], tags, d["correspondent"], d["document_type"], d["storage_path"], d["custom_fields"],
                    notes, d["archive_serial_number"], d["title"], d["created"]]);
    Sha256::digest(fp.to_string().as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Fields of the fingerprint query (`fields=`).
pub const FINGERPRINT_FIELDS: &str = "id,modified,tags,correspondent,document_type,storage_path,custom_fields,notes,archive_serial_number,title,created";

/// v1-Dokument (+ Metadaten) → v2-Form inkl. `content`/`notes`.
pub fn doc_v2(d: &Value, meta: Option<&Value>, users: &Users, doclink_fields: &[i64]) -> Value {
    let id = d["id"].as_i64().unwrap_or(0);
    let fp = fingerprint(d);
    // rev from the fingerprint (stable, changes with the content)
    let rev = i64::from_str_radix(&fp[..12], 16).unwrap_or(0);
    let tags: Vec<Value> = d["tags"].as_array().map(|a| a.iter().map(|t| r(2, t)).collect()).unwrap_or_default();
    let cfs: Vec<Value> = d["custom_fields"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|c| {
                    let field = c["field"].as_i64().unwrap_or(0);
                    let value = if doclink_fields.contains(&field) {
                        Value::Array(c["value"].as_array().map(|l| l.iter().map(|x| r(1, x)).collect()).unwrap_or_default())
                    } else {
                        c["value"].clone()
                    };
                    json!({"field": ngx_uuid(6, field), "value": value})
                })
                .collect()
        })
        .unwrap_or_default();
    let notes: Vec<Value> = d["notes"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|n| {
                    let user = match &n["user"] {
                        Value::Object(u) => json!({"uuid": u.get("id").and_then(Value::as_i64).map(|i| ngx_uuid(8, i)), "username": u.get("username")}),
                        other => uref(other, users),
                    };
                    json!({"legacy_id": n["id"], "note": n["note"], "created": n["created"], "user": user})
                })
                .collect()
        })
        .unwrap_or_default();
    let has_archive = d["archived_file_name"].as_str().is_some_and(|s| !s.is_empty())
        || meta.is_some_and(|m| m["has_archive_version"].as_bool() == Some(true));
    let files = json!({
        "original": {"checksum": meta.map(|m| m["original_checksum"].clone()).unwrap_or(Value::Null),
                     "size": meta.map(|m| m["original_size"].clone()).unwrap_or(Value::Null)},
        "archive": if has_archive { json!({"checksum": meta.map(|m| m["archive_checksum"].clone()).unwrap_or(Value::Null),
                                           "size": meta.map(|m| m["archive_size"].clone()).unwrap_or(Value::Null)}) } else { Value::Null },
        // Thumbnail identifier: changes with the document (paperless-ngx: ETag = checksum)
        "thumbnail": {"etag": meta.and_then(|m| m["original_checksum"].as_str()).map(|c| format!("\"{c}\"")).unwrap_or_else(|| format!("ngx-{}", d["modified"].as_str().unwrap_or("")))},
    });
    let created = d["created"].as_str().map(|c| c.get(..10).unwrap_or(c).to_string());
    let can = d["user_can_change"].as_bool().unwrap_or(false);
    json!({
        "uuid": ngx_uuid(1, id), "legacy_id": id, "rev": rev, "title": d["title"], "created": created,
        "added": d["added"], "modified": d["modified"],
        "correspondent": r(3, &d["correspondent"]), "document_type": r(4, &d["document_type"]), "storage_path": r(5, &d["storage_path"]),
        "tags": tags, "custom_fields": cfs, "asn": d["archive_serial_number"], "owner": uref(&d["owner"], users),
        "page_count": d["page_count"], "mime_type": d["mime_type"], "original_filename": d["original_file_name"],
        "notes_count": notes.len(), "files": files, "access": {"can_view": true, "can_change": can, "can_delete": can},
        "content": d["content"], "notes": notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_uuids_are_valid_and_distinct() {
        let a = ngx_uuid(1, 42);
        assert_eq!(a, "00000000-0000-4000-8001-00000000002a");
        assert!(uuid::Uuid::parse_str(&a).is_ok());
        assert_ne!(ngx_uuid(2, 42), a);
        assert_eq!(code_of("document_type"), 4);
    }

    #[test]
    fn document_mapping_and_fingerprint() {
        let users: Users = [(3, "alice".to_string())].into_iter().collect();
        let d = json!({"id": 7, "title": "T", "content": "C", "tags": [2, 1], "correspondent": 5, "document_type": null,
            "storage_path": null, "created": "2024-01-31", "added": "2024-02-01T10:00:00Z", "modified": "2024-02-01T10:00:00Z",
            "archive_serial_number": 12, "owner": 3, "user_can_change": true, "notes": [{"id": 1, "note": "n", "created": "x", "user": {"id": 3, "username": "alice"}}],
            "custom_fields": [{"field": 9, "value": [7, 8]}, {"field": 4, "value": "s"}], "page_count": 1, "mime_type": "application/pdf",
            "original_file_name": "t.pdf", "archived_file_name": "t.pdf"});
        let meta = json!({"original_checksum": "abc", "original_size": 10, "archive_checksum": "def", "archive_size": 20, "has_archive_version": true});
        let v = doc_v2(&d, Some(&meta), &users, &[9]);
        assert_eq!(v["uuid"], ngx_uuid(1, 7));
        assert_eq!(v["tags"][0], ngx_uuid(2, 2));
        assert_eq!(v["correspondent"], ngx_uuid(3, 5));
        assert_eq!(v["owner"]["username"], "alice");
        assert_eq!(v["custom_fields"][0]["value"][1], ngx_uuid(1, 8));
        assert_eq!(v["custom_fields"][1]["value"], "s");
        assert_eq!(v["files"]["archive"]["checksum"], "def");
        assert_eq!(v["notes"][0]["note"], "n");
        // fingerprint: tag order does not matter, tag change without modified detected
        let mut d2 = d.clone();
        d2["tags"] = json!([1, 2]);
        assert_eq!(fingerprint(&d), fingerprint(&d2));
        d2["tags"] = json!([1, 2, 3]);
        assert_ne!(fingerprint(&d), fingerprint(&d2));
        let t = obj_v2("tag", &json!({"id": 2, "name": "A", "matching_algorithm": 6, "match": "", "is_insensitive": true, "parent": 1, "owner": null}), &users);
        assert_eq!(t["matching"]["algorithm"], "auto");
        assert_eq!(t["parent"], ngx_uuid(2, 1));
    }
}
