//! Pure search logic (without PdfCraft, testable): map the matches of a term in a text line onto the
//! word rectangles of that line.

use crate::job::Rect;

/// Lowercase per character (same length as the input, so that indices match).
fn fold(c: char) -> char {
    let mut l = c.to_lowercase();
    match (l.next(), l.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

/// Character ranges (start, end exclusive) of all occurrences of `term` in `text`, case-insensitive,
/// non-overlapping.
pub fn find_ranges(text: &str, term: &str) -> Vec<(usize, usize)> {
    let hay: Vec<char> = text.chars().map(fold).collect();
    let needle: Vec<char> = term.trim().chars().map(fold).collect();
    let mut out = Vec::new();
    if needle.is_empty() || needle.len() > hay.len() {
        return out;
    }
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()] == needle[..] {
            out.push((i, i + needle.len()));
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

/// Word boundaries (character ranges of non-empty runs without whitespace).
pub fn word_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut n = 0;
    for (i, c) in text.chars().enumerate() {
        n = i + 1;
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                out.push((s, i));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        out.push((s, n));
    }
    out
}

/// Rectangles for a match `range` in a line with `text`, line rectangle `line` and
/// word rectangles `ink`. If words and rectangles fit together, the words that
/// touch the match are taken (better to redact slightly more than too little); otherwise the whole line.
pub fn rects_for(text: &str, line: Rect, ink: &[Rect], range: (usize, usize)) -> Vec<Rect> {
    let words = word_ranges(text);
    if words.len() != ink.len() || ink.is_empty() {
        return vec![line];
    }
    let picked: Vec<Rect> = words.iter().zip(ink).filter(|((s, e), _)| *s < range.1 && range.0 < *e).map(|(_, r)| *r).collect();
    if picked.is_empty() { vec![line] } else { picked }
}

/// Enlarge the rectangle by `pad` points (normalised).
pub fn padded(r: Rect, pad: f64) -> Rect {
    let (x0, x1) = if r[0] <= r[2] { (r[0], r[2]) } else { (r[2], r[0]) };
    let (y0, y1) = if r[1] <= r[3] { (r[1], r[3]) } else { (r[3], r[1]) };
    [x0 - pad, y0 - pad, x1 + pad, y1 + pad]
}

/// Is the rectangle usable (finite numbers, area > 0, not absurdly large)?
pub fn valid_rect(r: &Rect) -> bool {
    r.iter().all(|v| v.is_finite() && v.abs() < 1.0e6) && (r[2] - r[0]).abs() > 0.01 && (r[3] - r[1]).abs() > 0.01
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_case_insensitive_non_overlapping() {
        assert_eq!(find_ranges("Geheim und GEHEIM", "geheim"), vec![(0, 6), (11, 17)]);
        assert_eq!(find_ranges("aaaa", "aa"), vec![(0, 2), (2, 4)]);
        assert_eq!(find_ranges("abc", " "), vec![]);
        assert_eq!(find_ranges("Straße", "STRASSE"), vec![]);
        assert_eq!(find_ranges("Öl", "öl"), vec![(0, 2)]);
    }

    #[test]
    fn words_and_rects() {
        assert_eq!(word_ranges("  ab  c d "), vec![(2, 4), (6, 7), (8, 9)]);
        let ink = [[0.0, 0.0, 10.0, 5.0], [12.0, 0.0, 20.0, 5.0], [22.0, 0.0, 30.0, 5.0]];
        let line = [0.0, 0.0, 30.0, 5.0];
        // "IBAN DE12 3456": match "de12 34" touches word 2 and 3
        let t = "IBAN DE12 3456";
        let r = find_ranges(t, "de12 34");
        assert_eq!(rects_for(t, line, &ink, r[0]), vec![ink[1], ink[2]]);
        // word count does not match → whole line
        assert_eq!(rects_for(t, line, &ink[..2], r[0]), vec![line]);
    }

    #[test]
    fn rect_helpers() {
        assert_eq!(padded([10.0, 5.0, 0.0, 0.0], 1.0), [-1.0, -1.0, 11.0, 6.0]);
        assert!(valid_rect(&[0.0, 0.0, 1.0, 1.0]));
        assert!(!valid_rect(&[0.0, 0.0, 0.0, 1.0]));
        assert!(!valid_rect(&[0.0, 0.0, f64::NAN, 1.0]));
    }
}
