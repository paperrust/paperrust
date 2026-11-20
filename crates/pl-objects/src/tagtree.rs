//! Tag hierarchy as an adjacency list (edge = `documents_tag.tn_parent_id`), without stored
//! derivations. Replaces the django-treenode columns (migration
//! `pl_objects.0001_paperrust_tag_adjacency`).
//!
//! Idea like upstream #14612 (paperless-ngx after v3.4.2, commit 8aa99a381: `Tag.get_hierarchy()`,
//! `documents/tag_hierarchy.py`), own implementation. Rules of `Tag.clean()` (v3.4.2 and #14612
//! the same): no own parent, no descendant as parent, depth at most
//! [`MAX_NESTING_DEPTH`].
//!
//! A [`TagTree`] is a snapshot for **one** operation (one query, all tags; a few KB for
//! a few thousand tags) – never cache globally, other writers change parents.

use std::collections::{HashMap, HashSet};

use sqlx::PgConnection;

/// `Tag.MAX_NESTING_DEPTH` (root = level 1).
pub const MAX_NESTING_DEPTH: usize = 5;

/// Edges of all tags: id → parent, parent → children (children in insertion order, i.e.
/// by id when from [`TagTree::load`]).
#[derive(Clone, Debug, Default)]
pub struct TagTree {
    parent: HashMap<i64, Option<i64>>,
    children: HashMap<i64, Vec<i64>>,
}

impl TagTree {
    /// From `(id, parent)` pairs. Unknown parents remain as an edge (like the DB).
    pub fn from_edges(edges: impl IntoIterator<Item = (i64, Option<i64>)>) -> Self {
        let mut t = TagTree::default();
        for (id, p) in edges {
            t.parent.insert(id, p);
            if let Some(p) = p {
                t.children.entry(p).or_default().push(id);
            }
        }
        t
    }

    /// All edges in one query (`SELECT id, tn_parent_id FROM documents_tag ORDER BY id`).
    pub async fn load(conn: &mut PgConnection) -> Result<Self, sqlx::Error> {
        let rows: Vec<(i32, Option<i32>)> =
            sqlx::query_as("SELECT id, tn_parent_id FROM documents_tag ORDER BY id").fetch_all(&mut *conn).await?;
        Ok(Self::from_edges(rows.into_iter().map(|(i, p)| (i64::from(i), p.map(i64::from)))))
    }

    /// Like [`TagTree::load`], over a pool.
    pub async fn load_pool(db: &sqlx::PgPool) -> Result<Self, sqlx::Error> {
        let mut conn = db.acquire().await?;
        Self::load(&mut conn).await
    }

    pub fn contains(&self, id: i64) -> bool {
        self.parent.contains_key(&id)
    }

    pub fn parent(&self, id: i64) -> Option<i64> {
        self.parent.get(&id).copied().flatten()
    }

    /// Direct children (by id).
    pub fn children(&self, id: i64) -> &[i64] {
        self.children.get(&id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Ancestors of `id`, from the root to the parent (`get_ancestors_pks()`); safe against
    /// cycles in faulty data (stops at the first repeated node).
    pub fn ancestors(&self, id: i64) -> Vec<i64> {
        self.chain_from(self.parent(id))
    }

    /// Chain upwards from `start` (incl. `start`), root first.
    fn chain_from(&self, start: Option<i64>) -> Vec<i64> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut cur = start;
        while let Some(p) = cur {
            if !seen.insert(p) {
                break;
            }
            out.push(p);
            cur = self.parent(p);
        }
        out.reverse();
        out
    }

    /// All descendants of the `roots` without the `roots` themselves (`get_descendants_pks()`).
    pub fn descendants(&self, roots: impl IntoIterator<Item = i64>) -> HashSet<i64> {
        let roots: HashSet<i64> = roots.into_iter().collect();
        let mut seen = roots.clone();
        let mut stack: Vec<i64> = roots.iter().copied().collect();
        while let Some(n) = stack.pop() {
            for c in self.children(n) {
                if seen.insert(*c) {
                    stack.push(*c);
                }
            }
        }
        seen.retain(|x| !roots.contains(x));
        seen
    }

    /// `roots` plus all their ancestors (`add_nested_tags`, workflow "assign tags").
    pub fn with_ancestors(&self, roots: impl IntoIterator<Item = i64>) -> HashSet<i64> {
        let mut out = HashSet::new();
        for r in roots {
            out.insert(r);
            out.extend(self.ancestors(r));
        }
        out
    }

    /// `roots` plus all their descendants (remove tags, delete).
    pub fn with_descendants(&self, roots: impl IntoIterator<Item = i64>) -> HashSet<i64> {
        let roots: Vec<i64> = roots.into_iter().collect();
        let mut out = self.descendants(roots.iter().copied());
        out.extend(roots);
        out
    }

    /// Height of the subtree under `id` (0 = no children; treenode `tn_depth`).
    pub fn height(&self, id: i64) -> usize {
        let mut best = 0;
        let mut seen = HashSet::from([id]);
        let mut stack = vec![(id, 0usize)];
        while let Some((n, d)) = stack.pop() {
            best = best.max(d);
            for c in self.children(n) {
                if seen.insert(*c) {
                    stack.push((*c, d + 1));
                }
            }
        }
        best
    }

    /// `Tag.clean()` with `tn_parent = new_parent` for the tag `this` (`None` = new tag).
    /// Error text for the key `parent` (TagSerializer.validate).
    pub fn check_parent(&self, this: Option<i64>, new_parent: Option<i64>) -> Result<(), &'static str> {
        if let (Some(t), Some(p)) = (this, new_parent)
            && t == p
        {
            return Err("Cannot set itself as parent.");
        }
        let chain = self.chain_from(new_parent);
        if let Some(t) = this
            && chain.contains(&t)
        {
            return Err("Cannot set parent to a descendant.");
        }
        let height = this.map(|t| self.height(t)).unwrap_or(0);
        if chain.len() + 1 + height > MAX_NESTING_DEPTH {
            return Err("Maximum nesting depth exceeded.");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tree of the reference seed data (reference/seed.py).
    fn seed() -> TagTree {
        TagTree::from_edges([
            (1, None), (2, None), (3, Some(2)), (4, Some(3)), (5, Some(3)), (6, Some(2)), (7, Some(6)), (8, Some(6)),
            (9, Some(8)), (10, None), (11, Some(10)), (12, Some(10)), (13, None),
        ])
    }

    fn set(v: &[i64]) -> HashSet<i64> {
        v.iter().copied().collect()
    }

    #[test]
    fn ancestors_root_first() {
        let t = seed();
        assert_eq!(t.ancestors(9), vec![2, 6, 8]);
        assert_eq!(t.ancestors(2), Vec::<i64>::new());
        assert_eq!(t.ancestors(999), Vec::<i64>::new());
    }

    #[test]
    fn descendants_and_closures() {
        let t = seed();
        assert_eq!(t.descendants([2]), set(&[3, 4, 5, 6, 7, 8, 9]));
        assert_eq!(t.descendants([6, 8]), set(&[7, 9])); // 8 is the root of the request → not included
        assert_eq!(t.with_descendants([10]), set(&[10, 11, 12]));
        assert_eq!(t.with_ancestors([9, 12]), set(&[9, 8, 6, 2, 12, 10]));
        assert_eq!(t.children(10), &[11, 12]);
    }

    #[test]
    fn heights_match_treenode_depth() {
        let t = seed();
        assert_eq!(t.height(2), 3);
        assert_eq!(t.height(6), 2);
        assert_eq!(t.height(9), 0);
        assert_eq!(t.height(13), 0);
    }

    #[test]
    fn clean_rules_like_upstream() {
        let t = seed();
        assert_eq!(t.check_parent(Some(2), Some(2)), Err("Cannot set itself as parent."));
        assert_eq!(t.check_parent(Some(2), Some(9)), Err("Cannot set parent to a descendant."));
        // 9 is on level 4 → new child on level 5 allowed
        assert_eq!(t.check_parent(None, Some(9)), Ok(()));
        // subtree 10 (height 1) under 9 → level 5 + 1 = 6
        assert_eq!(t.check_parent(Some(10), Some(9)), Err("Maximum nesting depth exceeded."));
        assert_eq!(t.check_parent(Some(10), Some(8)), Ok(()));
        assert_eq!(t.check_parent(Some(3), None), Ok(()));
        let deep = TagTree::from_edges([(1, None), (2, Some(1)), (3, Some(2)), (4, Some(3)), (5, Some(4))]);
        assert_eq!(deep.check_parent(None, Some(5)), Err("Maximum nesting depth exceeded."));
    }

    #[test]
    fn cycles_do_not_hang() {
        let t = TagTree::from_edges([(1, Some(2)), (2, Some(1)), (3, Some(3))]);
        assert_eq!(t.ancestors(1), vec![1, 2]);
        assert_eq!(t.descendants([1]), set(&[2]));
        assert_eq!(t.height(1), 1);
        assert_eq!(t.check_parent(Some(3), Some(3)), Err("Cannot set itself as parent."));
    }
}
