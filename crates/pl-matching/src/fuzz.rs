// upstream: rapidfuzz 3.14.6 `fuzz.partial_ratio` (rapidfuzz/fuzz_py.py:
//           partial_ratio_alignment, _partial_ratio_impl; distance/Indel_py.py)
//
// Used by src/documents/matching.py@v3.4.2 (MATCH_FUZZY) as
// `fuzz.partial_ratio(match, text, score_cutoff=90)`, i.e. no processor.
// Sequences are compared per Unicode code point (Python str elements).
// The float arithmetic follows the Python reference implementation step by step
// so that results at the threshold are bit-identical.

use std::collections::{HashMap, HashSet};

/// LCS length of `s1` (pattern) and `s2` using Hyyrö's bit-parallel algorithm.
fn lcs_len(s1: &[char], s2: &[char]) -> usize {
    if s1.is_empty() || s2.is_empty() {
        return 0;
    }
    let words = s1.len().div_ceil(64);
    let mut peq: HashMap<char, Vec<u64>> = HashMap::new();
    for (i, &c) in s1.iter().enumerate() {
        peq.entry(c).or_insert_with(|| vec![0u64; words])[i / 64] |= 1u64 << (i % 64);
    }
    let mut v = vec![u64::MAX; words];
    let zero = vec![0u64; words];
    for c in s2 {
        let m = peq.get(c).unwrap_or(&zero);
        let mut carry = 0u64;
        for w in 0..words {
            let u = v[w] & m[w];
            // x = v + u + carry (multiword add), v = x | (v - u)
            let (s1v, c1) = v[w].overflowing_add(u);
            let (s2v, c2) = s1v.overflowing_add(carry);
            carry = (c1 || c2) as u64;
            v[w] = s2v | (v[w] - u);
        }
    }
    // valid bits only
    let mut ones = 0usize;
    for (w, &val) in v.iter().enumerate() {
        let bits = if w == words - 1 && !s1.len().is_multiple_of(64) { s1.len() % 64 } else { 64 };
        let mask = if bits == 64 { u64::MAX } else { (1u64 << bits) - 1 };
        ones += ((!val) & mask).count_ones() as usize;
    }
    ones
}

/// `rapidfuzz.fuzz.ratio(s1, s2, score_cutoff=cutoff)` on already processed strings
/// (rapidfuzz/fuzz_py.py `ratio`): `Indel.normalized_similarity(score_cutoff=cutoff/100) * 100`
/// (comparison in normalised space). Used by `document_fuzzy_match` (pl-cli).
pub fn ratio(s1: &[char], s2: &[char], score_cutoff: f64) -> f64 {
    let cutoff = score_cutoff / 100.0;
    let maximum = s1.len() + s2.len();
    if maximum == 0 {
        return 100.0;
    }
    // Upper bound without LCS (lcs ≤ shorter length): safely below → 0 like rapidfuzz
    let upper = 2.0 * s1.len().min(s2.len()) as f64 / maximum as f64;
    if upper + 1e-12 < cutoff {
        return 0.0;
    }
    let (short, long) = if s1.len() <= s2.len() { (s1, s2) } else { (s2, s1) };
    let lcs = lcs_len(short, long);
    let dist = maximum - 2 * lcs;
    let norm_sim = 1.0 - dist as f64 / maximum as f64;
    if norm_sim >= cutoff { norm_sim * 100.0 } else { 0.0 }
}

/// Indel `_block_normalized_similarity(block, s1, s2, score_cutoff)`.
fn indel_normalized_similarity(s1: &[char], s2: &[char], score_cutoff: f64) -> f64 {
    let maximum = s1.len() + s2.len();
    let lcs = lcs_len(s1, s2);
    let dist = maximum - 2 * lcs;
    let norm_dist = if maximum != 0 { dist as f64 / maximum as f64 } else { 0.0 };
    let norm_sim = 1.0 - norm_dist;
    if norm_sim >= score_cutoff { norm_sim } else { 0.0 }
}

/// `_partial_ratio_impl` – assumes `s1.len() <= s2.len()`. Returns score in 0..=100.
fn partial_ratio_impl(s1: &[char], s2: &[char], mut score_cutoff: f64) -> f64 {
    let set: HashSet<char> = s1.iter().copied().collect();
    let len1 = s1.len();
    let len2 = s2.len();
    let mut score = 0.0f64;
    if len1 == 0 {
        // every `s2[...] not in s1_char_set` check skips -> score 0
        return 0.0;
    }

    for i in 1..len1 {
        if !set.contains(&s2[i - 1]) {
            continue;
        }
        let r = indel_normalized_similarity(s1, &s2[..i], score_cutoff);
        if r > score {
            score = r;
            score_cutoff = r;
            if score == 1.0 {
                return 100.0;
            }
        }
    }
    for i in 0..len2.saturating_sub(len1) {
        if !set.contains(&s2[i + len1 - 1]) {
            continue;
        }
        let r = indel_normalized_similarity(s1, &s2[i..i + len1], score_cutoff);
        if r > score {
            score = r;
            score_cutoff = r;
            if score == 1.0 {
                return 100.0;
            }
        }
    }
    for i in len2.saturating_sub(len1)..len2 {
        if !set.contains(&s2[i]) {
            continue;
        }
        let r = indel_normalized_similarity(s1, &s2[i..], score_cutoff);
        if r > score {
            score = r;
            score_cutoff = r;
            if score == 1.0 {
                return 100.0;
            }
        }
    }
    score * 100.0
}

/// `rapidfuzz.fuzz.partial_ratio(s1, s2, score_cutoff=...)` (no processor).
pub fn partial_ratio(s1: &str, s2: &str, score_cutoff: f64) -> f64 {
    let a: Vec<char> = s1.chars().collect();
    let b: Vec<char> = s2.chars().collect();
    if a.is_empty() && b.is_empty() {
        return 100.0;
    }
    let (shorter, longer) = if a.len() <= b.len() { (&a, &b) } else { (&b, &a) };
    let mut res = partial_ratio_impl(shorter, longer, score_cutoff / 100.0);
    let mut cutoff = score_cutoff;
    if res != 100.0 && a.len() == b.len() {
        cutoff = cutoff.max(res);
        let res2 = partial_ratio_impl(longer, shorter, cutoff / 100.0);
        if res2 > res {
            res = res2;
        }
    }
    if res < cutoff { 0.0 } else { res }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lcs() {
        let a: Vec<char> = "abcde".chars().collect();
        let b: Vec<char> = "ace".chars().collect();
        assert_eq!(lcs_len(&a, &b), 3);
        let long: Vec<char> = "x".repeat(130).chars().collect();
        let long2: Vec<char> = "xy".repeat(100).chars().collect();
        assert_eq!(lcs_len(&long, &long2), 100);
    }

    #[test]
    fn rapidfuzz_doc_examples() {
        // rapidfuzz docs: partial_ratio("this is a test", "this is a test!") == 100
        assert_eq!(partial_ratio("this is a test", "this is a test!", 0.0), 100.0);
        // values computed with rapidfuzz 3.14.6 (reference/venv)
        let r = partial_ratio("fuzzy wuzzy was a bear", "wuzzy fuzzy was a bear", 0.0);
        assert_eq!(r, 93.02325581395348);
        assert_eq!(partial_ratio("springfield miss", "1220 main street springf eld miss", 90.0), 93.75);
        assert_eq!(partial_ratio("", "", 90.0), 100.0);
        assert_eq!(partial_ratio("", "abc", 0.0), 0.0);
    }
}
