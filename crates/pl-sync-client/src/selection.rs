//! Selection and limits (docs/SYNC.md §4.4).
//!
//! Selectable are individual documents, tags and saved views; the result is the
//! union. Each entry is resolved individually via `/sync/resolve`, so the client can show before
//! adopting which entry contributes how much. Limits per profile: `max_bytes`
//! (default 2 GiB) and `max_documents` (default 5,000); file mode `none` counts no bytes.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::remote::{ResolvedDoc, ResolvedSize};
use crate::store::SelectionItem;

pub const DEFAULT_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const DEFAULT_MAX_DOCUMENTS: u64 = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Limits {
    pub max_bytes: u64,
    pub max_documents: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_bytes: DEFAULT_MAX_BYTES, max_documents: DEFAULT_MAX_DOCUMENTS }
    }
}

/// Welche Dateien eines Dokuments lokal liegen sollen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Wants {
    /// Archive PDF if present, otherwise original.
    pub preferred: bool,
    pub original: bool,
    pub archive: bool,
}

impl Wants {
    pub fn from_mode(mode: &str) -> Wants {
        match mode {
            "original" => Wants { original: true, ..Default::default() },
            "both" => Wants { original: true, archive: true, ..Default::default() },
            "none" => Wants::default(),
            _ => Wants { preferred: true, ..Default::default() },
        }
    }

    pub fn union(self, o: Wants) -> Wants {
        Wants { preferred: self.preferred || o.preferred, original: self.original || o.original, archive: self.archive || o.archive }
    }

    pub fn any(self) -> bool {
        self.preferred || self.original || self.archive
    }

    /// Concrete variants for a document (`archive` only if there is one).
    pub fn variants(self, has_archive: bool) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.original || (self.preferred && !has_archive) {
            v.push("original");
        }
        if has_archive && (self.archive || self.preferred) {
            v.push("archive");
        }
        v
    }

    /// Bytes laut `/sync/resolve`.
    pub fn bytes(self, size: &ResolvedSize) -> u64 {
        let has_archive = size.archive.is_some();
        self.variants(has_archive)
            .into_iter()
            .map(|v| if v == "archive" { size.archive.unwrap_or(0) } else { size.original.unwrap_or(0) })
            .sum()
    }
}

/// Beitrag eines Auswahleintrags.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EntryReport {
    #[serde(flatten)]
    pub item: SelectionItem,
    pub documents: u64,
    pub bytes: u64,
}

/// A document of the resolved selection.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlannedDoc {
    pub rev: i64,
    pub wants: Wants,
    pub size: SizeOut,
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct SizeOut {
    pub original: Option<u64>,
    pub archive: Option<u64>,
}

/// A selection entry the server could not resolve (skipped, the rest of the selection is used).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SkippedEntry {
    pub item: SelectionItem,
    pub reason: String,
}

impl SkippedEntry {
    /// `type key: reason` (for reports/logs).
    pub fn describe(&self) -> String {
        format!("{} {}: {}", self.item.kind, self.item.key, self.reason)
    }
}

/// Result of the resolution + check against the limits.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Plan {
    pub entries: Vec<EntryReport>,
    pub total_documents: u64,
    pub total_bytes: u64,
    pub limits: Limits,
    pub within_limits: bool,
    /// Hint text on exceeding (which entries contribute how much).
    pub message: Option<String>,
    #[serde(skip)]
    pub docs: BTreeMap<String, PlannedDoc>,
    pub as_of_seq: Option<i64>,
    /// Entries the server could not resolve (e.g. saved view with a full-text rule); they
    /// contribute nothing.
    pub skipped: Vec<SkippedEntry>,
}

fn human(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes as f64 >= 1024.0 * MIB {
        format!("{:.1} GiB", bytes as f64 / (1024.0 * MIB))
    } else {
        format!("{:.1} MiB", bytes as f64 / MIB)
    }
}

/// Plan from the individual resolutions (per entry the documents from `/sync/resolve`).
pub fn plan(resolved: Vec<(SelectionItem, Vec<ResolvedDoc>)>, limits: Limits, as_of_seq: Option<i64>) -> Plan {
    let mut docs: BTreeMap<String, PlannedDoc> = BTreeMap::new();
    let mut entries = Vec::new();
    for (item, list) in resolved {
        let w = Wants::from_mode(&item.files);
        let mut bytes = 0;
        for d in &list {
            bytes += w.bytes(&d.size);
            let e = docs.entry(d.uuid.clone()).or_insert_with(|| PlannedDoc {
                rev: d.rev,
                wants: Wants::default(),
                size: SizeOut { original: d.size.original, archive: d.size.archive },
            });
            e.wants = e.wants.union(w);
            e.rev = e.rev.max(d.rev);
        }
        entries.push(EntryReport { item, documents: list.len() as u64, bytes });
    }
    let total_documents = docs.len() as u64;
    let total_bytes: u64 = docs.values().map(|d| d.wants.bytes(&ResolvedSize { original: d.size.original, archive: d.size.archive })).sum();
    let within_limits = total_documents <= limits.max_documents && total_bytes <= limits.max_bytes;
    let message = (!within_limits).then(|| {
        let mut parts = vec![format!(
            "Auswahl: {total_documents} Dokumente / {} (Grenze {} Dokumente / {}).",
            human(total_bytes),
            limits.max_documents,
            human(limits.max_bytes)
        )];
        let mut sorted: Vec<&EntryReport> = entries.iter().collect();
        sorted.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(b.documents.cmp(&a.documents)));
        for e in sorted {
            parts.push(format!("{} {}: {} Dokumente, {}", e.item.kind, e.item.key, e.documents, human(e.bytes)));
        }
        parts.join(" ")
    });
    Plan { entries, total_documents, total_bytes, limits, within_limits, message, docs, as_of_seq, skipped: Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rd(uuid: &str, orig: u64, arch: Option<u64>) -> ResolvedDoc {
        ResolvedDoc { uuid: uuid.into(), rev: 1, size: ResolvedSize { original: Some(orig), archive: arch } }
    }

    fn item(kind: &str, key: &str, files: &str) -> SelectionItem {
        SelectionItem { kind: kind.into(), key: key.into(), files: files.into() }
    }

    #[test]
    fn wants_variants() {
        assert_eq!(Wants::from_mode("preferred").variants(true), vec!["archive"]);
        assert_eq!(Wants::from_mode("preferred").variants(false), vec!["original"]);
        assert_eq!(Wants::from_mode("both").variants(true), vec!["original", "archive"]);
        assert_eq!(Wants::from_mode("both").variants(false), vec!["original"]);
        assert!(Wants::from_mode("none").variants(true).is_empty());
        let s = ResolvedSize { original: Some(10), archive: Some(30) };
        assert_eq!(Wants::from_mode("preferred").bytes(&s), 30);
        assert_eq!(Wants::from_mode("original").bytes(&s), 10);
        assert_eq!(Wants::from_mode("both").bytes(&s), 40);
        assert_eq!(Wants::from_mode("none").bytes(&s), 0);
    }

    #[test]
    fn union_and_limits() {
        let p = plan(
            vec![
                (item("tag", "t1", "preferred"), vec![rd("a", 10, Some(30)), rd("b", 5, None)]),
                (item("document", "a", "original"), vec![rd("a", 10, Some(30))]),
                (item("saved_view", "v", "none"), vec![rd("c", 1000, None)]),
            ],
            Limits { max_bytes: 100, max_documents: 3 },
            Some(7),
        );
        assert_eq!(p.total_documents, 3);
        // a: archive (preferred) + original = 40, b: 5, c: 0
        assert_eq!(p.total_bytes, 45);
        assert!(p.within_limits);
        assert_eq!(p.entries[0].bytes, 35);
        assert_eq!(p.docs["a"].wants.variants(true), vec!["original", "archive"]);

        let p = plan(vec![(item("tag", "t1", "both"), vec![rd("a", 60, Some(60))])], Limits { max_bytes: 100, max_documents: 3 }, None);
        assert!(!p.within_limits);
        assert!(p.message.unwrap().contains("tag t1: 1 Dokumente"));
        let p = plan(
            vec![(item("tag", "t1", "none"), vec![rd("a", 1, None), rd("b", 1, None)])],
            Limits { max_bytes: 0, max_documents: 1 },
            None,
        );
        assert!(!p.within_limits, "document count limit");
    }
}
