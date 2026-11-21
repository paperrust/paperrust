// upstream: src/documents/bulk_edit.py@v3.4.2 (_resolve_root_and_source_doc, _scratch_path,
// rotate, merge, split, delete_pages, edit_pdf, remove_password)
//
//! Pure decision logic of PDF bulk editing without DB/queue: source document,
//! output file names, overrides. pl-documents-write loads the documents, calls [`crate::ops`] and
//! enqueues `consume_file` (details in the contract, section "bulk_edit").

use std::path::{Path, PathBuf};

use crate::Result;

/// `SourceMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceMode {
    #[default]
    LatestVersion,
    ExplicitSelection,
}

impl SourceMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "latest_version" => Some(SourceMode::LatestVersion),
            "explicit_selection" => Some(SourceMode::ExplicitSelection),
            _ => None,
        }
    }
}

/// `_resolve_root_and_source_doc`: `(root_id, source_id)`.
///
/// `root_document_id` = `doc.root_document_id`; `latest_version_for_root(root)` =
/// `get_latest_version_for_root` (only called when needed).
pub fn resolve_root_and_source(
    doc_id: i64,
    root_document_id: Option<i64>,
    mode: SourceMode,
    latest_version_for_root: impl FnOnce(i64) -> i64,
) -> (i64, i64) {
    let root = root_document_id.unwrap_or(doc_id);
    if mode == SourceMode::ExplicitSelection || root_document_id.is_some() {
        return (root, doc_id);
    }
    (root, latest_version_for_root(root))
}

/// New directory only for the own user (security review final W-N2, like
/// `tempfile.mkdtemp`: Unix `0700`). Windows: no own ACL – the directory inherits the
/// permissions of `SCRATCH_DIR` (default `%TEMP%` of the service account or the desktop data directory
/// restricted to the user via ACL); `mkdtemp` sets no ACL on Windows either.
fn create_private_dir(p: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new().mode(0o700).create(p)
    }
    #[cfg(not(unix))]
    {
        std::fs::DirBuilder::new().create(p)
    }
}

/// `_scratch_path(name)`: new directory under `SCRATCH_DIR` (like `tempfile.mkdtemp`) + `name`.
pub fn scratch_path(scratch_dir: &Path, name: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(scratch_dir)?;
    for _ in 0..100 {
        let dir = scratch_dir.join(format!("tmp{}", random_suffix()));
        match create_private_dir(&dir) {
            Ok(()) => return Ok(dir.join(name)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "No usable temporary directory name found").into())
}

fn random_suffix() -> String {
    use std::hash::{BuildHasher, Hasher};
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    let mut v = h.finish();
    (0..8)
        .map(|_| {
            let c = ALPHABET[(v % ALPHABET.len() as u64) as usize] as char;
            v /= ALPHABET.len() as u64;
            c
        })
        .collect()
}

/// Output file names (each in its own scratch directory).
pub mod names {
    pub fn rotated(root_id: i64) -> String {
        format!("{root_id}_rotated.pdf")
    }
    /// `f"{'_'.join(ids)[:100]}_merged.pdf"`
    pub fn merged(affected: &[i64]) -> String {
        let joined: String = affected.iter().map(i64::to_string).collect::<Vec<_>>().join("_");
        format!("{}_merged.pdf", joined.chars().take(100).collect::<String>())
    }
    /// `f"{doc.id}_{split_doc[0]}-{split_doc[-1]}.pdf"` (with the *selected* id).
    pub fn split(doc_id: i64, first: i64, last: i64) -> String {
        format!("{doc_id}_{first}-{last}.pdf")
    }
    pub fn pages_deleted(root_id: i64) -> String {
        format!("{root_id}_pages_deleted.pdf")
    }
    pub fn edited(root_id: i64) -> String {
        format!("{root_id}_edited.pdf")
    }
    /// `edit_pdf` without `update_document`: index from 1.
    pub fn edit_part(root_id: i64, idx: usize) -> String {
        format!("{root_id}_edit_{idx}.pdf")
    }
    pub fn unprotected(root_id: i64) -> String {
        format!("{root_id}_unprotected.pdf")
    }
}

/// Overrides for a new/versioned document (subset of `DocumentMetadataOverrides`
/// that bulk_edit sets). `from_document` = `DocumentMetadataOverrides.from_document(doc)` first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverridesPlan {
    pub from_document: Option<i64>,
    pub title: Option<String>,
    pub owner_id: Option<i64>,
    pub actor_id: Option<i64>,
    pub skip_asn_if_exists: bool,
    /// `Some(x)` = `overrides.asn = x` (`None` is also possible: edit_pdf with delete_original).
    pub asn: Option<Option<i64>>,
}

/// rotate / delete_pages: new version of the root, overrides from the root, `actor_id`.
pub fn version_overrides(root_id: i64, user_id: Option<i64>) -> OverridesPlan {
    OverridesPlan { from_document: Some(root_id), actor_id: user_id, ..Default::default() }
}

/// A document for `merge` (already resolved via [`resolve_root_and_source`]).
#[derive(Debug, Clone)]
pub struct MergeDoc {
    /// selected id (`doc.id`)
    pub id: i64,
    /// ASN of the *selected* document
    pub archive_serial_number: Option<i64>,
    pub source_mime_type: String,
    pub source_path: PathBuf,
    pub source_archive_path: Option<PathBuf>,
}

/// Input file for `merger.add`: archive only with `archive_fallback` and a non-PDF source.
pub fn merge_input_path(doc: &MergeDoc, archive_fallback: bool) -> &Path {
    if archive_fallback
        && doc.source_mime_type != "application/pdf"
        && let Some(a) = &doc.source_archive_path
    {
        return a;
    }
    &doc.source_path
}

/// Result of the overrides decision of `merge`.
///
/// `handoff_asn` = first ASN of the *merged* documents in order; the
/// metadata document (if set and present) overrides it with its ASN.
pub fn merge_overrides(
    merged: &[(i64, Option<i64>)],
    metadata_document: Option<(i64, &str, Option<i64>)>,
    user_id: Option<i64>,
    delete_originals: bool,
) -> OverridesPlan {
    let mut handoff = merged.iter().find_map(|(_, asn)| *asn);
    let mut o = OverridesPlan::default();
    if let Some((id, title, asn)) = metadata_document {
        o.from_document = Some(id);
        o.title = Some(format!("{title} (merged)"));
        if asn.is_some() {
            handoff = asn;
        }
    }
    o.owner_id = user_id;
    if !delete_originals {
        o.skip_asn_if_exists = true;
    }
    if delete_originals && handoff.is_some() {
        o.asn = Some(handoff);
    }
    o
}

/// `split`: overrides per part (`idx` from 0), title `"{title} (split {idx+1})"` from the
/// selected document.
pub fn split_overrides(doc_id: i64, doc_title: &str, idx: usize, user_id: Option<i64>, delete_originals: bool) -> OverridesPlan {
    OverridesPlan {
        from_document: Some(doc_id),
        title: Some(format!("{doc_title} (split {})", idx + 1)),
        owner_id: user_id,
        skip_asn_if_exists: !delete_originals,
        ..Default::default()
    }
}

/// `edit_pdf` / `remove_password`: `include_metadata` → from the root; user → owner+actor.
/// `edit_pdf` without `update_document`: `skip_asn_if_exists` without delete_original, ASN of the root
/// with delete_original and exactly one output (`root_asn`). `remove_password` never sets either
/// (`for_edit = false`).
#[allow(clippy::too_many_arguments)]
pub fn edit_overrides(
    root_id: i64,
    root_asn: Option<i64>,
    include_metadata: bool,
    user_id: Option<i64>,
    new_documents: bool,
    delete_original: bool,
    output_count: usize,
    for_edit: bool,
) -> OverridesPlan {
    let mut o = OverridesPlan { from_document: include_metadata.then_some(root_id), ..Default::default() };
    if user_id.is_some() {
        o.owner_id = user_id;
        o.actor_id = user_id;
    }
    if for_edit && new_documents {
        if !delete_original {
            o.skip_asn_if_exists = true;
        }
        if delete_original && output_count == 1 {
            o.asn = Some(root_asn);
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_source() {
        assert_eq!(resolve_root_and_source(5, None, SourceMode::LatestVersion, |_| 9), (5, 9));
        assert_eq!(resolve_root_and_source(5, None, SourceMode::ExplicitSelection, |_| 9), (5, 5));
        assert_eq!(resolve_root_and_source(7, Some(5), SourceMode::LatestVersion, |_| 9), (5, 7));
        assert_eq!(SourceMode::parse("explicit_selection"), Some(SourceMode::ExplicitSelection));
    }

    #[test]
    fn file_names() {
        assert_eq!(names::merged(&[3, 1, 2]), "3_1_2_merged.pdf");
        let many: Vec<i64> = (1000..1100).collect();
        assert_eq!(names::merged(&many).len(), 100 + "_merged.pdf".len());
        assert_eq!(names::split(4, 1, 3), "4_1-3.pdf");
        assert_eq!(names::edit_part(4, 1), "4_edit_1.pdf");
        let t = tempfile::tempdir().unwrap();
        let p = scratch_path(t.path(), "x.pdf").unwrap();
        assert!(p.parent().unwrap().is_dir());
        assert_ne!(p, scratch_path(t.path(), "x.pdf").unwrap());
    }

    #[test]
    fn merge_asn_handoff() {
        let merged = [(1, None), (2, Some(1002)), (3, Some(1003))];
        let o = merge_overrides(&merged, None, Some(7), true);
        assert_eq!(o.asn, Some(Some(1002)));
        assert_eq!(o.owner_id, Some(7));
        assert!(!o.skip_asn_if_exists);
        let o = merge_overrides(&merged, Some((3, "Rechnung", Some(1003))), None, true);
        assert_eq!((o.title.as_deref(), o.asn), (Some("Rechnung (merged)"), Some(Some(1003))));
        let o = merge_overrides(&merged, Some((1, "A", None)), None, false);
        assert_eq!((o.asn, o.skip_asn_if_exists), (None, true));
    }

    #[test]
    fn edit_and_split_overrides() {
        let o = edit_overrides(5, Some(1005), true, Some(2), true, true, 1, true);
        assert_eq!((o.from_document, o.owner_id, o.actor_id, o.asn), (Some(5), Some(2), Some(2), Some(Some(1005))));
        let o = edit_overrides(5, Some(1005), false, None, true, false, 2, true);
        assert_eq!((o.from_document, o.skip_asn_if_exists, o.asn), (None, true, None));
        let o = edit_overrides(5, Some(1005), true, None, true, true, 1, false);
        assert_eq!((o.skip_asn_if_exists, o.asn), (false, None));
        let s = split_overrides(4, "Brief", 1, None, false);
        assert_eq!((s.title.as_deref(), s.skip_asn_if_exists), (Some("Brief (split 2)"), true));
    }
}
