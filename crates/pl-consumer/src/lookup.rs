//! Lookup store per ingestion run (docs/DATAMODEL-V2.md §6 "stage 6", docs/PERF-SCALE.md
//! §12.3): when ingesting a document, signals, workflows, file name templates and
//! audit queried the same immutable small things over and over (correspondent name 31×,
//! ContentType 11×, tag ancestors 10× …). Inside [`scope`] the run remembers these
//! answers.
//!
//! Two kinds of entries:
//! * immutable during a run: names of existing labels and users, ContentType ids,
//!   tag ancestors, "object exists". New objects (e.g. tags from subfolders) have new ids and
//!   are not yet in the store; "not present" is not remembered.
//! * changed by the run itself: the document row and the tags of the document. Every write place
//!   in pl-consumer discards the entry ([`forget_doc`], [`forget_docs`], [`forget_tags`]); it is
//!   then read from the database again.
//!
//! Without [`scope`] (all other callers) every function queries the database as before.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;

#[derive(Default)]
pub struct Lookup {
    names: HashMap<(String, i64), String>,
    users: HashMap<i64, String>,
    ctypes: HashMap<(String, String), i32>,
    ancestors: HashMap<i64, Vec<i64>>,
    ancestor_sets: HashMap<Vec<i64>, Vec<i64>>,
    existing: std::collections::HashSet<(String, i64)>,
    docs: HashMap<i64, crate::db::DocRow>,
    doc_tags: HashMap<i64, Vec<(i64, String)>>,
    /// Hits (for tests and profile)
    pub hits: u64,
}

tokio::task_local! {
    static CACHE: RefCell<Lookup>;
}

/// Run `f` with its own lookup store.
pub async fn scope<F: Future>(f: F) -> F::Output {
    CACHE.scope(RefCell::new(Lookup::default()), f).await
}

/// Hits in the current store (0 without [`scope`]).
pub fn hits() -> u64 {
    CACHE.try_with(|c| c.borrow().hits).unwrap_or(0)
}

fn get<T>(f: impl FnOnce(&Lookup) -> Option<T>) -> Option<T> {
    CACHE
        .try_with(|c| {
            let mut c = c.borrow_mut();
            let v = f(&c);
            if v.is_some() {
                c.hits += 1;
            }
            v
        })
        .ok()
        .flatten()
}

fn put(f: impl FnOnce(&mut Lookup)) {
    let _ = CACHE.try_with(|c| f(&mut c.borrow_mut()));
}

pub(crate) fn name(table: &str, id: i64) -> Option<String> {
    get(|c| c.names.get(&(table.to_string(), id)).cloned())
}
pub(crate) fn set_name(table: &str, id: i64, v: &str) {
    put(|c| {
        c.names.insert((table.to_string(), id), v.to_string());
    });
}
pub(crate) fn user(id: i64) -> Option<String> {
    get(|c| c.users.get(&id).cloned())
}
pub(crate) fn set_user(id: i64, v: &str) {
    put(|c| {
        c.users.insert(id, v.to_string());
    });
}
pub(crate) fn ctype(app: &str, model: &str) -> Option<i32> {
    get(|c| c.ctypes.get(&(app.to_string(), model.to_string())).copied())
}
pub(crate) fn set_ctype(app: &str, model: &str, v: i32) {
    put(|c| {
        c.ctypes.insert((app.to_string(), model.to_string()), v);
    });
}
pub(crate) fn ancestors(tag: i64) -> Option<Vec<i64>> {
    get(|c| c.ancestors.get(&tag).cloned())
}
pub(crate) fn set_ancestors(tag: i64, v: &[i64]) {
    put(|c| {
        c.ancestors.insert(tag, v.to_vec());
    });
}
pub(crate) fn ancestor_set(tags: &[i64]) -> Option<Vec<i64>> {
    get(|c| c.ancestor_sets.get(tags).cloned())
}
pub(crate) fn set_ancestor_set(tags: &[i64], v: &[i64]) {
    put(|c| {
        c.ancestor_sets.insert(tags.to_vec(), v.to_vec());
    });
}
pub(crate) fn exists(table: &str, id: i64) -> bool {
    get(|c| c.existing.contains(&(table.to_string(), id)).then_some(())).is_some()
}
pub(crate) fn set_exists(table: &str, id: i64) {
    put(|c| {
        c.existing.insert((table.to_string(), id));
    });
}
pub(crate) fn doc(id: i64) -> Option<crate::db::DocRow> {
    get(|c| c.docs.get(&id).cloned())
}
pub(crate) fn set_doc(row: &crate::db::DocRow) {
    put(|c| {
        c.docs.insert(row.id, row.clone());
    });
}
/// Document row was written.
pub fn forget_doc(id: i64) {
    put(|c| {
        c.docs.remove(&id);
    });
}
/// Document rows were written with a statement whose target is not individually known here.
pub fn forget_docs() {
    put(|c| c.docs.clear());
}
pub(crate) fn doc_tags(id: i64) -> Option<Vec<(i64, String)>> {
    get(|c| c.doc_tags.get(&id).cloned())
}
pub(crate) fn set_doc_tags(id: i64, v: &[(i64, String)]) {
    put(|c| {
        c.doc_tags.insert(id, v.to_vec());
    });
}
/// Tag assignment of the document was written.
pub fn forget_tags(id: i64) {
    put(|c| {
        c.doc_tags.remove(&id);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_inside_scope() {
        set_name("documents_tag", 1, "a");
        assert_eq!(name("documents_tag", 1), None, "ohne scope nichts merken");
        scope(async {
            assert_eq!(name("documents_tag", 1), None);
            set_name("documents_tag", 1, "a");
            set_ctype("documents", "document", 7);
            assert_eq!(name("documents_tag", 1).as_deref(), Some("a"));
            assert_eq!(name("documents_correspondent", 1), None);
            assert_eq!(ctype("documents", "document"), Some(7));
            assert_eq!(hits(), 2);
        })
        .await;
        assert_eq!(hits(), 0);
    }
}
