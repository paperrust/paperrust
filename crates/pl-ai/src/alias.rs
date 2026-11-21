//! Aliases (extension): confirmed mappings "other name → existing object".
//! Table `paperrust_taxonomy_alias` (migration `migrations/0001_paperrust_ai.sql`, not active yet).
//!
//! Storage logic:
//! * The key is `(kind, alias_key, target_id)`; `alias_key` = [`crate::taxonomy::alias_key`].
//! * An alias is usable only if the caller may see the **target object** – matching
//!   only receives aliases of visible targets (backends filter via a join on the target).
//! * Creation only if the user may **change** the target object (`change_<model>`).
//! * An alias that equals the name of *another* visible object of the same kind
//!   (same core) is rejected – otherwise the alias would shadow a real object.
//! * No alias to itself (core = core of the target name): would have no effect.

use std::sync::Mutex;

use crate::taxonomy::{Alias, TaxonomyEntry, TaxonomyKind, alias_key};

/// Why an alias is not stored.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AliasError {
    #[error("alias is empty")]
    Empty,
    #[error("alias is identical to the target name")]
    SameAsTarget,
    #[error("alias collides with existing {kind} #{id} \"{name}\"")]
    Collides { kind: &'static str, id: i64, name: String },
    #[error("target not found or not visible")]
    TargetNotFound,
    #[error("storage error")]
    Storage,
}

/// Checks a new alias against the visible objects. Returns the finished record.
pub fn validate_alias(kind: TaxonomyKind, target_id: i64, alias: &str, visible: &[TaxonomyEntry]) -> Result<Alias, AliasError> {
    let alias = alias.trim();
    let key = alias_key(kind, alias);
    if key.is_empty() {
        return Err(AliasError::Empty);
    }
    let target = visible.iter().find(|e| e.kind == kind && e.id == target_id).ok_or(AliasError::TargetNotFound)?;
    if alias_key(kind, &target.name) == key {
        return Err(AliasError::SameAsTarget);
    }
    if let Some(other) = visible.iter().find(|e| e.kind == kind && e.id != target_id && alias_key(kind, &e.name) == key) {
        return Err(AliasError::Collides { kind: kind.as_str(), id: other.id, name: other.name.clone() });
    }
    Ok(Alias { kind, target_id, alias: alias.to_string(), alias_key: key, owner_id: target.owner_id })
}

/// Storage backend for tests and operation without a DB.
#[derive(Default)]
pub struct MemoryAliasStore {
    inner: Mutex<Vec<Alias>>,
}

impl MemoryAliasStore {
    pub fn add(&self, a: Alias) -> Result<bool, AliasError> {
        let mut g = self.inner.lock().map_err(|_| AliasError::Storage)?;
        if g.iter().any(|x| x.kind == a.kind && x.alias_key == a.alias_key && x.target_id == a.target_id) {
            return Ok(false);
        }
        g.push(a);
        Ok(true)
    }

    /// All aliases of a kind.
    pub fn visible_unfiltered(&self, kind: TaxonomyKind) -> Vec<Alias> {
        let Ok(g) = self.inner.lock() else { return Vec::new() };
        g.iter().filter(|a| a.kind == kind).cloned().collect()
    }

    /// Aliases of a kind whose target is in `visible`.
    pub fn visible(&self, kind: TaxonomyKind, visible: &[TaxonomyEntry]) -> Vec<Alias> {
        let Ok(g) = self.inner.lock() else { return Vec::new() };
        g.iter().filter(|a| a.kind == kind && visible.iter().any(|e| e.kind == kind && e.id == a.target_id)).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: i64, name: &str) -> TaxonomyEntry {
        TaxonomyEntry { kind: TaxonomyKind::Correspondent, id, name: name.into(), owner_id: None, extra: None, document_count: None }
    }

    #[test]
    fn rules() {
        let v = vec![e(1, "Swisscom (Schweiz) AG"), e(2, "Sunrise GmbH")];
        let k = TaxonomyKind::Correspondent;
        assert!(validate_alias(k, 1, "Blue Entertainment", &v).is_ok());
        assert_eq!(validate_alias(k, 1, "Swisscom AG", &v), Err(AliasError::SameAsTarget));
        assert!(matches!(validate_alias(k, 1, "Sunrise", &v), Err(AliasError::Collides { id: 2, .. })));
        assert_eq!(validate_alias(k, 9, "x", &v), Err(AliasError::TargetNotFound));
        assert_eq!(validate_alias(k, 1, "  ", &v), Err(AliasError::Empty));
        let s = MemoryAliasStore::default();
        let a = validate_alias(k, 1, "Blue Entertainment", &v).unwrap();
        assert_eq!(s.add(a.clone()), Ok(true));
        assert_eq!(s.add(a), Ok(false));
        assert_eq!(s.visible(k, &v).len(), 1);
        assert_eq!(s.visible(k, &v[1..]).len(), 0, "Ziel unsichtbar → Alias unsichtbar");
    }
}
