//! Similarity measures (PaperRust extension): Jaro-Winkler, Levenshtein,
//! token-set ratio, prefix/subset detection. All values in `[0, 1]`.
//! Comparison on Unicode code points.

use std::collections::BTreeSet;

/// Levenshtein distance (insert/delete/replace cost 1 each).
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// `1 - d / max(len)`; two empty strings → 1.
pub fn levenshtein_similarity(a: &str, b: &str) -> f64 {
    let la = a.chars().count();
    let lb = b.chars().count();
    let m = la.max(lb);
    if m == 0 {
        return 1.0;
    }
    1.0 - levenshtein(a, b) as f64 / m as f64
}

/// Jaro similarity.
pub fn jaro(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let window = (a.len().max(b.len()) / 2).saturating_sub(1);
    let mut a_m = vec![false; a.len()];
    let mut b_m = vec![false; b.len()];
    let mut matches = 0usize;
    for i in 0..a.len() {
        let lo = i.saturating_sub(window);
        let hi = (i + window + 1).min(b.len());
        for j in lo..hi {
            if !b_m[j] && a[i] == b[j] {
                a_m[i] = true;
                b_m[j] = true;
                matches += 1;
                break;
            }
        }
    }
    if matches == 0 {
        return 0.0;
    }
    let mut t = 0usize;
    let mut k = 0usize;
    for i in 0..a.len() {
        if a_m[i] {
            while !b_m[k] {
                k += 1;
            }
            if a[i] != b[k] {
                t += 1;
            }
            k += 1;
        }
    }
    let m = matches as f64;
    (m / a.len() as f64 + m / b.len() as f64 + (m - (t / 2) as f64) / m) / 3.0
}

/// Jaro-Winkler (p = 0.1, common prefix up to 4 characters).
pub fn jaro_winkler(a: &str, b: &str) -> f64 {
    let j = jaro(a, b);
    let prefix = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).take(4).count();
    j + prefix as f64 * 0.1 * (1.0 - j)
}

/// Token-set ratio (like fuzzywuzzy `token_set_ratio`, but with Levenshtein similarity):
/// intersection `s`, remainder `a\s`, `b\s`; maximum of the similarities of
/// `s` vs `s+a`, `s` vs `s+b`, `s+a` vs `s+b`.
pub fn token_set_ratio(a_tokens: &[String], b_tokens: &[String]) -> f64 {
    let sa: BTreeSet<&str> = a_tokens.iter().map(String::as_str).collect();
    let sb: BTreeSet<&str> = b_tokens.iter().map(String::as_str).collect();
    if sa.is_empty() && sb.is_empty() {
        return 1.0;
    }
    if sa.is_empty() || sb.is_empty() {
        return 0.0;
    }
    let inter: Vec<&str> = sa.intersection(&sb).copied().collect();
    let diff_a: Vec<&str> = sa.difference(&sb).copied().collect();
    let diff_b: Vec<&str> = sb.difference(&sa).copied().collect();
    let s = inter.join(" ");
    let join = |base: &str, rest: &[&str]| {
        if base.is_empty() { rest.join(" ") } else if rest.is_empty() { base.to_string() } else { format!("{base} {}", rest.join(" ")) }
    };
    let ta = join(&s, &diff_a);
    let tb = join(&s, &diff_b);
    let mut best = levenshtein_similarity(&ta, &tb);
    if !s.is_empty() {
        best = best.max(levenshtein_similarity(&s, &ta)).max(levenshtein_similarity(&s, &tb));
    }
    best
}

/// Share of the tokens of the shorter list that occur (exactly or by prefix ≥ 4 characters) in the
/// longer one. 1.0 = subset ("swisscom" ⊂ "swisscom mobile").
pub fn token_containment(a_tokens: &[String], b_tokens: &[String]) -> f64 {
    let (short, long) = if a_tokens.len() <= b_tokens.len() { (a_tokens, b_tokens) } else { (b_tokens, a_tokens) };
    if short.is_empty() {
        return 0.0;
    }
    let hit = short
        .iter()
        .filter(|t| {
            long.iter().any(|l| {
                l == *t
                    || (t.chars().count() >= 4 && l.starts_with(t.as_str()))
                    || (l.chars().count() >= 4 && t.starts_with(l.as_str()))
            })
        })
        .count();
    hit as f64 / short.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn known_values() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert!(close(jaro("martha", "marhta"), 0.944));
        assert!(close(jaro_winkler("martha", "marhta"), 0.961));
        assert!(close(jaro_winkler("dixon", "dicksonx"), 0.813));
        assert_eq!(jaro_winkler("", ""), 1.0);
        assert_eq!(jaro("a", ""), 0.0);
    }

    #[test]
    fn token_set() {
        let t = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert_eq!(token_set_ratio(&t("bank kantonal"), &t("kantonal bank")), 1.0);
        assert_eq!(token_set_ratio(&t("swisscom"), &t("swisscom mobile")), 1.0);
        assert!(token_set_ratio(&t("abc"), &t("xyz")) < 0.2);
        assert_eq!(token_containment(&t("swisscom"), &t("swisscom mobile")), 1.0);
        assert_eq!(token_containment(&t("versicherung"), &t("versicherungen axa")), 1.0);
    }
}
