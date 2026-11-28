//! CTC decoding of the recognizer output and word boxes from the CTC time steps.
//!
//! Model: RapidOCR `ch_ppocr_rec/utils.py` (`CTCLabelDecode.decode`, `get_word_info`) and
//! `cal_rec_boxes/main.py` (`cal_ocr_word_box`, `calc_en_num_box`, `adjust_box_overlap`).
//! Difference: the confidence of a word is the mean of its characters' probabilities (RapidOCR
//! pairs word *i* with character *i*'s probability, which is an indexing slip).

/// Decoded text line.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub text: String,
    /// Mean probability of the kept characters (0 if none), like RapidOCR's line score.
    pub score: f32,
    /// One entry per non-space character group (word).
    pub words: Vec<WordCols>,
    /// Width of the text in time steps: `T * wh_ratio / max_wh_ratio` (RapidOCR `line_txt_len`).
    pub line_len: f32,
}

/// A word: its text, the time-step column of each character, and per-character probabilities.
#[derive(Debug, Clone, PartialEq)]
pub struct WordCols {
    pub text: String,
    pub cols: Vec<usize>,
    pub probs: Vec<f32>,
    pub cjk: bool,
}

impl WordCols {
    pub fn conf(&self) -> f32 {
        if self.probs.is_empty() { 0.0 } else { self.probs.iter().sum::<f32>() / self.probs.len() as f32 }
    }
}

fn is_cjk(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}

/// Greedy CTC decode of one sequence. `probs`: `t`×`classes` softmax output (row-major);
/// `charset[i]` is the label of class `i` (class 0 = blank). `wh_ratio`/`max_wh_ratio`: aspect
/// ratio of this crop and of the batch (for the word-box geometry).
pub fn decode(probs: &[f32], t: usize, classes: usize, charset: &[String], wh_ratio: f32, max_wh_ratio: f32) -> Decoded {
    let mut sel: Vec<(usize, usize, f32)> = Vec::new(); // (time step, class, prob)
    let mut prev: Option<usize> = None;
    for step in 0..t {
        let row = &probs[step * classes..(step + 1) * classes];
        let (mut best, mut bp) = (0usize, f32::MIN);
        for (k, &p) in row.iter().enumerate() {
            if p > bp {
                bp = p;
                best = k;
            }
        }
        let keep = best != 0 && prev != Some(best);
        prev = Some(best);
        if keep {
            sel.push((step, best, bp));
        }
    }
    let chars: Vec<(usize, String, f32)> = sel.iter().map(|&(s, k, p)| (s, charset.get(k).cloned().unwrap_or_default(), p)).collect();
    let text: String = chars.iter().map(|c| c.1.as_str()).collect();
    let score = if chars.is_empty() { 0.0 } else { chars.iter().map(|c| c.2).sum::<f32>() / chars.len() as f32 };
    let line_len = if max_wh_ratio > 0.0 { t as f32 * wh_ratio / max_wh_ratio } else { t as f32 };
    Decoded { text, score, words: word_info(&chars), line_len }
}

/// RapidOCR `get_word_info`: split at whitespace, at a change between CJK and other characters,
/// and where two consecutive characters are more than 5 time steps apart.
fn word_info(chars: &[(usize, String, f32)]) -> Vec<WordCols> {
    let mut out = Vec::new();
    if chars.is_empty() {
        return out;
    }
    let first_cjk = chars[0].1.chars().next().is_some_and(is_cjk);
    let mut cur = WordCols { text: String::new(), cols: Vec::new(), probs: Vec::new(), cjk: false };
    let mut state: Option<bool> = None;
    for (i, (col, s, p)) in chars.iter().enumerate() {
        let gap = if i == 0 { (if first_cjk { 3 } else { 2 }).min(*col) } else { col - chars[i - 1].0 };
        if s.chars().all(char::is_whitespace) {
            if !cur.text.is_empty() {
                out.push(std::mem::replace(&mut cur, WordCols { text: String::new(), cols: Vec::new(), probs: Vec::new(), cjk: false }));
            }
            continue;
        }
        let c_cjk = s.chars().next().is_some_and(is_cjk);
        if state.is_none() {
            state = Some(c_cjk);
        }
        if state != Some(c_cjk) || gap > 5 {
            if !cur.text.is_empty() {
                out.push(std::mem::replace(&mut cur, WordCols { text: String::new(), cols: Vec::new(), probs: Vec::new(), cjk: false }));
            }
            state = Some(c_cjk);
        }
        cur.text.push_str(s);
        cur.cols.push(*col);
        cur.probs.push(*p);
        cur.cjk = c_cjk;
    }
    if !cur.text.is_empty() {
        out.push(cur);
    }
    out
}

/// Horizontal word spans `(x0, x1)` in crop pixels (crop width `crop_w`), one per word of `d`
/// (RapidOCR `cal_ocr_word_box` for Latin text: one box per word; CJK words get the span of their
/// characters as well). Overlaps between neighbours are split in the middle.
pub fn word_spans(d: &Decoded, crop_w: f32) -> Vec<(f32, f32)> {
    if d.text.is_empty() || d.line_len <= 0.0 || d.words.is_empty() {
        return Vec::new();
    }
    let col_w = crop_w / d.line_len;
    let widths: Vec<f32> = d
        .words
        .iter()
        .filter(|w| w.cols.len() > 1)
        .map(|w| (w.cols[w.cols.len() - 1] - w.cols[0]) as f32 * col_w / (w.cols.len() - 1) as f32)
        .collect();
    let n_chars = d.text.chars().count().max(1) as f32;
    let char_w = if widths.is_empty() { crop_w / n_chars } else { widths.iter().sum::<f32>() / widths.len() as f32 };
    let mut spans: Vec<(f32, f32)> = d
        .words
        .iter()
        .map(|w| {
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for &c in &w.cols {
                let centre = (c as f32 + 0.5) * col_w;
                let x0 = ((centre - char_w / 2.0).trunc()).max(0.0);
                let x1 = ((centre + char_w / 2.0).trunc()).min(crop_w);
                lo = lo.min(x0);
                hi = hi.max(x1);
            }
            (lo, hi.max(lo))
        })
        .collect();
    for i in 0..spans.len().saturating_sub(1) {
        if spans[i].1 > spans[i + 1].0 {
            let d = spans[i].1 - spans[i + 1].0;
            spans[i].1 -= d / 2.0;
            spans[i + 1].0 += d - d / 2.0;
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn charset() -> Vec<String> {
        // blank, a, b, c, space
        ["blank", "a", "b", "c", " "].iter().map(|s| s.to_string()).collect()
    }

    /// One-hot probability rows for a class sequence.
    fn rows(seq: &[usize], p: f32) -> Vec<f32> {
        let mut v = vec![0.0; seq.len() * 5];
        for (t, &k) in seq.iter().enumerate() {
            for j in 0..5 {
                v[t * 5 + j] = if j == k { p } else { (1.0 - p) / 4.0 };
            }
        }
        v
    }

    #[test]
    fn collapses_repeats_and_blanks() {
        let seq = [0, 1, 1, 0, 1, 2, 2, 0, 4, 0, 3, 0];
        let d = decode(&rows(&seq, 0.9), seq.len(), 5, &charset(), 1.0, 1.0);
        assert_eq!(d.text, "aab c");
        assert!((d.score - 0.9).abs() < 1e-5);
        assert_eq!(d.words.len(), 2);
        assert_eq!(d.words[0].text, "aab");
        assert_eq!(d.words[0].cols, vec![1, 4, 5]);
        assert_eq!(d.words[1].text, "c");
        assert_eq!(d.line_len, 12.0);
    }

    #[test]
    fn empty_sequence() {
        let d = decode(&rows(&[0, 0, 0], 0.9), 3, 5, &charset(), 1.0, 2.0);
        assert_eq!(d.text, "");
        assert_eq!(d.score, 0.0);
        assert!(d.words.is_empty());
        assert_eq!(d.line_len, 1.5);
        assert!(word_spans(&d, 100.0).is_empty());
    }

    #[test]
    fn large_gap_splits_word() {
        let seq = [1, 0, 0, 0, 0, 0, 0, 2];
        let d = decode(&rows(&seq, 0.8), seq.len(), 5, &charset(), 1.0, 1.0);
        assert_eq!(d.text, "ab");
        assert_eq!(d.words.len(), 2);
    }

    #[test]
    fn spans_follow_columns_and_do_not_overlap() {
        // "ab c" over 20 steps, crop 200 px → 10 px per step
        let mut seq = vec![0; 20];
        seq[1] = 1;
        seq[3] = 2;
        seq[6] = 4;
        seq[10] = 3;
        let d = decode(&rows(&seq, 0.9), 20, 5, &charset(), 1.0, 1.0);
        let s = word_spans(&d, 200.0);
        assert_eq!(s.len(), 2);
        // char width from "ab": (3-1)*10/1 = 20 px; a centre 15 → 5..25; b centre 35 → 25..45
        assert_eq!(s[0], (5.0, 45.0));
        assert_eq!(s[1], (95.0, 115.0));
        assert!(s.windows(2).all(|p| p[0].1 <= p[1].0));
        assert!((d.words[0].conf() - 0.9).abs() < 1e-6);
    }
}
