//! Image preprocessing before text recognition: straightening (`--deskew`) and cleaning
//! (`--clean` / `--clean-final`) – PaperRust's own replacement for Leptonica deskew and `unpaper`,
//! entirely without external programs.
//!
//! Model: OCRmyPDF 17.12 `_pipeline.py` (`preprocess_deskew`, `preprocess_clean`,
//! `create_visible_page_*`), `_exec/unpaper.py` (`clean`), Leptonica `pixFindSkew`/`pixDeskew`
//! (projection profile, sum of squared row differences) and unpaper (noisefilter,
//! grayfilter, border/mask detection).
//!
//! - [`estimate_skew`]: skew on a downscaled grayscale image (longest side
//!   ≤ [`SKEW_TARGET_PX`]), coarse ±[`MAX_SKEW_DEGREES`]° in 0.25° steps, then fine in
//!   0.02° steps with parabola interpolation. Dark areas touching the image edge
//!   (scanner edges, table in photos) do not count.
//! - [`rotate`]: one-time rotation at full size (bilinear, same image size, padded with white –
//!   like Leptonica `L_BRING_IN_WHITE`).
//! - [`clean_mask`]: which pixels become white: dark edge areas (connected from the image
//!   edge), punch holes in the margin area, faint gray tiles (light gray filtering)
//!   and small isolated specks. [`apply_mask`] applies the mask to gray or color images
//!   (color is kept with `clean-final`).

use std::borrow::Cow;

use crate::render::Raster;

/// Smaller angles are not rotated (image quality, archive stays original).
pub const MIN_DESKEW_DEGREES: f64 = 0.1;
/// Search range of the skew (±).
pub const MAX_SKEW_DEGREES: f64 = 15.0;
/// Longest side of the image on which the angle is determined.
pub const SKEW_TARGET_PX: usize = 1800;
/// Minimum quality (best / mean profile measure of the coarse search); below: no text lines detected.
pub const MIN_SKEW_CONFIDENCE: f64 = 1.5;

const COARSE_STEP: f64 = 0.25;
const FINE_RANGE: f64 = 0.3;
const FINE_STEP: f64 = 0.02;
const MAX_POINTS_FINE: usize = 300_000;
const MAX_POINTS_COARSE: usize = 60_000;
const MIN_POINTS: usize = 150;

/// Below this gray value a pixel counts as dark when cleaning (unpaper: black-threshold).
const DARK: u8 = 128;
/// Tiles whose darkest pixel is at least this bright become white (light gray filtering;
/// unpaper uses 128 – that would erase light gray text).
const GRAY_FILTER_MIN: u8 = 192;
/// A block counts as "dense" (edge area) if its mean is below this.
const DENSE_BLOCK_MEAN: u32 = 170;
/// A block counts as "fully black" (punch hole) if its mean is below this.
const SOLID_BLOCK_MEAN: u32 = 128;

// ----------------------------------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------------------------------

/// ITU-R 601-2 like Pillow `convert("L")` (same formula as the PDFium gray path in `render`).
#[inline]
fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114 + 500) / 1000) as u8
}

/// Grayscale copy (1 channel); gray images are only cloned.
pub fn to_gray(r: &Raster) -> Raster {
    Raster { width: r.width, height: r.height, channels: 1, data: gray_data(r).into_owned(), dpi: r.dpi }
}

fn gray_data(r: &Raster) -> Cow<'_, [u8]> {
    let ch = usize::from(r.channels.max(1));
    if ch == 1 {
        return Cow::Borrowed(&r.data);
    }
    Cow::Owned(r.data.chunks_exact(ch).map(|p| if ch >= 3 { luma(p[0], p[1], p[2]) } else { p[0] }).collect())
}

/// Color image without color (all pixels R≈G≈B, deviation ≤ 2) → 1 channel (smaller archive).
pub fn reduce_if_neutral(r: Raster) -> Raster {
    if r.channels != 3 {
        return r;
    }
    let neutral = r.data.as_chunks::<3>().0.iter().all(|p| p[0].abs_diff(p[1]) <= 2 && p[1].abs_diff(p[2]) <= 2);
    if neutral { to_gray(&r) } else { r }
}

fn dpi_or_default(dpi: f64) -> f64 {
    if dpi.is_finite() && dpi >= 30.0 { dpi } else { 300.0 }
}

/// Box averaging by the integer factor `f`.
fn downsample(px: &[u8], w: usize, h: usize, f: usize) -> (Vec<u8>, usize, usize) {
    if f <= 1 {
        return (px.to_vec(), w, h);
    }
    let (sw, sh) = (w.div_ceil(f), h.div_ceil(f));
    let mut sum = vec![0u32; sw * sh];
    let mut cnt = vec![0u32; sw * sh];
    for y in 0..h {
        let row = &px[y * w..(y + 1) * w];
        let base = (y / f) * sw;
        for (x, v) in row.iter().enumerate() {
            let i = base + x / f;
            sum[i] += u32::from(*v);
            cnt[i] += 1;
        }
    }
    let out = sum.iter().zip(&cnt).map(|(s, c)| (s / (*c).max(1)) as u8).collect();
    (out, sw, sh)
}

/// Otsu-Schwelle.
fn otsu(px: &[u8]) -> u8 {
    let mut hist = [0u64; 256];
    for v in px {
        hist[usize::from(*v)] += 1;
    }
    let total = px.len() as f64;
    if total == 0.0 {
        return DARK;
    }
    let sum_all: f64 = hist.iter().enumerate().map(|(i, c)| i as f64 * *c as f64).sum();
    let (mut w_b, mut sum_b, mut best, mut best_t) = (0.0f64, 0.0f64, -1.0f64, DARK);
    for (t, c) in hist.iter().enumerate() {
        w_b += *c as f64;
        if w_b == 0.0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0.0 {
            break;
        }
        sum_b += t as f64 * *c as f64;
        let m_b = sum_b / w_b;
        let m_f = (sum_all - sum_b) / w_f;
        let between = w_b * w_f * (m_b - m_f) * (m_b - m_f);
        if between > best {
            best = between;
            best_t = t as u8;
        }
    }
    best_t
}

/// Flood fill (8-neighborhood) over `pass` from the start points; result: reached points.
fn flood(w: usize, h: usize, seeds: impl IntoIterator<Item = usize>, pass: impl Fn(usize) -> bool) -> Vec<bool> {
    let mut seen = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    for s in seeds {
        if s < w * h && !seen[s] && pass(s) {
            seen[s] = true;
            stack.push(s);
        }
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let (x0, x1) = (x.saturating_sub(1), (x + 1).min(w - 1));
        let (y0, y1) = (y.saturating_sub(1), (y + 1).min(h - 1));
        for ny in y0..=y1 {
            for nx in x0..=x1 {
                let j = ny * w + nx;
                if !seen[j] && pass(j) {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
    }
    seen
}

/// Indices of the outermost image frame (1 pixel wide).
fn frame(w: usize, h: usize) -> impl Iterator<Item = usize> {
    let top = 0..w;
    let bottom = (0..w).map(move |x| (h - 1) * w + x);
    let left = (0..h).map(move |y| y * w);
    let right = (0..h).map(move |y| y * w + w - 1);
    top.chain(bottom).chain(left).chain(right)
}

// ----------------------------------------------------------------------------------------------
// Skew
// ----------------------------------------------------------------------------------------------

/// Detected skew.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Skew {
    /// Degrees; positive = text lines slope down to the right (content rotated clockwise).
    /// Straightening = [`rotate`] by `-angle`.
    pub angle: f64,
    /// Best / mean profile measure of the coarse search (≥ [`MIN_SKEW_CONFIDENCE`]).
    pub confidence: f64,
}

/// Profile measure for the shear `tan`: sum of squared differences of adjacent rows
/// (Leptonica `pixFindDifferentialSquareSum`). Sharp text lines → large value.
fn profile_score(pts: &[(f32, f32)], tan: f64, offset: f64, hist: &mut [u32]) -> f64 {
    hist.fill(0);
    let tan = tan as f32;
    let off = offset as f32;
    let n = hist.len();
    for &(x, y) in pts {
        let b = y - x * tan + off;
        if b >= 0.0 {
            let b = b as usize;
            if b < n {
                hist[b] += 1;
            }
        }
    }
    hist.windows(2)
        .map(|p| {
            let d = f64::from(p[1]) - f64::from(p[0]);
            d * d
        })
        .sum()
}

/// Determine the skew of a page (`None`: no text lines recognizable or angle at the edge of the
/// search range).
pub fn estimate_skew(r: &Raster) -> Option<Skew> {
    let (w, h) = (r.width as usize, r.height as usize);
    if w < 16 || h < 16 {
        return None;
    }
    let gray = gray_data(r);
    let f = w.max(h).div_ceil(SKEW_TARGET_PX).max(1);
    let (s, sw, sh) = downsample(&gray, w, h, f);
    drop(gray);
    let t = otsu(&s).clamp(64, 200);
    // Exclude dark areas at the image edge (scanner edge, table).
    let border = flood(sw, sh, frame(sw, sh), |i| s[i] < t);
    let cx = sw as f32 / 2.0;
    let mut pts: Vec<(f32, f32)> = Vec::new();
    for y in 0..sh {
        for x in 0..sw {
            let i = y * sw + x;
            if s[i] < t && !border[i] {
                pts.push((x as f32 - cx, y as f32));
            }
        }
    }
    drop(border);
    if pts.len() < MIN_POINTS {
        return None;
    }
    let thin = |v: &[(f32, f32)], max: usize| -> Vec<(f32, f32)> {
        let step = v.len().div_ceil(max).max(1);
        v.iter().step_by(step).copied().collect()
    };
    let fine_pts = if pts.len() > MAX_POINTS_FINE { thin(&pts, MAX_POINTS_FINE) } else { pts };
    let coarse_pts = thin(&fine_pts, MAX_POINTS_COARSE);
    let max_tan = (MAX_SKEW_DEGREES + 1.0).to_radians().tan();
    let offset = (sw as f64 / 2.0) * max_tan + 2.0;
    let mut hist = vec![0u32; sh + 2 * offset.ceil() as usize + 4];

    // coarse search
    let steps = (MAX_SKEW_DEGREES / COARSE_STEP).round() as i64;
    let mut coarse: Vec<(f64, f64)> = (-steps..=steps)
        .map(|k| {
            let a = k as f64 * COARSE_STEP;
            (a, profile_score(&coarse_pts, a.to_radians().tan(), offset, &mut hist))
        })
        .collect();
    let (best_a, best_s) = coarse.iter().copied().fold((0.0, f64::MIN), |b, c| if c.1 > b.1 { c } else { b });
    coarse.sort_by(|a, b| a.1.total_cmp(&b.1));
    let median = coarse[coarse.len() / 2].1;
    if median <= 0.0 || best_s <= 0.0 {
        return None;
    }
    let confidence = best_s / median;
    if confidence < MIN_SKEW_CONFIDENCE || best_a.abs() > MAX_SKEW_DEGREES - COARSE_STEP {
        return None;
    }

    // fine search around the coarse maximum
    let n = (FINE_RANGE / FINE_STEP).round() as i64;
    let fine: Vec<(f64, f64)> = (-n..=n)
        .map(|k| {
            let a = best_a + k as f64 * FINE_STEP;
            (a, profile_score(&fine_pts, a.to_radians().tan(), offset, &mut hist))
        })
        .collect();
    let mut bi = 0;
    for (i, c) in fine.iter().enumerate() {
        if c.1 > fine[bi].1 {
            bi = i;
        }
    }
    let mut angle = fine[bi].0;
    if bi > 0 && bi + 1 < fine.len() {
        let (l, m, r) = (fine[bi - 1].1, fine[bi].1, fine[bi + 1].1);
        let den = l - 2.0 * m + r;
        if den < 0.0 {
            angle += (0.5 * (l - r) / den).clamp(-0.5, 0.5) * FINE_STEP;
        }
    }
    Some(Skew { angle, confidence })
}

/// Rotate content by `degrees` clockwise (screen, y down) around the image center;
/// same image size, white outside, bilinear. For gray and color images.
pub fn rotate(r: &Raster, degrees: f64) -> Raster {
    let threads = if (r.width as usize) * (r.height as usize) < 1_000_000 {
        1
    } else {
        std::thread::available_parallelism().map_or(1, |n| n.get()).min(8)
    };
    rotate_n(r, degrees, threads)
}

/// [`rotate`] with a fixed number of threads (test: bit-identical to one thread).
fn rotate_n(r: &Raster, degrees: f64, threads: usize) -> Raster {
    let (w, h) = (r.width as usize, r.height as usize);
    let ch = usize::from(r.channels.max(1));
    if w == 0 || h == 0 || degrees == 0.0 || r.data.len() < w * h * ch {
        return r.clone();
    }
    let (s, c) = degrees.to_radians().sin_cos();
    let (cx, cy) = ((w as f64 - 1.0) / 2.0, (h as f64 - 1.0) / 2.0);
    let mut out = vec![255u8; w * h * ch];
    const ONE: f64 = 65536.0;
    let (step_x, step_y) = ((c * ONE).round() as i64, (-s * ONE).round() as i64);
    let src = &r.data;
    let (wi, hi) = (w as i64, h as i64);
    let row = |oy: usize, orow: &mut [u8]| {
        let dy = oy as f64 - cy;
        let dx = -cx;
        // source = R(-φ)·(o - c) + c
        let mut sx = ((cx + c * dx + s * dy) * ONE).round() as i64;
        let mut sy = ((cy - s * dx + c * dy) * ONE).round() as i64;
        for ox in 0..w {
            let (x0, y0) = (sx >> 16, sy >> 16);
            if x0 >= -1 && y0 >= -1 && x0 < wi && y0 < hi {
                let fx = ((sx >> 8) & 0xFF) as u32;
                let fy = ((sy >> 8) & 0xFF) as u32;
                let inside = x0 >= 0 && y0 >= 0 && x0 + 1 < wi && y0 + 1 < hi;
                for k in 0..ch {
                    let p = |x: i64, y: i64| -> u32 {
                        if x < 0 || y < 0 || x >= wi || y >= hi { 255 } else { u32::from(src[(y as usize * w + x as usize) * ch + k]) }
                    };
                    let (p00, p10, p01, p11) = if inside {
                        let i = (y0 as usize * w + x0 as usize) * ch + k;
                        let j = i + w * ch;
                        (u32::from(src[i]), u32::from(src[i + ch]), u32::from(src[j]), u32::from(src[j + ch]))
                    } else {
                        (p(x0, y0), p(x0 + 1, y0), p(x0, y0 + 1), p(x0 + 1, y0 + 1))
                    };
                    let top = p00 * (256 - fx) + p10 * fx;
                    let bot = p01 * (256 - fx) + p11 * fx;
                    orow[ox * ch + k] = ((top * (256 - fy) + bot * fy + 32768) >> 16) as u8;
                }
            }
            sx += step_x;
            sy += step_y;
        }
    };
    // Rows are independent: distribute large pages across several cores (Jetson measurement 19.11.:
    // scan per document 2.3 → 2.8 s after introducing straightening/cleaning; rotating is the largest
    // part). Result bit-identical to the single-core version.
    if threads <= 1 {
        for (oy, orow) in out.chunks_mut(w * ch).enumerate() {
            row(oy, orow);
        }
    } else {
        let rows_per = h.div_ceil(threads);
        std::thread::scope(|sc| {
            for (i, chunk) in out.chunks_mut(rows_per * w * ch).enumerate() {
                let row = &row;
                sc.spawn(move || {
                    for (j, orow) in chunk.chunks_mut(w * ch).enumerate() {
                        row(i * rows_per + j, orow);
                    }
                });
            }
        });
    }
    Raster { width: r.width, height: r.height, channels: r.channels, data: out, dpi: r.dpi }
}

/// Straightening: `Some((rotated image, skew))` if a skew ≥
/// [`MIN_DESKEW_DEGREES`] was detected.
pub fn deskew(r: &Raster) -> Option<(Raster, Skew)> {
    let skew = estimate_skew(r)?;
    (skew.angle.abs() >= MIN_DESKEW_DEGREES).then(|| (rotate(r, -skew.angle), skew))
}

// ----------------------------------------------------------------------------------------------
// Cleaning
// ----------------------------------------------------------------------------------------------

/// What was found when cleaning (pixels or count).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanReport {
    /// Points of dark edge areas (scanner edges, black borders).
    pub border_pixels: usize,
    /// Removed punch holes.
    pub holes: usize,
    /// Faint gray tiles set to white.
    pub gray_tiles: usize,
    /// Removed small specks.
    pub specks: usize,
    /// All points set to white.
    pub masked: usize,
    /// Page looks like a photo (paper gray instead of white): not cleaned.
    pub skipped_photo: bool,
}

/// Brightness of the paper (median of the bright points) below which a page counts as a photo.
/// Scanners deliver white paper (≈ 240–255); photographed pages are unevenly lit and
/// gray (Jetson check L1: median 211). Cleaning is built for scans – on photos it erased
/// i and umlaut dots as "specks" and broke the gray background into hard tiles (word rate
/// fell from 6/6 to 0/6). Better to clean once too little than to destroy text.
pub const PHOTO_PAPER_MEDIAN: u8 = 225;

/// Does the page look like a photo? Median of the bright points (≥ [`DARK`]) below
/// [`PHOTO_PAPER_MEDIAN`]. Almost only dark points (negative) does not count as a photo.
pub fn looks_like_photo(px: &[u8]) -> bool {
    let mut hist = [0usize; 256];
    for v in px {
        hist[usize::from(*v)] += 1;
    }
    let light: usize = hist[usize::from(DARK)..].iter().sum();
    if light * 5 < px.len() {
        return false;
    }
    let mut acc = 0usize;
    for (v, c) in hist.iter().enumerate().skip(usize::from(DARK)) {
        acc += c;
        if acc * 2 >= light {
            return v < usize::from(PHOTO_PAPER_MEDIAN);
        }
    }
    false
}

/// Mask of the points that become white when cleaning.
#[derive(Debug, Clone)]
pub struct CleanMask {
    pub width: u32,
    pub height: u32,
    pub mask: Vec<bool>,
    pub report: CleanReport,
}

impl CleanMask {
    pub fn is_empty(&self) -> bool {
        self.report.masked == 0
    }
}

/// Take along the light fringe (edge smoothing) around already masked areas (1 pixel).
fn add_halo(mask: &mut [bool], px: &[u8], w: usize, h: usize, region: &[bool]) {
    let mut add = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if region[i] || mask[i] || px[i] == 255 {
                continue;
            }
            let (x0, x1) = (x.saturating_sub(1), (x + 1).min(w - 1));
            let (y0, y1) = (y.saturating_sub(1), (y + 1).min(h - 1));
            if (y0..=y1).any(|ny| (x0..=x1).any(|nx| region[ny * w + nx])) {
                add.push(i);
            }
        }
    }
    for i in add {
        mask[i] = true;
    }
}

/// Which sub-steps of cleaning run (default: all). Can be switched off via `unpaper_args`
/// (gap E6): `--no-border-scan`/`--no-mask-scan`/`--no-border` → borders, `--no-blackfilter` →
/// punch holes, `--no-grayfilter` → gray filter, `--no-noisefilter` → specks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanFilters {
    pub border: bool,
    pub holes: bool,
    pub gray: bool,
    pub specks: bool,
}

impl Default for CleanFilters {
    fn default() -> Self {
        Self { border: true, holes: true, gray: true, specks: true }
    }
}

impl CleanFilters {
    /// Evaluate unpaper arguments (like OCRmyPDF `--unpaper-args`, separated by spaces).
    /// Returns: filters and the arguments that cannot be mapped (for a warning).
    pub fn from_unpaper_args(args: &str) -> (Self, Vec<String>) {
        let mut f = Self::default();
        let mut ignored = Vec::new();
        let mut it = args.split_whitespace().peekable();
        while let Some(a) = it.next() {
            match a {
                "--no-border-scan" | "--no-mask-scan" | "--no-border" => f.border = false,
                "--no-blackfilter" => f.holes = false,
                "--no-grayfilter" => f.gray = false,
                "--no-noisefilter" => f.specks = false,
                // no counterpart, but harmless (OCRmyPDF defaults)
                "--no-border-align" | "--no-mask-center" | "--no-deskew" | "--no-blurfilter" => {}
                "--layout" | "--mask-scan-size" => {
                    it.next();
                }
                other => {
                    ignored.push(other.to_owned());
                    // skip the value of an unknown switch
                    if other.starts_with('-') && it.peek().is_some_and(|n| !n.starts_with('-')) {
                        it.next();
                    }
                }
            }
        }
        (f, ignored)
    }
}

/// Determine cleaning (unpaper replacement) on an image; color images are converted to gray for this.
pub fn clean_mask(r: &Raster) -> CleanMask {
    clean_mask_with(r, CleanFilters::default())
}

/// [`clean_mask`] with selectable sub-steps.
pub fn clean_mask_with(r: &Raster, filters: CleanFilters) -> CleanMask {
    let (w, h) = (r.width as usize, r.height as usize);
    let mut rep = CleanReport::default();
    if w < 8 || h < 8 {
        return CleanMask { width: r.width, height: r.height, mask: vec![false; w * h], report: rep };
    }
    let px = gray_data(r);
    if looks_like_photo(&px) {
        rep.skipped_photo = true;
        return CleanMask { width: r.width, height: r.height, mask: vec![false; w * h], report: rep };
    }
    let dpi = dpi_or_default(r.dpi);
    let n = w * h;
    let mut mask = vec![false; n];

    // block means (≈ 0.7 mm)
    let bs = ((dpi / 36.0).round() as usize).clamp(2, 32);
    let (bw, bh) = (w.div_ceil(bs), h.div_ceil(bs));
    let mut bsum = vec![0u32; bw * bh];
    let mut bcnt = vec![0u32; bw * bh];
    for y in 0..h {
        let base = (y / bs) * bw;
        for x in 0..w {
            bsum[base + x / bs] += u32::from(px[y * w + x]);
            bcnt[base + x / bs] += 1;
        }
    }
    let bmean: Vec<u32> = bsum.iter().zip(&bcnt).map(|(s, c)| s / (*c).max(1)).collect();
    drop((bsum, bcnt));
    let block = |i: usize| (i / w / bs) * bw + (i % w) / bs;

    // 1. Dark edge areas: from the image frame over dark points that lie in a dense block
    //    or at most 2 mm from the image edge (thin lines/tables stop the flood).
    let edge = ((dpi / 25.4) * 2.0).round() as usize;
    let border = flood(w, h, frame(w, h).filter(|_| filters.border), |i| {
        if px[i] >= DARK {
            return false;
        }
        let (x, y) = (i % w, i / w);
        x < edge || y < edge || x + edge >= w || y + edge >= h || bmean[block(i)] < DENSE_BLOCK_MEAN
    });
    let border_count = border.iter().filter(|b| **b).count();
    // Almost everything dark (negative, dark photo): count nothing as edge.
    if border_count > 0 && border_count * 2 < n {
        for (m, b) in mask.iter_mut().zip(&border) {
            *m |= *b;
        }
        add_halo(&mut mask, &px, w, h, &border);
        rep.border_pixels = border_count;
    }
    drop(border);

    // 2. Punch holes: full black, round block groups of 4–10 mm in the outer margin area (8 %).
    let solid: Vec<bool> = (0..bw * bh).map(|b| bmean[b] < SOLID_BLOCK_MEAN).collect();
    let mut seen = vec![false; bw * bh];
    let mm = dpi / 25.4;
    let mut hole_blocks = vec![false; bw * bh];
    let mut comp: Vec<usize> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..bw * bh {
        if !filters.holes || !solid[start] || seen[start] {
            continue;
        }
        comp.clear();
        seen[start] = true;
        stack.push(start);
        while let Some(b) = stack.pop() {
            comp.push(b);
            let (bx, by) = (b % bw, b / bw);
            for ny in by.saturating_sub(1)..=(by + 1).min(bh - 1) {
                for nx in bx.saturating_sub(1)..=(bx + 1).min(bw - 1) {
                    let j = ny * bw + nx;
                    if solid[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        let mut touches_masked = false;
        for b in &comp {
            let (bx, by) = (b % bw, b / bw);
            x0 = x0.min(bx);
            y0 = y0.min(by);
            x1 = x1.max(bx);
            y1 = y1.max(by);
            let (px0, py0) = (bx * bs, by * bs);
            if mask[py0 * w + px0] {
                touches_masked = true;
            }
        }
        if touches_masked {
            continue;
        }
        let (cw, chh) = (((x1 - x0 + 1) * bs) as f64 / mm, ((y1 - y0 + 1) * bs) as f64 / mm);
        let aspect = cw / chh;
        let fill = comp.len() as f64 / ((x1 - x0 + 1) * (y1 - y0 + 1)) as f64;
        let (ccx, ccy) = (((x0 + x1 + 1) * bs / 2) as f64, ((y0 + y1 + 1) * bs / 2) as f64);
        let in_margin = ccx < 0.08 * w as f64 || ccx > 0.92 * w as f64 || ccy < 0.08 * h as f64 || ccy > 0.92 * h as f64;
        if in_margin && (4.0..=10.0).contains(&cw) && (4.0..=10.0).contains(&chh) && (0.75..=1.33).contains(&aspect) && fill >= 0.55 {
            rep.holes += 1;
            // Release the surroundings (±2 blocks) for the point flood.
            for by in y0.saturating_sub(2)..=(y1 + 2).min(bh - 1) {
                for bx in x0.saturating_sub(2)..=(x1 + 2).min(bw - 1) {
                    hole_blocks[by * bw + bx] = true;
                }
            }
            // start points: block centers of the punch hole
            let seeds: Vec<usize> = comp
                .iter()
                .map(|b| {
                    let (bx, by) = (b % bw, b / bw);
                    (by * bs + bs / 2).min(h - 1) * w + (bx * bs + bs / 2).min(w - 1)
                })
                .collect();
            let region = flood(w, h, seeds, |j| px[j] < DARK && hole_blocks[block(j)]);
            for (m, r) in mask.iter_mut().zip(&region) {
                *m |= *r;
            }
            add_halo(&mut mask, &px, w, h, &region);
            hole_blocks.fill(false);
        }
    }
    drop((solid, seen, hole_blocks));

    // 3. Light gray filtering: tiles (≈ 8 mm) without a point darker than GRAY_FILTER_MIN.
    let ts = ((dpi / 3.0).round() as usize).clamp(8, 400);
    for ty in (0..h).step_by(ts).filter(|_| filters.gray) {
        for tx in (0..w).step_by(ts) {
            let (ey, ex) = ((ty + ts).min(h), (tx + ts).min(w));
            let mut min = 255u8;
            for y in ty..ey {
                for x in tx..ex {
                    let i = y * w + x;
                    if !mask[i] {
                        min = min.min(px[i]);
                    }
                }
            }
            if (GRAY_FILTER_MIN..255).contains(&min) {
                rep.gray_tiles += 1;
                for y in ty..ey {
                    for m in &mut mask[y * w + tx..y * w + ex] {
                        *m = true;
                    }
                }
            }
        }
    }

    // 4. Small isolated specks (8-neighborhood), size scaled with the resolution:
    //    ≤ 6 points at 300 dpi (a full stop in 8 pt has ≥ 9 dark points there).
    let max_speck = ((6.0 * (dpi / 300.0).powi(2)).round() as usize).max(2);
    let mut visited = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut comp: Vec<usize> = Vec::new();
    let mut speck_region = vec![false; n];
    let mut any_speck = false;
    for start in (0..n).filter(|_| filters.specks) {
        if visited[start] || mask[start] || px[start] >= DARK {
            continue;
        }
        comp.clear();
        visited[start] = true;
        stack.push(start);
        let mut size = 0usize;
        while let Some(i) = stack.pop() {
            size += 1;
            if comp.len() <= max_speck {
                comp.push(i);
            }
            let (x, y) = (i % w, i / w);
            let (x0, x1) = (x.saturating_sub(1), (x + 1).min(w - 1));
            let (y0, y1) = (y.saturating_sub(1), (y + 1).min(h - 1));
            for ny in y0..=y1 {
                for nx in x0..=x1 {
                    let j = ny * w + nx;
                    if !visited[j] && !mask[j] && px[j] < DARK {
                        visited[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        if size <= max_speck {
            rep.specks += 1;
            any_speck = true;
            for i in &comp {
                speck_region[*i] = true;
            }
        }
    }
    drop(visited);
    if any_speck {
        for (m, s) in mask.iter_mut().zip(&speck_region) {
            *m |= *s;
        }
        add_halo(&mut mask, &px, w, h, &speck_region);
    }

    // Count only points that actually change.
    rep.masked = mask.iter().zip(px.iter()).filter(|(m, v)| **m && **v != 255).count();
    CleanMask { width: r.width, height: r.height, mask, report: rep }
}

/// Apply the mask (masked points white, all channels). Different image size → unchanged.
pub fn apply_mask(r: &mut Raster, m: &CleanMask) {
    if r.width != m.width || r.height != m.height {
        return;
    }
    let ch = usize::from(r.channels.max(1));
    for (p, on) in r.data.chunks_exact_mut(ch).zip(&m.mask) {
        if *on {
            p.fill(255);
        }
    }
}

// ----------------------------------------------------------------------------------------------
// Flow per page
// ----------------------------------------------------------------------------------------------

/// Which preprocessing a page gets (from the OCRmyPDF options).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Steps {
    /// `--deskew`: straighten (image for recognition *and* archive).
    pub deskew: bool,
    /// `--clean` or `--clean-final`: clean the recognition image.
    pub clean: bool,
    /// `--clean-final`: put the cleaned image into the archive too.
    pub clean_final: bool,
    /// Sub-steps of cleaning (`unpaper_args`).
    pub filters: CleanFilters,
}

impl Steps {
    pub fn any(&self) -> bool {
        self.deskew || self.clean || self.clean_final
    }
    /// Can the visible page image (archive) be changed?
    pub fn may_change_visible(&self) -> bool {
        self.deskew || self.clean_final
    }
}

/// Result of preprocessing a page.
#[derive(Debug, Clone)]
pub struct Prepared {
    /// Image for the archive (straightened; cleaned with `clean_final`).
    pub visible: Raster,
    /// `visible` differs from the input image (page must be replaced).
    pub visible_changed: bool,
    /// Image for recognition, if it differs from `visible` (cleaned, gray).
    pub ocr: Option<Raster>,
    pub skew: Option<Skew>,
    pub clean: Option<CleanReport>,
    /// Duration of straightening / cleaning.
    pub deskew_time: std::time::Duration,
    pub clean_time: std::time::Duration,
}

/// OCRmyPDF order: (rotation beforehand) → straighten → clean.
pub fn prepare(raster: Raster, steps: Steps) -> Prepared {
    let t0 = std::time::Instant::now();
    let (mut visible, mut changed, mut skew) = (raster, false, None);
    if steps.deskew {
        match estimate_skew(&visible) {
            Some(s) if s.angle.abs() >= MIN_DESKEW_DEGREES => {
                visible = rotate(&visible, -s.angle);
                changed = true;
                skew = Some(s);
            }
            s => skew = s,
        }
    }
    let deskew_time = t0.elapsed();
    let t1 = std::time::Instant::now();
    let (mut ocr, mut report) = (None, None);
    if steps.clean || steps.clean_final {
        let m = clean_mask_with(&visible, steps.filters);
        if !m.is_empty() {
            let mut g = to_gray(&visible);
            apply_mask(&mut g, &m);
            if steps.clean_final {
                apply_mask(&mut visible, &m);
                changed = true;
            }
            ocr = Some(g);
        }
        report = Some(m.report);
    }
    Prepared { visible, visible_changed: changed, ocr, skew, clean: report, deskew_time, clean_time: t1.elapsed() }
}

#[cfg(test)]
pub(crate) mod testpage {
    //! Synthetic pages (without font file): lines of "words" made of glyph rectangles with
    //! ascenders/descenders – for the projection profile like text.
    use crate::render::Raster;

    pub struct Lcg(pub u64);
    impl Lcg {
        pub fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 33
        }
        pub fn range(&mut self, a: usize, b: usize) -> usize {
            a + (self.next() as usize) % (b - a).max(1)
        }
    }

    pub fn fill(r: &mut Raster, x0: usize, y0: usize, x1: usize, y1: usize, v: u8) {
        let (w, h) = (r.width as usize, r.height as usize);
        let ch = usize::from(r.channels);
        for y in y0.min(h)..y1.min(h) {
            for x in x0.min(w)..x1.min(w) {
                for k in 0..ch {
                    r.data[(y * w + x) * ch + k] = v;
                }
            }
        }
    }

    /// Text page; font size scales with `dpi` (≈ 11 pt). `lines` = None: whole page.
    pub fn text_page(w: u32, h: u32, dpi: f64, lines: Option<usize>, seed: u64) -> Raster {
        let mut r = Raster { width: w, height: h, channels: 1, data: vec![255; (w * h) as usize], dpi };
        let s = dpi / 150.0;
        let px = |v: f64| (v * s).round().max(1.0) as usize;
        let mut rng = Lcg(seed);
        let (margin_x, margin_y) = ((w as f64 * 0.1) as usize, (h as f64 * 0.08) as usize);
        let mut y = margin_y + px(20.0);
        let mut n = 0;
        while y + px(30.0) < h as usize - margin_y && lines.is_none_or(|l| n < l) {
            let mut x = margin_x;
            let end = w as usize - margin_x - rng.range(0, px(200.0));
            while x < end {
                let glyphs = rng.range(2, 9);
                for _ in 0..glyphs {
                    let gw = rng.range(px(5.0), px(9.0));
                    let kind = rng.range(0, 10);
                    let (top, bot) = match kind {
                        0 | 1 => (y - px(6.0), y + px(10.0)), // ascender
                        2 => (y, y + px(15.0)),               // descender
                        _ => (y, y + px(10.0)),
                    };
                    // glyph as frame + stroke (not fully filled)
                    let st = px(1.6);
                    fill(&mut r, x, top, x + st, bot, 0);
                    fill(&mut r, x + gw - st, top, x + gw, bot, 0);
                    fill(&mut r, x, y, x + gw, y + st, 0);
                    fill(&mut r, x, y + px(10.0) - st, x + gw, y + px(10.0), 0);
                    x += gw + px(2.0);
                }
                x += rng.range(px(8.0), px(14.0));
            }
            y += px(28.0);
            n += 1;
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::testpage::*;
    use super::*;

    fn dark_count(r: &Raster, x0: usize, y0: usize, x1: usize, y1: usize) -> usize {
        let w = r.width as usize;
        let mut c = 0;
        for y in y0..y1.min(r.height as usize) {
            for x in x0..x1.min(w) {
                if r.data[y * w + x] < DARK {
                    c += 1;
                }
            }
        }
        c
    }

    #[test]
    fn skew_sign_convention() {
        // Draw lines directly with a slope (without `rotate`): slopes down to the right → positive.
        let (w, h) = (1000usize, 800usize);
        let mut r = Raster { width: w as u32, height: h as u32, channels: 1, data: vec![255; w * h], dpi: 150.0 };
        let slope = 3f64.to_radians().tan();
        for line in 0..18 {
            let y0 = 100.0 + line as f64 * 30.0;
            for x in 100..900 {
                if (x / 12) % 4 == 3 {
                    continue; // word gaps
                }
                let y = (y0 + (x as f64 - 100.0) * slope) as usize;
                fill(&mut r, x, y, x + 1, y + 8, 0);
            }
        }
        let s = estimate_skew(&r).expect("skew");
        assert!((s.angle - 3.0).abs() < 0.3, "{s:?}");
        // rotate(+a) rotates clockwise = positive skew angle
        let p = text_page(1240, 1754, 150.0, None, 1);
        let s = estimate_skew(&rotate(&p, 2.0)).expect("skew");
        assert!((s.angle - 2.0).abs() < 0.3, "{s:?}");
    }

    #[test]
    fn skew_known_angles_within_0_3_degrees() {
        let page = text_page(1240, 1754, 150.0, None, 7);
        let mut worst = 0.0f64;
        for a in [-13.0, -9.5, -5.0, -2.2, -0.8, -0.3, 0.0, 0.25, 0.6, 1.5, 3.7, 7.0, 11.0, 13.5] {
            let s = estimate_skew(&rotate(&page, a)).unwrap_or_else(|| panic!("no skew for {a}"));
            let err = (s.angle - a).abs();
            worst = worst.max(err);
            assert!(err <= 0.3, "angle {a}: got {:.3} (conf {:.2})", s.angle, s.confidence);
        }
        eprintln!("skew: worst error {worst:.3}°");
    }

    #[test]
    fn skew_few_lines_and_dark_border() {
        // only three lines
        let page = text_page(1240, 1754, 150.0, Some(3), 3);
        for a in [-4.0, 1.2, 6.0] {
            let s = estimate_skew(&rotate(&page, a)).unwrap_or_else(|| panic!("no skew for {a}"));
            assert!((s.angle - a).abs() <= 0.3, "few lines {a}: {s:?}");
        }
        // photo/scan with dark edge (table/scanner lid), page skewed on it
        let page = text_page(1100, 1600, 150.0, None, 4);
        for a in [-3.0, 2.5, 8.0] {
            let rot = rotate(&page, a);
            let mut big = Raster { width: 1240, height: 1754, channels: 1, data: vec![40; 1240 * 1754], dpi: 150.0 };
            // insert the rotated page; color corner wedges (white) dark like the table
            for y in 0..1600usize {
                for x in 0..1100usize {
                    big.data[(y + 77) * 1240 + x + 70] = rot.data[y * 1100 + x];
                }
            }
            let s = estimate_skew(&big).unwrap_or_else(|| panic!("no skew for border {a}"));
            assert!((s.angle - a).abs() <= 0.3, "border {a}: {s:?}");
        }
    }

    #[test]
    fn skew_none_for_blank_and_noise() {
        let blank = Raster { width: 800, height: 1000, channels: 1, data: vec![255; 800_000], dpi: 150.0 };
        assert_eq!(estimate_skew(&blank), None);
        let mut rng = Lcg(9);
        let noise: Vec<u8> = (0..800_000).map(|_| if rng.range(0, 100) < 8 { 0 } else { 255 }).collect();
        let noise = Raster { width: 800, height: 1000, channels: 1, data: noise, dpi: 150.0 };
        assert_eq!(estimate_skew(&noise), None);
        let mut rng = Lcg(5);
        let photo: Vec<u8> = (0..800_000).map(|i| ((i % 800) / 5 + rng.range(0, 60)) as u8).collect();
        assert_eq!(estimate_skew(&Raster { width: 800, height: 1000, channels: 1, data: photo, dpi: 150.0 }), None);
    }

    #[test]
    fn rotate_parallel_is_bit_identical() {
        let page = text_page(1300, 1700, 150.0, None, 5);
        let rgb = Raster {
            width: page.width,
            height: page.height,
            channels: 3,
            data: page.data.iter().enumerate().flat_map(|(i, v)| [*v, (i % 251) as u8, v / 2]).collect(),
            dpi: 150.0,
        };
        for r in [&page, &rgb] {
            for deg in [-3.7, 0.4, 12.0] {
                let one = rotate_n(r, deg, 1);
                for n in [2, 3, 7] {
                    assert_eq!(rotate_n(r, deg, n).data, one.data, "{n} Threads, {deg}°, {} Kanäle", r.channels);
                }
            }
        }
    }

    #[test]
    fn rotate_keeps_size_white_fill_and_color() {
        let mut r = Raster { width: 101, height: 61, channels: 3, data: vec![0; 101 * 61 * 3], dpi: 100.0 };
        let o = rotate(&r, 10.0);
        assert_eq!((o.width, o.height, o.channels), (101, 61, 3));
        assert_eq!(&o.data[0..3], &[255, 255, 255]); // corner filled in
        let c = (30 * 101 + 50) * 3;
        assert_eq!(&o.data[c..c + 3], &[0, 0, 0]); // center stays
        fill(&mut r, 0, 0, 101, 61, 200);
        assert_eq!(rotate(&r, 0.0).data, r.data);
        // 180° ≈ exact mirroring
        let p = text_page(300, 200, 100.0, None, 2);
        let q = rotate(&rotate(&p, 180.0), 180.0);
        let diff = p.data.iter().zip(&q.data).filter(|(a, b)| a.abs_diff(**b) > 8).count();
        assert!(diff < 50, "{diff}");
    }

    #[test]
    fn deskew_restores_lines() {
        let page = text_page(1240, 1754, 150.0, None, 11);
        let skewed = rotate(&page, 4.0);
        let (fixed, s) = deskew(&skewed).expect("deskew");
        assert!((s.angle - 4.0).abs() <= 0.3);
        let after = estimate_skew(&fixed).map_or(0.0, |s| s.angle);
        assert!(after.abs() <= 0.15, "{after}");
        // < 0.1°: do not rotate
        let tiny = rotate(&page, 0.04);
        assert!(deskew(&tiny).is_none_or(|(_, s)| s.angle.abs() >= MIN_DESKEW_DEGREES));
        assert!(deskew(&page).is_none());
    }

    /// Page with text + specks + black edge + wedge + punch hole + gray areas.
    fn dirty_page() -> (Raster, Raster, Vec<(usize, usize)>) {
        let (w, h, dpi) = (1240usize, 1754usize, 150.0);
        let clean = text_page(w as u32, h as u32, dpi, None, 21);
        let mut r = clean.clone();
        // scanner edge left (dark gray, 25 px) and dark wedge top right
        fill(&mut r, 0, 0, 25, h, 60);
        for y in 0..120usize {
            let x0 = w - 120 + y;
            fill(&mut r, x0, y, w, y + 1, 30);
        }
        // punch hole (6 mm ≈ 35 px) in the left margin
        let (hx, hy, rad) = (70i64, 877i64, 17i64);
        for y in hy - rad..=hy + rad {
            for x in hx - rad..=hx + rad {
                if (x - hx).pow(2) + (y - hy).pow(2) <= rad * rad {
                    r.data[y as usize * w + x as usize] = 20;
                }
            }
        }
        // faint gray areas (background, show-through)
        fill(&mut r, 400, 1650, 900, 1740, 225);
        // specks (1–2 points) on white spots
        let mut rng = Lcg(77);
        let mut specks = Vec::new();
        while specks.len() < 300 {
            let (x, y) = (rng.range(30, w - 30), rng.range(10, h - 10));
            let free = (y - 3..y + 4).all(|yy| (x - 3..x + 4).all(|xx| r.data[yy * w + xx] == 255));
            if free {
                r.data[y * w + x] = 0;
                if rng.range(0, 2) == 0 {
                    r.data[y * w + x + 1] = 10;
                }
                specks.push((x, y));
            }
        }
        (clean, r, specks)
    }

    /// Gap E6: `unpaper_args` switches off sub-steps.
    #[test]
    fn unpaper_args_disable_filters() {
        let (f, ignored) = CleanFilters::from_unpaper_args(
            "--layout none --no-noisefilter --no-grayfilter --mask-scan-size 100 --foo 3 --no-border-align",
        );
        assert_eq!(f, CleanFilters { border: true, holes: true, gray: false, specks: false });
        assert_eq!(ignored, vec!["--foo".to_string()]);
        let (f, ignored) = CleanFilters::from_unpaper_args("--no-border-scan --no-blackfilter");
        assert_eq!(f, CleanFilters { border: false, holes: false, gray: true, specks: true });
        assert!(ignored.is_empty());
        let (_, dirty, _) = dirty_page();
        let all = clean_mask(&dirty).report;
        let none = clean_mask_with(&dirty, CleanFilters { border: false, holes: false, gray: false, specks: false });
        assert!(none.is_empty(), "{:?}", none.report);
        let no_specks = clean_mask_with(&dirty, CleanFilters { specks: false, ..Default::default() }).report;
        assert_eq!(no_specks.specks, 0);
        assert_eq!((no_specks.holes, no_specks.gray_tiles), (all.holes, all.gray_tiles));
        assert!(no_specks.border_pixels > 0);
    }

    #[test]
    fn clean_removes_specks_borders_holes_keeps_text() {
        let (clean, dirty, specks) = dirty_page();
        let m = clean_mask(&dirty);
        let mut out = dirty.clone();
        apply_mask(&mut out, &m);
        let w = out.width as usize;
        for (x, y) in &specks {
            assert_eq!(out.data[y * w + x], 255, "speck at {x},{y}");
        }
        assert_eq!(dark_count(&out, 0, 0, 25, out.height as usize), 0, "left border");
        assert!(out.data[..25].iter().all(|v| *v == 255));
        assert_eq!(dark_count(&out, w - 120, 0, w, 120), 0, "wedge");
        assert!((40..100).all(|x| out.data[877 * w + x] == 255), "hole");
        assert!(out.data[1700 * w + 600] == 255, "gray area");
        // text stays (≥ 99.5 % of the dark points in the text area)
        let before = dark_count(&clean, 100, 100, w - 130, 1640);
        let after = dark_count(&out, 100, 100, w - 130, 1640);
        assert!(after * 1000 >= before * 995, "text kept {after}/{before}");
        assert!(m.report.specks >= 300 && m.report.holes == 1 && m.report.border_pixels > 0 && m.report.gray_tiles > 0, "{:?}", m.report);
        // clean page: (almost) nothing to do
        let m2 = clean_mask(&clean);
        assert_eq!(m2.report.specks, 0, "{:?}", m2.report);
        assert_eq!(m2.report.border_pixels, 0);
        assert_eq!(m2.report.holes, 0);
    }

    #[test]
    fn clean_speck_size_scales_with_dpi() {
        // 300 dpi: 2×2 speck (4 points) removed, 3×3 point (punctuation, 9 points) stays.
        let mut r = Raster { width: 400, height: 400, channels: 1, data: vec![255; 160_000], dpi: 300.0 };
        fill(&mut r, 100, 100, 102, 102, 0);
        fill(&mut r, 200, 200, 203, 203, 0);
        let m = clean_mask(&r);
        apply_mask(&mut r, &m);
        assert_eq!(r.data[100 * 400 + 100], 255);
        assert_eq!(r.data[201 * 400 + 201], 0);
    }

    /// Jetson check L1: photographed page (gray-lit paper, dark table, small
    /// i dots) is not cleaned; the same page on white paper is.
    #[test]
    fn clean_skips_photos_keeps_scans() {
        let mut photo = testpage::text_page(1200, 1600, 300.0, Some(20), 7);
        // paper gray with gradient (200..230), text stays dark, edge = table
        let w = photo.width as usize;
        for (i, v) in photo.data.iter_mut().enumerate() {
            let (x, y) = (i % w, i / w);
            if *v >= DARK {
                *v = (230 - (y * 30 / 1600)) as u8;
            }
            if x < 60 || y < 60 || x + 60 >= w || y + 60 >= 1600 {
                *v = 60;
            }
        }
        // an i dot (2×2) – would be removed as a speck
        testpage::fill(&mut photo, 600, 800, 602, 802, 20);
        let m = clean_mask(&photo);
        assert!(m.report.skipped_photo);
        assert!(m.is_empty(), "Foto bleibt unverändert");

        let (_, scan, _) = dirty_page();
        let m = clean_mask(&scan);
        assert!(!m.report.skipped_photo);
        assert!(!m.is_empty(), "Scan wird weiter gesäubert");
        assert!(!looks_like_photo(&[255; 100]));
        assert!(!looks_like_photo(&[10; 100]), "Negativ ist kein Foto");
        assert!(looks_like_photo(&[210; 100]));
    }

    #[test]
    fn clean_dark_document_and_color() {
        // Almost entirely dark (negative): no edge, nothing is deleted except specks
        let mut r = Raster { width: 300, height: 300, channels: 1, data: vec![20; 90_000], dpi: 150.0 };
        fill(&mut r, 100, 100, 200, 200, 255);
        let m = clean_mask(&r);
        assert_eq!(m.report.border_pixels, 0);
        // color: mask on RGB, color is kept
        let mut c = Raster { width: 300, height: 300, channels: 3, data: vec![255; 270_000], dpi: 150.0 };
        fill(&mut c, 0, 0, 15, 300, 30); // edge
        fill(&mut c, 150, 150, 170, 170, 0);
        for p in c.data.as_chunks_mut::<3>().0.iter_mut().skip(150 * 300 + 150).take(10) {
            p.copy_from_slice(&[200, 0, 0]); // red line (dark enough)
        }
        let m = clean_mask(&c);
        apply_mask(&mut c, &m);
        assert_eq!(&c.data[0..3], &[255, 255, 255]);
        let i = (150 * 300 + 151) * 3;
        assert_eq!(&c.data[i..i + 3], &[200, 0, 0]);
        let reduced = reduce_if_neutral(c.clone());
        assert_eq!(reduced.channels, 3);
        let gray = Raster { width: 2, height: 1, channels: 3, data: vec![10, 11, 10, 200, 200, 201], dpi: 1.0 };
        assert_eq!(reduce_if_neutral(gray).channels, 1);
    }

    #[test]
    fn prepare_combinations() {
        let page = text_page(1240, 1754, 150.0, None, 31);
        let skewed = rotate(&page, 3.0);
        let (_, dirty, _) = dirty_page();
        // none
        let p = prepare(skewed.clone(), Steps::default());
        assert!(!p.visible_changed && p.ocr.is_none() && p.skew.is_none());
        // deskew alone: archive changed, recognition = archive image
        let p = prepare(skewed.clone(), Steps { deskew: true, ..Default::default() });
        assert!(p.visible_changed && p.ocr.is_none());
        assert!((p.skew.unwrap().angle - 3.0).abs() <= 0.3);
        // straight page + deskew: nothing changed (original stays)
        let p = prepare(page.clone(), Steps { deskew: true, ..Default::default() });
        assert!(!p.visible_changed);
        // clean: recognition image only
        let p = prepare(dirty.clone(), Steps { clean: true, ..Default::default() });
        assert!(!p.visible_changed && p.ocr.is_some());
        assert_eq!(p.visible.data, dirty.data);
        // clean-final: archive image cleaned
        let p = prepare(dirty.clone(), Steps { clean: true, clean_final: true, ..Default::default() });
        assert!(p.visible_changed);
        assert_eq!(p.visible.data[877 * 1240 + 70], 255);
        // clean-final on a clean page: archive unchanged
        let p = prepare(page.clone(), Steps { clean_final: true, ..Default::default() });
        assert!(!p.visible_changed);
        // deskew + clean-final
        let p = prepare(rotate(&dirty, -2.0), Steps { deskew: true, clean: true, clean_final: true, ..Default::default() });
        assert!(p.visible_changed && p.ocr.is_some());
        assert!((p.skew.unwrap().angle + 2.0).abs() <= 0.3);
    }

    #[test]
    fn timing_a4_300dpi() {
        // Measurement (debug build is many times slower than release).
        let page = text_page(2480, 3508, 300.0, None, 41);
        let skewed = rotate(&page, 2.5);
        let t = std::time::Instant::now();
        let s = estimate_skew(&skewed).expect("skew");
        let t_est = t.elapsed();
        let t = std::time::Instant::now();
        let fixed = rotate(&skewed, -s.angle);
        let t_rot = t.elapsed();
        let t = std::time::Instant::now();
        let m = clean_mask(&fixed);
        let t_clean = t.elapsed();
        let rgb = Raster {
            width: fixed.width,
            height: fixed.height,
            channels: 3,
            data: fixed.data.iter().flat_map(|v| [*v, *v, *v]).collect(),
            dpi: 300.0,
        };
        let t = std::time::Instant::now();
        let _ = rotate(&rgb, 1.0);
        let t_rot_rgb = t.elapsed();
        eprintln!(
            "A4 300 dpi: Winkel {:.3}° (Soll 2.5) in {t_est:?}, Drehen grau {t_rot:?}, Drehen RGB {t_rot_rgb:?}, Säubern {t_clean:?} ({} Punkte)",
            s.angle, m.report.masked
        );
        assert!((s.angle - 2.5).abs() <= 0.3);
    }
}
