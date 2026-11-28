//! DE·HTR v2 (German handwriting, line-level CTC) as a second pass for text lines PP-OCR read with
//! low confidence (docs/OCR-NEXT.md §2.6, stage 5).
//!
//! Model: `naeyn/de-htr-web-v2` `config.json` (format 2): input `image` `[1, 1, 64, W]`, gray,
//! height 64 with the aspect ratio kept, right-padded with white to a multiple of 4, pixels in
//! `[0, 1]` (normalisation is inside the model), at most 1024 px wide (longer lines are split);
//! output `logits` `[1, T, 112]` (raw, class 0 = blank, output stride 4). Reference runner:
//! `results/ocr-eval/2026-11-24/tools/laptop/run_dehtr.py` (`recognize`, `split_wide`).

use crate::image::Image;
use crate::orient::area_resize;
use crate::runtime::{Model, Tensor};
use crate::{Error, Result};

/// The 111 characters of the model (index + 1 = class), from `config.json`.
pub const ALPHABET: &str =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZäöüÄÖÜß0123456789 .,;:!?\"'()[]{}<>-–—_/\\@#%&*+=|€§°²³→←↑↓×÷";
pub const HEIGHT: usize = 64;
pub const MAX_WIDTH: usize = 1024;

/// Result of one line: text and mean probability of the kept characters (0..1).
#[derive(Debug, Clone, PartialEq)]
pub struct HtrLine {
    pub text: String,
    pub conf: f32,
}

pub struct Htr {
    model: Box<dyn Model>,
    alphabet: Vec<char>,
}

impl Htr {
    pub fn new(model: Box<dyn Model>) -> Self {
        Self { model, alphabet: ALPHABET.chars().collect() }
    }

    /// Reads one line crop (text running left to right).
    pub fn recognize(&self, crop: &Image) -> Result<HtrLine> {
        let g = prepare(crop);
        if g.width == 0 {
            return Ok(HtrLine { text: String::new(), conf: 0.0 });
        }
        let mut parts = Vec::new();
        let mut probs = Vec::new();
        for part in split_wide(&g, MAX_WIDTH) {
            let data: Vec<f32> = part.data.iter().map(|&v| f32::from(v) / 255.0).collect();
            let out = self.model.run(Tensor::new(vec![1, 1, HEIGHT, part.width], data)?)?;
            let (t, c) = match out.shape.as_slice() {
                [1, t, c] => (*t, *c),
                s => return Err(Error::Model(format!("unexpected DE·HTR output shape {s:?}"))),
            };
            if c != self.alphabet.len() + 1 {
                return Err(Error::Model(format!("DE·HTR output has {c} classes, alphabet {}", self.alphabet.len() + 1)));
            }
            let (s, p) = decode(&out.data, t, c, &self.alphabet);
            if !s.trim().is_empty() {
                parts.push(s.trim().to_string());
            }
            probs.extend(p);
        }
        let conf = if probs.is_empty() { 0.0 } else { probs.iter().sum::<f32>() / probs.len() as f32 };
        Ok(HtrLine { text: parts.join(" "), conf })
    }
}

/// Gray, height 64 (box filter when shrinking, bilinear when enlarging), white right padding to a
/// multiple of 4.
pub fn prepare(crop: &Image) -> Image {
    if crop.width == 0 || crop.height == 0 {
        return Image { width: 0, height: HEIGHT, channels: 1, data: Vec::new() };
    }
    let gray = to_gray(crop);
    let scale = HEIGHT as f32 / gray.height as f32;
    let w = ((gray.width as f32 * scale).round() as usize).max(4);
    let r = if scale < 1.0 { area_resize(&gray, w, HEIGHT) } else { gray.resize(w, HEIGHT) };
    let pad = (4 - w % 4) % 4;
    if pad == 0 {
        return r;
    }
    let nw = w + pad;
    let mut data = vec![255u8; nw * HEIGHT];
    for y in 0..HEIGHT {
        data[y * nw..y * nw + w].copy_from_slice(&r.data[y * w..(y + 1) * w]);
    }
    Image { width: nw, height: HEIGHT, channels: 1, data }
}

/// ITU-R 601 luma (like OpenCV `COLOR_BGR2GRAY` on RGB data).
fn to_gray(img: &Image) -> Image {
    if img.channels == 1 {
        return img.clone();
    }
    let data = img
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| (0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2])).round().clamp(0.0, 255.0) as u8)
        .collect();
    Image { width: img.width, height: img.height, channels: 1, data }
}

/// Python `split_wide`: split a line wider than `max_w` at the lightest (smoothed) column between
/// 30 % and 70 % of its width, cut aligned to 4 px; recursively.
pub fn split_wide(g: &Image, max_w: usize) -> Vec<Image> {
    if g.width <= max_w || g.width < 8 {
        return vec![g.clone()];
    }
    let (w, h) = (g.width, g.height);
    let (lo, hi) = ((w as f32 * 0.3) as usize, (w as f32 * 0.7) as usize);
    let ink: Vec<f32> = (lo..hi).map(|x| (0..h).map(|y| 255.0 - f32::from(g.data[y * w + x])).sum()).collect();
    // 9-tap moving average, `np.convolve(mode="same")`
    let sm: Vec<f32> = (0..ink.len())
        .map(|i| {
            let a = i.saturating_sub(4);
            let b = (i + 5).min(ink.len());
            ink[a..b].iter().sum::<f32>() / 9.0
        })
        .collect();
    let best = sm.iter().enumerate().fold((0usize, f32::MAX), |acc, (i, &v)| if v < acc.1 { (i, v) } else { acc }).0;
    let cut = ((lo + best) / 4 * 4).clamp(4, w - 4);
    let crop = |x0: usize, x1: usize| -> Image {
        let nw = x1 - x0;
        let mut data = Vec::with_capacity(nw * h);
        for y in 0..h {
            data.extend_from_slice(&g.data[y * w + x0..y * w + x1]);
        }
        Image { width: nw, height: h, channels: 1, data }
    };
    let mut out = split_wide(&crop(0, cut), max_w);
    out.extend(split_wide(&crop(cut, w), max_w));
    out
}

/// Greedy CTC decode of raw logits (`t`×`c`); returns the text and the softmax probability of every
/// kept character.
pub fn decode(logits: &[f32], t: usize, c: usize, alphabet: &[char]) -> (String, Vec<f32>) {
    let mut s = String::new();
    let mut probs = Vec::new();
    let mut prev = 0usize;
    for step in 0..t {
        let row = &logits[step * c..(step + 1) * c];
        let (best, m) = row.iter().copied().enumerate().fold((0usize, f32::MIN), |a, (i, v)| if v > a.1 { (i, v) } else { a });
        if best != 0
            && best != prev
            && let Some(&ch) = alphabet.get(best - 1)
        {
            let z: f32 = row.iter().map(|v| (v - m).exp()).sum();
            s.push(ch);
            probs.push(1.0 / z);
        }
        prev = best;
    }
    (s, probs)
}

/// Word spans as fractions of the line width, distributed by character share: every character
/// (including the spaces between words) gets the same width. Used for the text-layer boxes of a
/// line whose text came from DE·HTR (its CTC frames are not mapped back, see docs/OCR-PPOCR.md).
pub fn word_fractions(text: &str) -> Vec<(String, f32, f32)> {
    let n = text.chars().count();
    if n == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut word = String::new();
    for (i, ch) in text.chars().chain(std::iter::once(' ')).enumerate() {
        if ch.is_whitespace() {
            if let Some(s) = start.take() {
                out.push((std::mem::take(&mut word), s as f32 / n as f32, i.min(n) as f32 / n as f32));
            }
        } else {
            if start.is_none() {
                start = Some(i);
            }
            word.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alphabet_matches_model_classes() {
        // model output has 112 classes = blank + 111 characters
        assert_eq!(ALPHABET.chars().count(), 111);
        assert_eq!(ALPHABET.chars().nth(52), Some('ä'));
    }

    #[test]
    fn decode_collapses_and_scores() {
        let alph: Vec<char> = "ab".chars().collect();
        // classes: blank, a, b; steps: a a blank b
        let l = [0.0, 5.0, 0.0, 0.0, 5.0, 0.0, 5.0, 0.0, 0.0, 0.0, 0.0, 5.0];
        let (s, p) = decode(&l, 4, 3, &alph);
        assert_eq!(s, "ab");
        assert_eq!(p.len(), 2);
        assert!(p.iter().all(|&x| x > 0.98 && x <= 1.0));
    }

    #[test]
    fn prepare_scales_to_64_and_pads_white() {
        let img = Image::filled(101, 32, 3, 0);
        let g = prepare(&img);
        assert_eq!((g.height, g.channels), (64, 1));
        assert_eq!(g.width % 4, 0);
        assert_eq!(g.width, 204); // 202 → padded to 204
        assert_eq!(g.data[g.width - 1], 255);
        assert_eq!(g.data[0], 0);
        assert_eq!(prepare(&Image::filled(0, 0, 1, 0)).width, 0);
    }

    #[test]
    fn wide_lines_split_at_light_columns() {
        let (w, h) = (2400, 64);
        let mut data = vec![0u8; w * h];
        // white gap around x = 1200
        for y in 0..h {
            for x in 1190..1215 {
                data[y * w + x] = 255;
            }
        }
        let g = Image::new(w, h, 1, data).unwrap();
        let parts = split_wide(&g, 1024);
        assert!(parts.iter().all(|p| p.width <= 1024), "{:?}", parts.iter().map(|p| p.width).collect::<Vec<_>>());
        assert_eq!(parts.iter().map(|p| p.width).sum::<usize>(), w);
    }

    #[test]
    fn fractions_by_character_share() {
        let f = word_fractions("ab cdef");
        assert_eq!(f.len(), 2);
        assert_eq!(f[0], ("ab".to_string(), 0.0, 2.0 / 7.0));
        assert_eq!(f[1], ("cdef".to_string(), 3.0 / 7.0, 1.0));
        assert!(word_fractions("").is_empty());
        assert_eq!(word_fractions("  x ").len(), 1);
    }
}
