//! Exact port of Python's `difflib.SequenceMatcher` (ratio, quick_ratio, real_quick_ratio)
//! and `difflib.get_close_matches` for strings (comparison on Unicode code points like
//! Python `str`).
//!
//! upstream: CPython Lib/difflib.py (3.12/3.13 – unchanged since 3.2: `autojunk=True`,
//! `isjunk=None`). Used by `paperless_ai/matching.py@v3.4.2::_match_names_to_queryset`
//! with `cutoff=MATCH_THRESHOLD (0.8)` and `n=1`.
//!
//! Special features that are reproduced exactly:
//! * `autojunk`: if `b` (the *second* sequence, in `get_close_matches` the searched word) is
//!   at least 200 characters long, characters occurring more often than `len(b)//100 + 1`
//!   count as "popular" and are removed from the index (but not treated as junk).
//! * `get_close_matches` sorts via `heapq.nlargest` by `(score, x)`: on equal score
//!   the lexicographically *greater* string wins.

use std::collections::{HashMap, HashSet};

/// `SequenceMatcher(None, a, b)`.
pub struct SequenceMatcher {
    a: Vec<char>,
    b: Vec<char>,
    b2j: HashMap<char, Vec<usize>>,
    fullbcount: Option<HashMap<char, usize>>,
}

impl SequenceMatcher {
    /// `SequenceMatcher(None, a, b)` (autojunk=True).
    pub fn new(a: &str, b: &str) -> Self {
        let mut s = Self { a: a.chars().collect(), b: Vec::new(), b2j: HashMap::new(), fullbcount: None };
        s.set_seq2(b);
        s
    }

    /// `set_seq1(a)`.
    pub fn set_seq1(&mut self, a: &str) {
        self.a = a.chars().collect();
    }

    /// `set_seq2(b)` incl. `__chain_b` (autojunk).
    pub fn set_seq2(&mut self, b: &str) {
        self.b = b.chars().collect();
        self.fullbcount = None;
        let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
        for (i, c) in self.b.iter().enumerate() {
            b2j.entry(*c).or_default().push(i);
        }
        let n = self.b.len();
        if n >= 200 {
            let ntest = n / 100 + 1;
            let popular: HashSet<char> = b2j.iter().filter(|(_, idx)| idx.len() > ntest).map(|(c, _)| *c).collect();
            for c in popular {
                b2j.remove(&c);
            }
        }
        self.b2j = b2j;
    }

    /// `find_longest_match(alo, ahi, blo, bhi)` → (i, j, k). `bjunk` is empty (isjunk=None),
    /// so the junk extension loops are omitted.
    fn find_longest_match(&self, alo: usize, ahi: usize, blo: usize, bhi: usize) -> (usize, usize, usize) {
        let (a, b) = (&self.a, &self.b);
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for (i, ai) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut newj2len: HashMap<usize, usize> = HashMap::new();
            if let Some(js) = self.b2j.get(ai) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let prev = if j == 0 { 0 } else { j2len.get(&(j - 1)).copied().unwrap_or(0) };
                    let k = prev + 1;
                    newj2len.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = newj2len;
        }
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi && bestj + bestsize < bhi && a[besti + bestsize] == b[bestj + bestsize] {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    /// Sum of the lengths of all `get_matching_blocks()`.
    fn matches(&self) -> usize {
        let (la, lb) = (self.a.len(), self.b.len());
        let mut queue = vec![(0usize, la, 0usize, lb)];
        let mut total = 0usize;
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.find_longest_match(alo, ahi, blo, bhi);
            if k > 0 {
                total += k;
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        total
    }

    fn calc(matches: usize, length: usize) -> f64 {
        if length > 0 { 2.0 * matches as f64 / length as f64 } else { 1.0 }
    }

    /// `ratio()`.
    pub fn ratio(&self) -> f64 {
        Self::calc(self.matches(), self.a.len() + self.b.len())
    }

    /// `quick_ratio()` (upper bound via character frequencies).
    pub fn quick_ratio(&mut self) -> f64 {
        if self.fullbcount.is_none() {
            let mut m: HashMap<char, usize> = HashMap::new();
            for c in &self.b {
                *m.entry(*c).or_default() += 1;
            }
            self.fullbcount = Some(m);
        }
        let full = self.fullbcount.as_ref().map(|m| m as &HashMap<char, usize>);
        let mut avail: HashMap<char, isize> = HashMap::new();
        let mut matches = 0usize;
        for c in &self.a {
            let numb = match avail.get(c) {
                Some(v) => *v,
                None => full.and_then(|m| m.get(c)).copied().unwrap_or(0) as isize,
            };
            avail.insert(*c, numb - 1);
            if numb > 0 {
                matches += 1;
            }
        }
        Self::calc(matches, self.a.len() + self.b.len())
    }

    /// `real_quick_ratio()`.
    pub fn real_quick_ratio(&self) -> f64 {
        let (la, lb) = (self.a.len(), self.b.len());
        Self::calc(la.min(lb), la + lb)
    }
}

/// `difflib.SequenceMatcher(None, a, b).ratio()`.
pub fn ratio(a: &str, b: &str) -> f64 {
    SequenceMatcher::new(a, b).ratio()
}

/// `difflib.get_close_matches(word, possibilities, n, cutoff)`.
pub fn get_close_matches<'a>(word: &str, possibilities: &[&'a str], n: usize, cutoff: f64) -> Vec<&'a str> {
    if n == 0 {
        return Vec::new();
    }
    let mut s = SequenceMatcher::new("", word);
    let mut result: Vec<(f64, &'a str)> = Vec::new();
    for x in possibilities {
        s.set_seq1(x);
        if s.real_quick_ratio() >= cutoff && s.quick_ratio() >= cutoff {
            let r = s.ratio();
            if r >= cutoff {
                result.push((r, x));
            }
        }
    }
    // heapq.nlargest(n, result): descending by (score, x); Python compares str by code points,
    // which corresponds to Rust's `str::cmp` (UTF-8 bytes).
    result.sort_by(|l, r| r.0.total_cmp(&l.0).then_with(|| r.1.cmp(l.1)));
    result.truncate(n);
    result.into_iter().map(|(_, x)| x).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_doc_examples() {
        assert_eq!(get_close_matches("appel", &["ape", "apple", "peach", "puppy"], 3, 0.6), vec!["apple", "ape"]);
        let s = SequenceMatcher::new("abcd", "bcde");
        assert!((s.ratio() - 0.75).abs() < 1e-12);
    }

    #[test]
    fn empty() {
        assert_eq!(ratio("", ""), 1.0);
        assert_eq!(ratio("a", ""), 0.0);
    }
}
