//! PP-OCR text detection + recognition on one page image.
//!
//! Model: RapidOCR 3.x `main.py` (`RapidOCR.__call__`: `preprocess_img` with `max_side_len`,
//! `apply_vertical_padding`, `detect_and_crop`, `recognize_txt`, `calc_word_boxes`,
//! `filter_by_text_score`), `ch_ppocr_det/main.py`, `ch_ppocr_rec/main.py`, as configured by the
//! 2026-11-24 measurement (`tools/laptop/run_rapidocr.py`: max side 2560, textline classifier off,
//! word boxes on). Reading order: [`crate::order`].

use crate::ctc::{self, Decoded};
use crate::db::{self, DbParams, DetBox};
use crate::geom::{self, Quad};
use crate::image::{Homography, Image};
use crate::order;
use crate::runtime::{Model, Tensor};
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct OcrParams {
    /// Longest image side before detection (RapidOCR `Global.max_side_len`; measured: 2560 reads
    /// as well as 3600; default [`DEFAULT_MAX_SIDE`] = 1920 since stage 3).
    pub max_side: usize,
    pub min_side: usize,
    /// Detector input: shortest side at least this (RapidOCR `Det.limit_side_len`, `limit_type: min`).
    pub det_limit_side: usize,
    /// Lines with a mean character probability below this are dropped (`Global.text_score`).
    pub text_score: f32,
    pub rec_batch: usize,
    pub rec_height: usize,
    pub db: DbParams,
}

impl Default for OcrParams {
    fn default() -> Self {
        Self {
            max_side: DEFAULT_MAX_SIDE,
            min_side: 30,
            det_limit_side: 736,
            text_score: 0.5,
            rec_batch: 6,
            rec_height: 48,
            db: DbParams::default(),
        }
    }
}

/// A recognized word; box in input image pixels, `conf` = mean character probability (0..1).
#[derive(Debug, Clone, PartialEq)]
pub struct OcrWord {
    pub text: String,
    pub bbox: [f32; 4],
    pub conf: f32,
}

/// A recognized text line (one detector box).
#[derive(Debug, Clone, PartialEq)]
pub struct TextLine {
    /// Quadrilateral in input image pixels (tl, tr, br, bl).
    pub quad: Quad,
    pub text: String,
    /// Mean character probability (0..1).
    pub score: f32,
    pub words: Vec<OcrWord>,
}

impl TextLine {
    pub fn bbox(&self) -> [f32; 4] {
        let (a, b, c, d) = geom::bounds(&self.quad);
        [a, b, c, d]
    }
}

/// Page result: lines in reading order with their `(block, row, segment)` position.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Page {
    pub width: usize,
    pub height: usize,
    /// `(block, row, segment)`; consecutive entries with equal `(block, row)` share a text row.
    pub lines: Vec<((u32, u32, u32), TextLine)>,
}

impl Page {
    /// Orders detector lines with [`order::reading_order`].
    pub fn from_lines(width: usize, height: usize, lines: Vec<TextLine>) -> Page {
        let boxes: Vec<order::BBox> = lines.iter().map(TextLine::bbox).collect();
        let ord = order::reading_order(&boxes, width as f32);
        let mut slots: Vec<Option<TextLine>> = lines.into_iter().map(Some).collect();
        let mut out = Vec::with_capacity(slots.len());
        for (b, block) in ord.iter().enumerate() {
            for (r, row) in block.iter().enumerate() {
                for (s, &i) in row.iter().enumerate() {
                    if let Some(l) = slots.get_mut(i).and_then(Option::take) {
                        out.push(((b as u32, r as u32, s as u32), l));
                    }
                }
            }
        }
        Page { width, height, lines: out }
    }

    /// Plain text: one line per text row, segments of a row joined by a space (like the measured
    /// Python reference).
    pub fn text(&self) -> String {
        let mut s = String::new();
        let mut last: Option<(u32, u32)> = None;
        for ((b, r, _), l) in &self.lines {
            if last == Some((*b, *r)) {
                s.push(' ');
            } else if last.is_some() {
                s.push('\n');
            }
            s.push_str(&l.text);
            last = Some((*b, *r));
        }
        if !s.is_empty() {
            s.push('\n');
        }
        s
    }
}

/// Handwriting second pass (stage 5): lines whose PP-OCR score is below `below` are read again
/// with DE·HTR; DE·HTR's text replaces the line only if its own confidence is higher. Word boxes of
/// a replaced line are distributed over the line box by character share
/// ([`crate::htr::word_fractions`]); every word gets DE·HTR's line confidence.
///
/// At most `max_lines` lines per page are read again (the ones closest below `below` first): a
/// page with show-through produced 83 garbage lines and +33 s in the measurement; real handwriting
/// pages of the corpus have fewer than 40 lines.
pub struct HtrPass<'a> {
    pub htr: &'a crate::htr::Htr,
    pub below: f32,
    pub max_lines: usize,
}

/// Default for [`HtrPass::max_lines`].
pub const HTR_MAX_LINES: usize = 40;

/// Indices of the lines the handwriting pass reads (score below `below`, best `max_lines`).
fn htr_candidates(decoded: &[Decoded], h: &HtrPass<'_>) -> Vec<usize> {
    let mut c: Vec<usize> = (0..decoded.len()).filter(|&i| decoded[i].score < h.below).collect();
    c.sort_by(|&a, &b| decoded[b].score.total_cmp(&decoded[a].score));
    c.truncate(h.max_lines);
    c
}

/// Longest side for the second detection attempt (see [`PpOcr::recognize_lines`]).
pub const RETRY_MAX_SIDE: usize = 2560;

/// Default longest side (stage 3): 1920 px keeps the corpus recall (88.8 vs 88.9 % at 2560, CER
/// 11.6 vs 13.1 %) and is 14 % faster on arm64 (Jetson Orin), 25 % on x86.
pub const DEFAULT_MAX_SIDE: usize = 1920;

/// Detector + recognizer pair with its character set.
pub struct PpOcr {
    det: Box<dyn Model>,
    rec: Box<dyn Model>,
    charset: Vec<String>,
    pub params: OcrParams,
}

impl PpOcr {
    /// The recognizer's character list comes from its ONNX metadata (`character`, as RapidOCR
    /// reads it); blank is prepended and a space appended (`CTCLabelDecode.get_character`).
    pub fn new(det: Box<dyn Model>, rec: Box<dyn Model>, params: OcrParams) -> Result<Self> {
        let chars = rec.metadata("character").ok_or_else(|| Error::Model("recognizer has no 'character' metadata".into()))?;
        let mut charset: Vec<String> = Vec::with_capacity(chars.lines().count() + 2);
        charset.push("blank".into());
        charset.extend(chars.lines().map(str::to_string));
        charset.push(" ".into());
        Ok(Self { det, rec, charset, params })
    }

    pub fn charset_len(&self) -> usize {
        self.charset.len()
    }

    /// Detects and reads all text lines of `img` (any size, gray or RGB). Lines in detector order.
    ///
    /// PaperRust addition: if the detector finds nothing at `max_side`, detection is retried once at
    /// another size ([`RETRY_MAX_SIDE`], or 1920 px when `max_side` is at least that). Measured:
    /// PP-OCRv6 small (also in the Python reference) finds nothing on an upstream test page
    /// (`multi-page-images.tiff`, 2476×3204, one line of 12 pt text) at 2560 px, but reads it at
    /// 1280/1920/3600 px. The retry costs one detector run, only on such pages.
    pub fn recognize_lines(&self, img: &Image) -> Result<Vec<TextLine>> {
        self.recognize_lines_with(img, None)
    }

    /// [`Self::recognize_lines`] with an optional handwriting second pass ([`HtrPass`]).
    pub fn recognize_lines_with(&self, img: &Image, htr: Option<&HtrPass<'_>>) -> Result<Vec<TextLine>> {
        let p = &self.params;
        if img.width == 0 || img.height == 0 {
            return Ok(Vec::new());
        }
        let lines = self.recognize_at(img, p.max_side, htr)?;
        let retry = if p.max_side >= RETRY_MAX_SIDE { 1920 } else { RETRY_MAX_SIDE };
        if lines.is_empty() && retry != p.max_side && img.width.max(img.height) > retry.min(p.max_side) {
            return self.recognize_at(img, retry, htr);
        }
        Ok(lines)
    }

    fn recognize_at(&self, img: &Image, max_side: usize, htr: Option<&HtrPass<'_>>) -> Result<Vec<TextLine>> {
        let p = &self.params;
        // 1. bound the image size (RapidOCR `resize_image_within_bounds`)
        let (work, ratio_w, ratio_h) = bound_size(img, p.min_side, max_side);
        // 2. vertical padding for very flat images (`apply_vertical_padding`, ratio 8, min height 30)
        let (work, pad_top) = vertical_padding(work, 8.0, 30);
        // 3. detection
        let boxes = self.detect(&work)?;
        if boxes.is_empty() {
            return Ok(Vec::new());
        }
        // 4. crops
        let mut crops: Vec<Crop> = Vec::with_capacity(boxes.len());
        for b in &boxes {
            if let Some(c) = crop(&work, &b.quad) {
                crops.push(c);
            }
        }
        // 5. recognition
        let decoded = self.recognize_crops(&crops)?;
        // 6. back to input coordinates, word boxes, score filter
        let (ow, oh) = (img.width as f32, img.height as f32);
        let to_orig =
            |pt: (f32, f32)| -> [f32; 2] { [((pt.0) * ratio_w).clamp(0.0, ow), ((pt.1 - pad_top as f32) * ratio_h).clamp(0.0, oh)] };
        let chosen = htr.map(|h| htr_candidates(&decoded, h)).unwrap_or_default();
        let mut lines = Vec::new();
        for (i, (c, d)) in crops.iter().zip(decoded).enumerate() {
            let quad: Quad = c.quad.map(|q| to_orig((q[0], q[1])));
            // second pass (stage 5): low-confidence line → DE·HTR; its text wins only if DE·HTR is
            // more confident than PP-OCR
            let second = match htr {
                Some(h) if chosen.contains(&i) => match h.htr.recognize(&c.img) {
                    Ok(r) => {
                        tracing::debug!(pp = d.score, htr = r.conf, pp_text = %d.text, htr_text = %r.text, "handwriting pass");
                        Some(r)
                    }
                    Err(e) => {
                        tracing::warn!("handwriting pass failed: {e}");
                        None
                    }
                },
                _ => None,
            };
            if let Some(r) = second
                && r.conf > d.score
                && !r.text.trim().is_empty()
                && r.conf >= p.text_score
            {
                let words = crate::htr::word_fractions(&r.text)
                    .into_iter()
                    .map(|(t, f0, f1)| {
                        let pts = c.span_to_source(f0, f1).map(to_orig);
                        let (a, b, cc, dd) = geom::bounds(&pts);
                        OcrWord { text: t, bbox: [a, b, cc, dd], conf: r.conf }
                    })
                    .collect();
                lines.push(TextLine { quad, text: r.text, score: r.conf, words });
                continue;
            }
            if d.text.trim().is_empty() || d.score < p.text_score {
                continue;
            }
            let spans = ctc::word_spans(&d, c.text_w as f32);
            let words = d
                .words
                .iter()
                .zip(spans)
                .map(|(w, (x0, x1))| {
                    let f0 = x0 / c.text_w.max(1) as f32;
                    let f1 = x1 / c.text_w.max(1) as f32;
                    let pts = c.span_to_source(f0, f1).map(to_orig);
                    let (a, b, cc, dd) = geom::bounds(&pts);
                    OcrWord { text: w.text.clone(), bbox: [a, b, cc, dd], conf: w.conf() }
                })
                .collect();
            lines.push(TextLine { quad, text: d.text, score: d.score, words });
        }
        Ok(lines)
    }

    /// [`Self::recognize_lines`] + reading order.
    pub fn recognize(&self, img: &Image) -> Result<Page> {
        self.recognize_with(img, None)
    }

    /// [`Self::recognize`] with an optional handwriting second pass.
    pub fn recognize_with(&self, img: &Image, htr: Option<&HtrPass<'_>>) -> Result<Page> {
        let lines = self.recognize_lines_with(img, htr)?;
        Ok(Page::from_lines(img.width, img.height, lines))
    }

    fn detect(&self, img: &Image) -> Result<Vec<DetBox>> {
        let p = &self.params;
        let (h, w) = (img.height as f32, img.width as f32);
        let ratio = if h.min(w) < p.det_limit_side as f32 { p.det_limit_side as f32 / h.min(w) } else { 1.0 };
        let rh = round32((h * ratio) as usize);
        let rw = round32((w * ratio) as usize);
        if rh == 0 || rw == 0 {
            return Ok(Vec::new());
        }
        let resized = img.resize(rw, rh);
        let data = resized.to_chw(1.0 / 255.0, [0.5; 3], [0.5; 3], true);
        drop(resized);
        let out = self.det.run(Tensor::new(vec![1, 3, rh, rw], data)?)?;
        let (mh, mw) = match out.shape.as_slice() {
            [_, _, mh, mw] => (*mh, *mw),
            s => return Err(Error::Model(format!("unexpected detector output shape {s:?}"))),
        };
        let boxes = db::boxes_from_map(&out.data, mw, mh, img.width, img.height, &p.db);
        Ok(db::sort_boxes(boxes))
    }

    fn recognize_crops(&self, crops: &[Crop]) -> Result<Vec<Decoded>> {
        let p = &self.params;
        let hh = p.rec_height;
        let ratio = |c: &Crop| c.img.width as f32 / c.img.height.max(1) as f32;
        let mut idx: Vec<usize> = (0..crops.len()).collect();
        idx.sort_by(|&a, &b| ratio(&crops[a]).total_cmp(&ratio(&crops[b])));
        let mut res: Vec<Option<Decoded>> = vec![None; crops.len()];
        for chunk in idx.chunks(p.rec_batch.max(1)) {
            let mut max_wh = 320.0 / 48.0f32;
            for &i in chunk {
                max_wh = max_wh.max(ratio(&crops[i]));
            }
            let bw = ((hh as f32 * max_wh) as usize).max(1);
            let plane = hh * bw;
            let mut data = vec![0f32; chunk.len() * 3 * plane];
            for (k, &i) in chunk.iter().enumerate() {
                let im = &crops[i].img;
                let rw = ((hh as f32 * ratio(&crops[i])).ceil() as usize).clamp(1, bw);
                let resized = im.resize(rw, hh);
                resized.write_chw(&mut data[k * 3 * plane..(k + 1) * 3 * plane], bw, 1.0 / 255.0, [0.5; 3], [0.5; 3], true);
            }
            let out = self.rec.run(Tensor::new(vec![chunk.len(), 3, hh, bw], data)?)?;
            let (n, t, c) = match out.shape.as_slice() {
                [n, t, c] => (*n, *t, *c),
                s => return Err(Error::Model(format!("unexpected recognizer output shape {s:?}"))),
            };
            if n != chunk.len() || c != self.charset.len() {
                return Err(Error::Model(format!(
                    "recognizer output {n}x{t}x{c} does not match batch {} / charset {}",
                    chunk.len(),
                    self.charset.len()
                )));
            }
            for (k, &i) in chunk.iter().enumerate() {
                let probs = &out.data[k * t * c..(k + 1) * t * c];
                res[i] = Some(ctc::decode(probs, t, c, &self.charset, ratio(&crops[i]), max_wh));
            }
        }
        Ok(res.into_iter().map(|d| d.unwrap_or(Decoded { text: String::new(), score: 0.0, words: Vec::new(), line_len: 0.0 })).collect())
    }
}

/// A text line crop and how to map positions in it back to the work image.
struct Crop {
    quad: Quad,
    img: Image,
    /// Crop → work image (before the optional rotation).
    h: Homography,
    /// Unrotated crop size.
    cw: f32,
    ch: f32,
    /// Crop was rotated by 90° counter-clockwise (vertical text, RapidOCR `get_rotate_crop_image`).
    rotated: bool,
    /// Width along the text direction (crop width after rotation).
    text_w: usize,
}

impl Crop {
    /// Corners in the work image of the part of the line between fractions `f0..f1` of the text
    /// width.
    fn span_to_source(&self, f0: f32, f1: f32) -> [(f32, f32); 4] {
        let pts: [(f32, f32); 4] = if self.rotated {
            // rotated image x' runs along the original crop's y axis
            [(0.0, f0 * self.ch), (self.cw, f0 * self.ch), (self.cw, f1 * self.ch), (0.0, f1 * self.ch)]
        } else {
            [(f0 * self.cw, 0.0), (f1 * self.cw, 0.0), (f1 * self.cw, self.ch), (f0 * self.cw, self.ch)]
        };
        pts.map(|(x, y)| self.h.apply(x, y))
    }
}

/// RapidOCR `get_rotate_crop_image`.
fn crop(img: &Image, q: &Quad) -> Option<Crop> {
    let cw = geom::dist(q[0], q[1]).max(geom::dist(q[2], q[3])) as usize;
    let ch = geom::dist(q[0], q[3]).max(geom::dist(q[1], q[2])) as usize;
    if cw == 0 || ch == 0 {
        return None;
    }
    let (im, h) = img.warp_quad(q, cw, ch)?;
    let rotated = ch as f32 / cw as f32 >= 1.5;
    let im = if rotated { im.rot90_ccw() } else { im };
    let text_w = im.width;
    Some(Crop { quad: *q, img: im, h, cw: cw as f32, ch: ch as f32, rotated, text_w })
}

fn round32(v: usize) -> usize {
    ((v as f32 / 32.0).round() as usize) * 32
}

/// RapidOCR `resize_image_within_bounds`: returns the image and the factors back to the input.
fn bound_size(img: &Image, min_side: usize, max_side: usize) -> (Image, f32, f32) {
    let (h, w) = (img.height, img.width);
    let mut out: Option<Image> = None;
    let (mut rw_, mut rh_) = (1.0f32, 1.0f32);
    if h.max(w) > max_side {
        let ratio = max_side as f32 / h.max(w) as f32;
        let (nh, nw) = (round32((h as f32 * ratio) as usize).max(32), round32((w as f32 * ratio) as usize).max(32));
        out = Some(img.resize(nw, nh));
        rh_ = h as f32 / nh as f32;
        rw_ = w as f32 / nw as f32;
    }
    let cur = out.as_ref().unwrap_or(img);
    let (ch, cw) = (cur.height, cur.width);
    if ch.min(cw) < min_side {
        let ratio = min_side as f32 / ch.min(cw) as f32;
        let (nh, nw) = (round32((ch as f32 * ratio) as usize).max(32), round32((cw as f32 * ratio) as usize).max(32));
        let resized = cur.resize(nw, nh);
        // like RapidOCR: the factors refer to the last resize only
        rh_ = ch as f32 / nh as f32;
        rw_ = cw as f32 / nw as f32;
        return (resized, rw_, rh_);
    }
    (out.unwrap_or_else(|| img.clone()), rw_, rh_)
}

/// RapidOCR `apply_vertical_padding`.
fn vertical_padding(img: Image, ratio: f32, min_height: usize) -> (Image, usize) {
    let (h, w) = (img.height, img.width);
    if h <= min_height || w as f32 / h as f32 > ratio {
        let new_h = ((w as f32 / ratio) as usize).max(min_height) * 2;
        let pad = new_h.abs_diff(h) / 2;
        return (img.pad_vertical(pad, pad), pad);
    }
    (img, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x0: f32, y0: f32, x1: f32, y1: f32) -> TextLine {
        TextLine { quad: [[x0, y0], [x1, y0], [x1, y1], [x0, y1]], text: text.into(), score: 0.9, words: Vec::new() }
    }

    #[test]
    fn page_text_joins_rows_and_segments() {
        let p = Page::from_lines(
            1000,
            500,
            vec![line("b", 300.0, 10.0, 400.0, 30.0), line("a", 0.0, 10.0, 100.0, 30.0), line("c", 0.0, 60.0, 100.0, 80.0)],
        );
        assert_eq!(p.text(), "a b\nc\n");
        assert_eq!(p.lines[0].0, (0, 0, 0));
        assert_eq!(p.lines[1].0, (0, 0, 1));
        assert_ne!(p.lines[2].0, p.lines[1].0);
        assert_eq!(Page::default().text(), "");
    }

    #[test]
    fn htr_candidates_prefer_near_misses_and_cap() {
        let d = |s: f32| Decoded { text: "x".into(), score: s, words: Vec::new(), line_len: 1.0 };
        let dec = vec![d(0.99), d(0.5), d(0.94), d(0.7), d(0.96)];
        let htr = crate::htr::Htr::new(Box::new(NoModel));
        let p = HtrPass { htr: &htr, below: 0.95, max_lines: 2 };
        assert_eq!(htr_candidates(&dec, &p), vec![2, 3]);
        let p = HtrPass { htr: &htr, below: 0.95, max_lines: 10 };
        assert_eq!(htr_candidates(&dec, &p), vec![2, 3, 1]);
    }

    struct NoModel;
    impl Model for NoModel {
        fn run(&self, _: Tensor) -> Result<Tensor> {
            Err(Error::Model("none".into()))
        }
        fn metadata(&self, _: &str) -> Option<String> {
            None
        }
    }

    #[test]
    fn size_bounds() {
        let img = Image::filled(5000, 3000, 1, 255);
        let (r, fw, fh) = bound_size(&img, 30, 2560);
        assert_eq!((r.width, r.height), (2560, 1536));
        assert!((fw - 5000.0 / 2560.0).abs() < 1e-5 && (fh - 3000.0 / 1536.0).abs() < 1e-5);
        let small = Image::filled(100, 20, 1, 255);
        let (r, _, fh) = bound_size(&small, 30, 2560);
        assert_eq!((r.width, r.height), (160, 32));
        assert!((fh - 20.0 / 32.0).abs() < 1e-5);
        let (r, fw, _) = bound_size(&Image::filled(800, 600, 3, 0), 30, 2560);
        assert_eq!((r.width, fw), (800, 1.0));
    }

    #[test]
    fn padding_for_flat_images() {
        let (p, top) = vertical_padding(Image::filled(900, 50, 1, 255), 8.0, 30);
        // new_h = max(900/8=112, 30)*2 = 224 → pad (224-50)/2 = 87
        assert_eq!(top, 87);
        assert_eq!(p.height, 50 + 2 * 87);
        let (p, top) = vertical_padding(Image::filled(400, 300, 1, 255), 8.0, 30);
        assert_eq!((top, p.height), (0, 300));
    }

    #[test]
    fn crop_maps_spans_back() {
        let img = Image::filled(200, 100, 1, 255);
        let q = [[10.0, 20.0], [110.0, 20.0], [110.0, 40.0], [10.0, 40.0]];
        let c = crop(&img, &q).unwrap();
        assert_eq!((c.img.width, c.img.height, c.rotated), (100, 20, false));
        let s = c.span_to_source(0.5, 1.0);
        assert!((s[0].0 - 60.0).abs() < 1e-3 && (s[0].1 - 20.0).abs() < 1e-3);
        assert!((s[2].0 - 110.0).abs() < 1e-3 && (s[2].1 - 40.0).abs() < 1e-3);
        // vertical box → rotated crop, span runs along y
        let qv = [[10.0, 10.0], [30.0, 10.0], [30.0, 90.0], [10.0, 90.0]];
        let c = crop(&img, &qv).unwrap();
        assert!(c.rotated);
        assert_eq!((c.img.width, c.img.height), (80, 20));
        let s = c.span_to_source(0.0, 0.5);
        let (_, y0, _, y1) = geom::bounds(&s.map(|p| [p.0, p.1]));
        assert!((y0 - 10.0).abs() < 1e-3 && (y1 - 50.0).abs() < 1e-3);
    }
}
