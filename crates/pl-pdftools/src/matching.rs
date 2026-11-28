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

// ------------------------------------------------------------------ Page-wide search (across lines)

/// One text line of a page as the PDF library reports it (text, line box, word boxes).
#[derive(Debug, Clone, Copy)]
pub struct Line<'a> {
    pub text: &'a str,
    pub rect: Rect,
    pub ink: &'a [Rect],
}

/// A match on one page: index into the term list, context text, rectangles of all touched words
/// (on every line the match spans).
#[derive(Debug, Clone, PartialEq)]
pub struct PageMatch {
    pub term: usize,
    pub context: String,
    pub rects: Vec<Rect>,
    /// (line index, char start, char end) per spanned line – identifies the match.
    pub segments: Vec<(usize, usize, usize)>,
}

/// How a line that ends in a hyphen is joined to the next one.
#[derive(Clone, Copy, PartialEq)]
enum HyphenJoin {
    /// "Muster- mann" (like any other line break)
    Space,
    /// "Muster-mann" (real hyphenated compound split at the hyphen, e.g. "Meier-Schmidt")
    Keep,
    /// "Mustermann" (soft hyphenation)
    Drop,
}

/// Normalised search term: case-folded, whitespace runs collapsed to one space, trimmed.
pub fn normalize_term(term: &str) -> Vec<char> {
    let mut out: Vec<char> = Vec::new();
    for c in term.trim().chars() {
        if c.is_whitespace() {
            if out.last().is_some_and(|l| *l != ' ') {
                out.push(' ');
            }
        } else {
            out.push(fold(c));
        }
    }
    out
}

fn ends_with_word_hyphen(text: &str) -> bool {
    let mut it = text.trim_end().chars().rev();
    matches!((it.next(), it.next()), (Some('-' | '\u{00AD}' | '\u{2010}'), Some(p)) if p.is_alphanumeric())
}

/// The page text as one stream (case-folded, whitespace collapsed, lines joined by one space or
/// per `hyphen` at a line-end hyphen) and, per stream char, its origin (line, char index) – `None`
/// for separators.
fn page_stream(lines: &[Line<'_>], hyphen: HyphenJoin) -> (Vec<char>, Vec<Option<(usize, usize)>>) {
    let mut chars: Vec<char> = Vec::new();
    let mut origin: Vec<Option<(usize, usize)>> = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        for (ci, c) in line.text.chars().enumerate() {
            if c.is_whitespace() {
                if chars.last().is_some_and(|l| *l != ' ') {
                    chars.push(' ');
                    origin.push(None);
                }
            } else {
                chars.push(fold(c));
                origin.push(Some((li, ci)));
            }
        }
        // drop trailing whitespace of the line before deciding how to join
        while chars.last() == Some(&' ') {
            chars.pop();
            origin.pop();
        }
        if li + 1 == lines.len() || chars.is_empty() {
            continue;
        }
        if ends_with_word_hyphen(line.text) {
            match hyphen {
                HyphenJoin::Space => {
                    chars.push(' ');
                    origin.push(None);
                }
                HyphenJoin::Keep => {}
                HyphenJoin::Drop => {
                    chars.pop();
                    origin.pop();
                }
            }
        } else {
            chars.push(' ');
            origin.push(None);
        }
    }
    (chars, origin)
}

/// All matches of `terms` on a page, also across line breaks (a term split over two or more
/// lines, whitespace differences, line-end hyphenation with and without the hyphen). Matches are
/// case-insensitive; per join variant non-overlapping; identical matches of several variants are
/// reported once. Rectangles: the touched words of each spanned line (see [`rects_for`]), padded
/// by 0.5 pt.
pub fn page_matches(lines: &[Line<'_>], terms: &[&str]) -> Vec<PageMatch> {
    let needles: Vec<Vec<char>> = terms.iter().map(|t| normalize_term(t)).collect();
    let mut out: Vec<PageMatch> = Vec::new();
    let any_hyphen = lines.iter().take(lines.len().saturating_sub(1)).any(|l| ends_with_word_hyphen(l.text));
    let variants: &[HyphenJoin] = if any_hyphen { &[HyphenJoin::Space, HyphenJoin::Keep, HyphenJoin::Drop] } else { &[HyphenJoin::Space] };
    for &variant in variants {
        let (hay, origin) = page_stream(lines, variant);
        for (ti, needle) in needles.iter().enumerate() {
            if needle.is_empty() || needle.len() > hay.len() {
                continue;
            }
            let mut i = 0;
            while i + needle.len() <= hay.len() {
                if hay[i..i + needle.len()] != needle[..] {
                    i += 1;
                    continue;
                }
                // per line: char range covered by the match
                let mut segs: Vec<(usize, usize, usize)> = Vec::new();
                for (li, ci) in origin[i..i + needle.len()].iter().flatten() {
                    match segs.last_mut() {
                        Some((l, _, e)) if *l == *li => *e = ci + 1,
                        _ => segs.push((*li, *ci, ci + 1)),
                    }
                }
                i += needle.len();
                if segs.is_empty() || out.iter().any(|m| m.term == ti && m.segments == segs) {
                    continue;
                }
                let mut rects = Vec::new();
                let mut context = String::new();
                for (li, s, e) in &segs {
                    let l = &lines[*li];
                    rects.extend(rects_for(l.text, l.rect, l.ink, (*s, *e)).into_iter().map(|r| padded(r, 0.5)));
                    if !context.is_empty() {
                        context.push(' ');
                    }
                    context.push_str(l.text.trim());
                }
                out.push(PageMatch { term: ti, context: context.chars().take(200).collect(), rects, segments: segs });
            }
        }
    }
    out
}

// ------------------------------------------------------------------ Page geometry (archive → original)

/// Geometry of a page: matrix from display space (points, origin bottom-left of the page as shown,
/// after `/Rotate` and `/UserUnit`) to user space, and the displayed size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageGeom {
    pub view: [f64; 6],
    pub size: (f64, f64),
}

fn apply(m: &[f64; 6], x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn invert(m: &[f64; 6]) -> Option<[f64; 6]> {
    let det = m[0] * m[3] - m[1] * m[2];
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    Some([a, b, c, d, -(a * m[4] + c * m[5]), -(b * m[4] + d * m[5])])
}

/// Map a rectangle in the user space of page `from` onto the same visible spot of page `to`
/// (pages of an OCR archive and its original show the same picture, possibly at another size or
/// with other boxes/rotation). `None` if the pages do not show the same aspect ratio (more than
/// 2 % apart) or a geometry is degenerate – then the archive cannot be trusted for positions.
pub fn map_rect(r: Rect, from: &PageGeom, to: &PageGeom) -> Option<Rect> {
    let (fw, fh) = from.size;
    let (tw, th) = to.size;
    if !(fw > 0.0 && fh > 0.0 && tw > 0.0 && th > 0.0) {
        return None;
    }
    if ((fw / fh) / (tw / th) - 1.0).abs() > 0.02 {
        return None;
    }
    let inv = invert(&from.view)?;
    let (sx, sy) = (tw / fw, th / fh);
    let mut xs = [0.0f64; 4];
    let mut ys = [0.0f64; 4];
    for (k, (x, y)) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])].into_iter().enumerate() {
        let (dx, dy) = apply(&inv, x, y);
        let (ux, uy) = apply(&to.view, dx * sx, dy * sy);
        xs[k] = ux;
        ys[k] = uy;
    }
    let min = |v: &[f64; 4]| v.iter().copied().fold(f64::INFINITY, f64::min);
    let max = |v: &[f64; 4]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let out = [min(&xs), min(&ys), max(&xs), max(&ys)];
    out.iter().all(|v| v.is_finite()).then_some(out)
}

/// Do two rectangles overlap (positive area)?
pub fn overlaps(a: &Rect, b: &Rect) -> bool {
    let a = padded(*a, 0.0);
    let b = padded(*b, 0.0);
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// Merge hits of the original text layer with hits of the OCR archive (rects already mapped onto
/// the original): an archive hit for the same term on the same page that overlaps an original hit
/// is the same occurrence and is dropped; all others are kept and marked `from_ocr`.
pub fn merge_hits(original: Vec<crate::job::Hit>, ocr: Vec<crate::job::Hit>) -> Vec<crate::job::Hit> {
    let mut out = original;
    let n = out.len();
    for mut h in ocr {
        let dup =
            out[..n].iter().any(|o| o.page == h.page && o.term == h.term && o.rects.iter().any(|a| h.rects.iter().any(|b| overlaps(a, b))));
        if !dup {
            h.from_ocr = true;
            out.push(h);
        }
    }
    out.sort_by_key(|h| h.page);
    out
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

    fn words_line(text: &str, y: f64) -> (String, Rect, Vec<Rect>) {
        // 10 pt per char, words as boxes
        let ink = word_ranges(text).into_iter().map(|(s, e)| [s as f64 * 10.0, y, e as f64 * 10.0, y + 10.0]).collect();
        (text.to_string(), [0.0, y, text.chars().count() as f64 * 10.0, y + 10.0], ink)
    }

    fn lines(v: &[(String, Rect, Vec<Rect>)]) -> Vec<Line<'_>> {
        v.iter().map(|(t, r, i)| Line { text: t, rect: *r, ink: i }).collect()
    }

    /// Review finding (2026-11-27): a term split over two lines was neither found nor caught by the
    /// verification (which used the same per-line search).
    #[test]
    fn term_across_line_break_is_found_on_both_lines() {
        let raw = [words_line("Vertrag mit Max", 100.0), words_line("Mustermann GmbH", 80.0)];
        let l = lines(&raw);
        let m = page_matches(&l, &["max  mustermann"]);
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(m[0].segments, vec![(0, 12, 15), (1, 0, 10)]);
        // word "Max" on line 1, word "Mustermann" on line 2 (padded)
        assert_eq!(m[0].rects, vec![padded(raw[0].2[2], 0.5), padded(raw[1].2[0], 0.5)]);
        assert!(m[0].context.contains("Vertrag mit Max Mustermann GmbH"));
        // "GmbH" is not part of it, a term within one line still works
        assert_eq!(page_matches(&l, &["gmbh"]).len(), 1);
        assert!(page_matches(&l, &["mit max mustermann gmbh x"]).is_empty());
    }

    #[test]
    fn hyphenated_line_break_with_and_without_hyphen() {
        let raw = [words_line("Herr Muster-", 100.0), words_line("mann und Frau Meier-", 80.0), words_line("Schmidt", 60.0)];
        let l = lines(&raw);
        let m = page_matches(&l, &["Mustermann", "Meier-Schmidt", "muster- mann"]);
        let found: Vec<usize> = m.iter().map(|x| x.term).collect();
        assert!(found.contains(&0) && found.contains(&1) && found.contains(&2), "{m:?}");
        let mm = m.iter().find(|x| x.term == 0).unwrap();
        assert_eq!(mm.segments, vec![(0, 5, 11), (1, 0, 4)]);
        // the same occurrence found by several join variants is reported once
        assert_eq!(m.iter().filter(|x| x.term == 0).count(), 1);
    }

    #[test]
    fn whitespace_inside_a_line_is_collapsed() {
        let raw = [words_line("IBAN   DE12  3456", 100.0)];
        let m = page_matches(&lines(&raw), &["iban de12 3456"]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].rects.len(), 3);
    }

    #[test]
    fn geometry_maps_archive_onto_original() {
        let orig = PageGeom { view: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], size: (600.0, 800.0) };
        // same page at half size
        let half = PageGeom { view: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], size: (300.0, 400.0) };
        assert_eq!(map_rect([10.0, 20.0, 30.0, 40.0], &half, &orig), Some([20.0, 40.0, 60.0, 80.0]));
        assert_eq!(map_rect([20.0, 40.0, 60.0, 80.0], &orig, &half), Some([10.0, 20.0, 30.0, 40.0]));
        // identical geometry: unchanged
        assert_eq!(map_rect([1.0, 2.0, 3.0, 4.0], &orig, &orig), Some([1.0, 2.0, 3.0, 4.0]));
        // other aspect ratio: refused
        let wide = PageGeom { view: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], size: (800.0, 600.0) };
        assert_eq!(map_rect([1.0, 2.0, 3.0, 4.0], &wide, &orig), None);
        // original with a crop box offset (display origin at 100,100)
        let cropped = PageGeom { view: [1.0, 0.0, 0.0, 1.0, 100.0, 100.0], size: (600.0, 800.0) };
        assert_eq!(map_rect([10.0, 20.0, 30.0, 40.0], &orig, &cropped), Some([110.0, 120.0, 130.0, 140.0]));
    }

    #[test]
    fn merge_drops_archive_duplicates_keeps_ocr_only_hits() {
        use crate::job::Hit;
        let h = |page, r: Rect| Hit { page, term: "x".into(), context: String::new(), rects: vec![r], from_ocr: false };
        let merged = merge_hits(vec![h(0, [0.0, 0.0, 10.0, 10.0])], vec![h(0, [1.0, 1.0, 9.0, 9.0]), h(1, [0.0, 0.0, 10.0, 10.0])]);
        assert_eq!(merged.len(), 2, "{merged:?}");
        assert!(!merged[0].from_ocr && merged[1].from_ocr && merged[1].page == 1);
    }

    #[test]
    fn rect_helpers() {
        assert_eq!(padded([10.0, 5.0, 0.0, 0.0], 1.0), [-1.0, -1.0, 11.0, 6.0]);
        assert!(valid_rect(&[0.0, 0.0, 1.0, 1.0]));
        assert!(!valid_rect(&[0.0, 0.0, 0.0, 1.0]));
        assert!(!valid_rect(&[0.0, 0.0, f64::NAN, 1.0]));
    }
}
