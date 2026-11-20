//! Iteration order of a CPython `set` of non-negative integers (hash(i) = i).
//!
//! upstream: CPython 3.12 Objects/setobject.c (`set_add_entry`, `set_table_resize`,
//! `set_insert_clean`, `set_merge`, `set_copy_and_difference`). Django creates m2m rows in the
//! order of `_get_missing_target_ids()` (`set(objs) - existing`), see
//! django/db/models/fields/related_descriptors.py `_add_items` – so that the IDs of the
//! intermediate tables are the same after an import.
//!
//! Taken over from pl-cli (`pyset.rs`) (W11), so that pl-consumer creates m2m rows in the same order
//! as Django; pl-cli should switch to this version (CORE-REQUEST).

const MINSIZE: usize = 8;
const LINEAR_PROBES: usize = 9;
const PERTURB_SHIFT: u32 = 5;

#[derive(Debug, Clone)]
pub struct PySet {
    table: Vec<Option<u64>>,
    fill: usize,
    used: usize,
}

impl Default for PySet {
    fn default() -> Self {
        PySet { table: vec![None; MINSIZE], fill: 0, used: 0 }
    }
}

fn insert_clean(table: &mut [Option<u64>], key: u64) {
    let mask = table.len() - 1;
    let mut perturb = key as usize;
    let mut i = key as usize & mask;
    loop {
        if table[i].is_none() {
            table[i] = Some(key);
            return;
        }
        if i + LINEAR_PROBES <= mask {
            for j in 1..=LINEAR_PROBES {
                if table[i + j].is_none() {
                    table[i + j] = Some(key);
                    return;
                }
            }
        }
        perturb >>= PERTURB_SHIFT;
        i = (i.wrapping_mul(5).wrapping_add(1).wrapping_add(perturb)) & mask;
    }
}

impl PySet {
    fn mask(&self) -> usize {
        self.table.len() - 1
    }

    fn resize(&mut self, minused: usize) {
        let mut newsize = MINSIZE;
        while newsize <= minused {
            newsize <<= 1;
        }
        let old = std::mem::replace(&mut self.table, vec![None; newsize]);
        for k in old.into_iter().flatten() {
            insert_clean(&mut self.table, k);
        }
        self.fill = self.used;
    }

    /// `set.add(key)`.
    pub fn add(&mut self, key: u64) {
        let mask = self.mask();
        let mut i = key as usize & mask;
        let mut perturb = key as usize;
        loop {
            let probes = if i + LINEAR_PROBES <= mask { LINEAR_PROBES } else { 0 };
            for j in 0..=probes {
                match self.table[i + j] {
                    None => {
                        self.table[i + j] = Some(key);
                        self.fill += 1;
                        self.used += 1;
                        if self.fill * 5 >= mask * 3 {
                            let m = if self.used > 50000 { self.used * 2 } else { self.used * 4 };
                            self.resize(m);
                        }
                        return;
                    }
                    Some(k) if k == key => return,
                    _ => {}
                }
            }
            perturb >>= PERTURB_SHIFT;
            i = (i.wrapping_mul(5).wrapping_add(1).wrapping_add(perturb)) & mask;
        }
    }

    /// `set(self)` (copy via `set_merge` into an empty set).
    pub fn copy(&self) -> PySet {
        let mut n = PySet::default();
        if self.used == 0 {
            return n;
        }
        if (n.fill + self.used) * 5 >= n.mask() * 3 {
            n.resize((n.used + self.used) * 2);
        }
        if n.mask() == self.mask() && self.fill == self.used {
            n.table = self.table.clone();
        } else {
            for k in self.table.iter().flatten() {
                insert_clean(&mut n.table, *k);
            }
        }
        n.fill = self.used;
        n.used = self.used;
        n
    }

    pub fn iter(&self) -> impl Iterator<Item = u64> + '_ {
        self.table.iter().flatten().copied()
    }
}

/// Order in which Django inserts the missing m2m targets: `set()` from `ids` (added in order),
/// then `.difference(existing)` (copy). Negative IDs stay in input order.
pub fn missing_targets_order(ids: &[i64]) -> Vec<i64> {
    if ids.iter().any(|i| *i < 0) {
        let mut seen = std::collections::HashSet::new();
        return ids.iter().copied().filter(|i| seen.insert(*i)).collect();
    }
    let mut s = PySet::default();
    for i in ids {
        s.add(*i as u64);
    }
    s.copy().iter().map(|k| k as i64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_cpython() {
        // Values checked with CPython 3.12: list(set([9, 4]).difference([])) etc.
        assert_eq!(missing_targets_order(&[9, 4]), vec![9, 4]);
        assert_eq!(missing_targets_order(&[4, 9]), vec![9, 4]);
        assert_eq!(missing_targets_order(&[3, 1, 2]), vec![1, 2, 3]);
        assert_eq!(missing_targets_order(&[8, 6]), vec![8, 6]);
        assert_eq!(missing_targets_order(&[1, 2, 3, 4, 5, 6, 7, 20, 33]), vec![1, 2, 3, 4, 5, 6, 7, 33, 20]);
        assert_eq!(missing_targets_order(&[64, 128, 98, 97, 49, 50, 53, 65, 70]), vec![64, 97, 98, 128, 65, 70, 49, 50, 53]);
        let v: Vec<i64> = (1..=40).rev().step_by(3).collect();
        assert_eq!(missing_targets_order(&v), vec![1, 34, 4, 37, 7, 40, 10, 13, 16, 19, 22, 25, 28, 31]);
    }
}
