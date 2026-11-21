// upstream: src/documents/versioning.py@v3.4.2 (get_root_document, resolve_requested_version_for_root,
//           resolve_effective_document, resolve_effective_document_by_pk)
//           src/documents/views.py@v3.4.2 (DocumentViewSet._resolve_file_doc, _get_effective_file_doc,
//           _resolve_request_and_root_doc, original_requested)

use sqlx::PgPool;

use crate::db::{self, DocRow};

/// Parsed query string with Django `QueryDict` semantics (last value wins for `.get`).
#[derive(Debug, Default, Clone)]
pub struct Params(pub Vec<(String, String)>);

impl Params {
    pub fn parse(raw: Option<&str>) -> Self {
        let raw = raw.unwrap_or("");
        Params(url::form_urlencoded::parse(raw.as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect())
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
    /// `DocumentViewSet.original_requested`
    pub fn original_requested(&self) -> bool {
        self.get("original") == Some("true")
    }
    /// `request.query_params.get("follow_formatting", False)` – Python truthiness.
    pub fn follow_formatting(&self) -> bool {
        self.get("follow_formatting").is_some_and(|v| !v.is_empty())
    }
}

/// Python `int(str)` acceptance. `Ok(Some(v))` valid and in range, `Ok(None)` valid but out of
/// i64 range (Django 5 returns no rows for out-of-range integer lookups), `Err(())` invalid.
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidInt;

pub fn py_int(s: &str) -> Result<Option<i64>, InvalidInt> {
    let t = s.trim();
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'+') => (false, &t[1..]),
        Some(b'-') => (true, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return Err(InvalidInt);
    }
    if !digits.chars().all(|c| c.is_ascii_digit() || c == '_') {
        return Err(InvalidInt);
    }
    let clean: String = digits.chars().filter(|c| *c != '_').collect();
    let full = if neg { format!("-{clean}") } else { clean };
    Ok(full.parse::<i64>().ok())
}

pub enum VersionError {
    Invalid,
    NotFound,
}

/// `get_root_document(doc)` – the FK descriptor uses the base manager (trash included).
pub async fn root_of(db: &PgPool, doc: &DocRow) -> Result<DocRow, sqlx::Error> {
    match doc.root_document_id {
        None => Ok(doc.clone()),
        Some(rid) => Ok(db::load_doc(db, rid, true).await?.unwrap_or_else(|| doc.clone())),
    }
}

/// `resolve_requested_version_for_root(root, request, include_deleted)`.
pub async fn requested_version_for_root(
    db: &PgPool,
    root: &DocRow,
    version: Option<&str>,
    include_deleted: bool,
) -> Result<Result<DocRow, VersionError>, sqlx::Error> {
    let v = match version {
        None | Some("") => return Ok(Ok(db::latest_version(db, root, include_deleted).await?)),
        Some(v) => v,
    };
    let id = match py_int(v) {
        Err(InvalidInt) => return Ok(Err(VersionError::Invalid)),
        Ok(None) => return Ok(Err(VersionError::NotFound)),
        Ok(Some(id)) => id,
    };
    let Some(candidate) = db::load_doc(db, id, include_deleted).await? else {
        return Ok(Err(VersionError::NotFound));
    };
    if candidate.id != root.id && candidate.root_document_id != Some(root.id) {
        return Ok(Err(VersionError::NotFound));
    }
    Ok(Ok(candidate))
}

/// `resolve_effective_document_by_pk(pk, request)` (include_deleted=False) as used by the
/// ETag / Last-Modified functions. `None` when no effective document.
pub async fn effective_by_pk(db: &PgPool, pk: i64, params: &Params) -> Result<Option<DocRow>, sqlx::Error> {
    let Some(request_doc) = db::load_doc(db, pk, false).await? else {
        return Ok(None);
    };
    let root = root_of(db, &request_doc).await?;
    let version = params.get("version");
    if version.is_some() {
        return Ok(requested_version_for_root(db, &root, version, false).await?.ok());
    }
    if request_doc.root_document_id.is_none() {
        return Ok(Some(db::latest_version(db, &root, false).await?));
    }
    Ok(Some(request_doc))
}

/// `_get_effective_file_doc` + `_resolve_file_doc` for the views (trash included).
pub async fn effective_file_doc(
    db: &PgPool,
    request_doc: &DocRow,
    root: &DocRow,
    params: &Params,
) -> Result<Result<DocRow, VersionError>, sqlx::Error> {
    let version = params.get("version");
    if request_doc.root_document_id.is_some() && version.is_none() {
        return Ok(Ok(request_doc.clone()));
    }
    requested_version_for_root(db, root, version, version.is_some()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_int() {
        assert_eq!(py_int(" 12 "), Ok(Some(12)));
        assert_eq!(py_int("+1_0"), Ok(Some(10)));
        assert_eq!(py_int("-3"), Ok(Some(-3)));
        assert_eq!(py_int("not-a-number"), Err(InvalidInt));
        assert_eq!(py_int("1.0"), Err(InvalidInt));
        assert_eq!(py_int("1__0"), Err(InvalidInt));
        assert_eq!(py_int("99999999999999999999999"), Ok(None));
    }

    #[test]
    fn params() {
        let p = Params::parse(Some("original=false&original=true&follow_formatting=false"));
        assert!(p.original_requested());
        assert!(p.follow_formatting());
        let p = Params::parse(Some("follow_formatting="));
        assert!(!p.follow_formatting());
    }
}
