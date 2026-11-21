//! Windows' own text recognition (`Windows.Media.Ocr`, WinRT) as an [`OcrEngine`] – optional, selected
//! via `PAPERRUST_OCR_ENGINE` ([`crate::engine_select`]). The default remains Tesseract.
//!
//! - **Recognition:** raster → grayscale `SoftwareBitmap` (`Gray8`, one copy) →
//!   `OcrEngine.RecognizeAsync` → lines with words and boxes ([`OcrLine`]/[`OcrWord`]). The boxes
//!   are pixels of the passed image (axis-aligned); from them [`Word`]s with a line identifier
//!   (block 1, paragraph n, line m) are built for the invisible text layer, and the page text (lines with `\n`,
//!   paragraphs with a blank line, like Tesseract's `txt`).
//! - **Confidence:** Windows OCR does not provide it → `conf = -1` (not evaluated anywhere).
//! - **Size:** images above `OcrEngine.MaxImageDimension` (10 000 px) are recognized downscaled,
//!   the boxes recalculated to the original size.
//! - **Languages:** one language per recognition; mapping `PAPERLESS_OCR_LANGUAGE` → installed
//!   language packs see [`crate::engine_select::plan_languages`] (missing packs: clear message with
//!   installation command).
//! - **Orientation:** Windows OCR does not detect 90°/180° rotation (only the skew
//!   `TextAngle`). `rotate_pages` therefore uses Tesseract OSD if Tesseract is present; otherwise
//!   the page stays unrotated (one-time warning).
//! - **Concurrency:** WinRT is initialized as MTA per thread on demand (windows-core).
//!   `OcrEngine` instances live in a process-wide pool per language tag; each instance is used by
//!   only one worker at a time (like [`crate::tesslib`]). Waiting happens without a
//!   message loop (completion callback + channel, with `timeout`).
//! - **Release when idle:** like with Tesseract, the shared cleanup thread
//!   ([`crate::tesslib`]) deletes recognizers that have been unused in the pool for longer than `PAPERRUST_OCR_ENGINE_IDLE_SECS` (default 180 s,
//!   `0` = never) ([`IdlePool`], [`release_idle_all`]).
//!
//! The pure conversions ([`page_from_lines`], [`to_gray`], [`fit_scale`]) are
//! platform-independent and tested; the WinRT part exists only under `cfg(windows)`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::engine::{PageOcr, Word};

/// Pool of free instances per key with the time of return (platform-independent, for
/// Windows OCR recognizers; tested with any `T`).
#[derive(Debug)]
pub struct IdlePool<T> {
    free: HashMap<String, Vec<(T, Instant)>>,
}

impl<T> Default for IdlePool<T> {
    fn default() -> Self {
        Self { free: HashMap::new() }
    }
}

impl<T> IdlePool<T> {
    /// Most recently returned instance for `key` (LIFO: the "warmest" first).
    pub fn take(&mut self, key: &str) -> Option<T> {
        let v = self.free.get_mut(key)?;
        let out = v.pop().map(|(t, _)| t);
        if v.is_empty() {
            self.free.remove(key);
        }
        out
    }

    pub fn give_back(&mut self, key: &str, item: T, now: Instant) {
        self.free.entry(key.to_string()).or_default().push((item, now));
    }

    /// Number of free instances.
    pub fn len(&self) -> usize {
        self.free.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Takes all instances that have been free for at least `max_idle` (the caller releases them
    /// outside the lock).
    pub fn take_idle(&mut self, max_idle: Duration, now: Instant) -> Vec<T> {
        let mut out = Vec::new();
        for v in self.free.values_mut() {
            let (old, keep): (Vec<_>, Vec<_>) =
                std::mem::take(v).into_iter().partition(|(_, since)| now.saturating_duration_since(*since) >= max_idle);
            *v = keep;
            out.extend(old.into_iter().map(|(t, _)| t));
        }
        self.free.retain(|_, v| !v.is_empty());
        out
    }
}

/// Deletes Windows OCR recognizers that have been unused for at least `max_idle`; returns: count.
/// Always 0 without Windows.
pub fn release_idle_all(max_idle: Duration) -> usize {
    #[cfg(windows)]
    {
        imp::release_idle(max_idle)
    }
    #[cfg(not(windows))]
    {
        let _ = max_idle;
        0
    }
}

/// A recognized line in image pixels: line text and words with box `[x, y, w, h]`.
#[derive(Debug, Clone, PartialEq)]
pub struct RawLine {
    pub text: String,
    pub words: Vec<(String, [f32; 4])>,
}

/// Lines of the engine → [`PageOcr`]. `scale`: factor by which the recognized image was
/// downscaled relative to the original (1.0 = original size); `width`/`height`: original size.
pub fn page_from_lines(lines: &[RawLine], width: u32, height: u32, scale: f64) -> PageOcr {
    let inv = if scale > 0.0 { 1.0 / scale } else { 1.0 };
    let mut page = PageOcr { width, height, words: Vec::new(), text: String::new() };
    // line heights for paragraph detection
    let boxes: Vec<Option<(f64, f64)>> = lines
        .iter()
        .map(|l| {
            let top = l.words.iter().map(|w| f64::from(w.1[1])).fold(f64::INFINITY, f64::min);
            let bottom = l.words.iter().map(|w| f64::from(w.1[1] + w.1[3])).fold(f64::NEG_INFINITY, f64::max);
            (top.is_finite() && bottom.is_finite()).then_some((top * inv, bottom * inv))
        })
        .collect();
    let mut heights: Vec<f64> = boxes.iter().flatten().map(|(t, b)| b - t).filter(|h| *h > 0.0).collect();
    heights.sort_by(f64::total_cmp);
    let median = heights.get(heights.len() / 2).copied().unwrap_or(0.0);
    let mut par = 1u32;
    let mut prev: Option<(f64, f64)> = None;
    let mut text = String::new();
    for (i, (line, bx)) in lines.iter().zip(&boxes).enumerate() {
        if line.words.is_empty() && line.text.trim().is_empty() {
            continue;
        }
        // New paragraph: gap to the previous line larger than a line height, or the line is
        // higher than the previous one (new column/block).
        if let (Some((pt, pb)), Some((t, _))) = (prev, *bx) {
            let gap = t - pb;
            if median > 0.0 && (gap > median || t < pt) {
                par += 1;
                text.push('\n');
            }
        }
        let line_text = if line.text.trim().is_empty() {
            line.words.iter().map(|w| w.0.as_str()).collect::<Vec<_>>().join(" ")
        } else {
            line.text.clone()
        };
        text.push_str(line_text.trim_end());
        text.push('\n');
        for (w, r) in &line.words {
            if w.trim().is_empty() {
                continue;
            }
            let x0 = f64::from(r[0]) * inv;
            let y0 = f64::from(r[1]) * inv;
            page.words.push(Word {
                text: w.clone(),
                x0,
                y0,
                x1: x0 + f64::from(r[2]) * inv,
                y1: y0 + f64::from(r[3]) * inv,
                conf: -1.0,
                line: (1, par, u32::try_from(i + 1).unwrap_or(u32::MAX)),
            });
        }
        if bx.is_some() {
            prev = *bx;
        }
    }
    if !text.is_empty() {
        // Tesseract ends the page with a form feed (`page_separator`).
        text.push('\x0c');
    }
    page.text = text;
    page
}

/// Raster (1 or 3 channels) as 8-bit grayscale (Pillow `convert("L")`: 299/587/114).
pub fn to_gray(channels: u8, data: &[u8]) -> Vec<u8> {
    match channels {
        1 => data.to_vec(),
        3 => data.as_chunks::<3>().0.iter().map(luma).collect(),
        4 => data.as_chunks::<4>().0.iter().map(|p| luma(&[p[0], p[1], p[2]])).collect(),
        _ => data.to_vec(),
    }
}

fn luma(p: &[u8; 3]) -> u8 {
    ((u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114 + 500) / 1000) as u8
}

/// Downscale factor (≤ 1) so that the longer side has at most `max_dim` pixels.
pub fn fit_scale(width: u32, height: u32, max_dim: u32) -> f64 {
    let m = width.max(height);
    if max_dim == 0 || m <= max_dim { 1.0 } else { f64::from(max_dim) / f64::from(m) }
}

#[cfg(windows)]
pub use imp::WindowsOcr;

#[cfg(windows)]
mod imp {
    use std::path::Path;
    use std::sync::{Mutex, OnceLock, mpsc};
    use std::time::{Duration, Instant};

    use windows::Globalization::Language;
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::{OcrEngine as WinEngine, OcrResult};
    use windows::Storage::Streams::DataWriter;
    use windows::core::HSTRING;

    use super::{IdlePool, RawLine, fit_scale, page_from_lines, to_gray};
    use crate::engine::{OcrEngine, OcrRequest, Orientation, PageOcr, TesseractCli};
    use crate::engine_select::{LangPlan, plan_languages, tess_for_tag};
    use crate::render::Raster;
    use crate::{OcrError, Result};

    fn werr(what: &str) -> impl Fn(windows::core::Error) -> OcrError + '_ {
        move |e| OcrError::SubprocessOutput(format!("Windows OCR: {what}: {e}"))
    }

    /// `Windows.Media.Ocr` as engine. `osd`: Tesseract for orientation detection (optional).
    pub struct WindowsOcr {
        osd: Option<TesseractCli>,
        max_dim: u32,
    }

    /// Process-wide pool per language tag.
    fn pool() -> &'static Mutex<IdlePool<WinEngine>> {
        static P: OnceLock<Mutex<IdlePool<WinEngine>>> = OnceLock::new();
        P.get_or_init(Default::default)
    }

    /// See [`super::release_idle_all`].
    pub(super) fn release_idle(max_idle: Duration) -> usize {
        if POOL_USED.get().is_none() {
            return 0;
        }
        let stale = pool().lock().unwrap_or_else(|e| e.into_inner()).take_idle(max_idle, Instant::now());
        let n = stale.len();
        // Release (COM release) outside the lock.
        drop(stale);
        if n > 0 {
            tracing::debug!("windows-ocr: released {n} idle recognizer(s)");
        }
        n
    }

    /// Set as soon as the pool has been used once (otherwise the cleanup thread has nothing to do).
    static POOL_USED: OnceLock<()> = OnceLock::new();

    impl WindowsOcr {
        /// Checks whether the WinRT class is present (`AvailableRecognizerLanguages`), otherwise
        /// [`OcrError::Unavailable`].
        pub fn new(osd: Option<TesseractCli>) -> Result<Self> {
            let max_dim =
                WinEngine::MaxImageDimension().map_err(|e| OcrError::Unavailable(format!("Windows.Media.Ocr not available: {e}")))?;
            WinEngine::AvailableRecognizerLanguages()
                .map_err(|e| OcrError::Unavailable(format!("Windows.Media.Ocr not available: {e}")))?;
            Ok(Self { osd: osd.filter(TesseractCli::is_available), max_dim })
        }

        /// Installed OCR language packs as BCP-47 tags (`de-DE`, …).
        pub fn installed_tags() -> Result<Vec<String>> {
            let langs = WinEngine::AvailableRecognizerLanguages().map_err(werr("languages"))?;
            let n = langs.Size().map_err(werr("languages"))?;
            (0..n).map(|i| langs.GetAt(i).and_then(|l| l.LanguageTag()).map(|t| t.to_string()).map_err(werr("languages"))).collect()
        }

        fn plan(&self, languages: &str) -> Result<LangPlan> {
            let installed = Self::installed_tags()?;
            plan_languages(languages, &installed).map_err(|m| OcrError::Other(format!("MissingDependencyError: {m}")))
        }

        fn take_engine(tag: &str) -> Result<WinEngine> {
            if let Some(e) = pool().lock().unwrap_or_else(|e| e.into_inner()).take(tag) {
                return Ok(e);
            }
            let lang = Language::CreateLanguage(&HSTRING::from(tag)).map_err(werr("language"))?;
            WinEngine::TryCreateFromLanguage(&lang)
                .map_err(|e| OcrError::Other(format!("MissingDependencyError: Windows OCR could not create a recognizer for {tag}: {e}")))
        }

        fn give_back(tag: &str, e: WinEngine) {
            pool().lock().unwrap_or_else(|e| e.into_inner()).give_back(tag, e, Instant::now());
            POOL_USED.get_or_init(|| ());
            crate::tesslib::start_reaper();
        }

        /// Recognition of a raster with a fixed language tag (without language check).
        pub fn recognize_with_tag(&self, raster: &Raster, tag: &str, timeout: Option<Duration>) -> Result<PageOcr> {
            let scale = fit_scale(raster.width, raster.height, self.max_dim);
            let mut gray = to_gray(raster.channels, &raster.data);
            let (mut w, mut h) = (raster.width, raster.height);
            if scale < 1.0 {
                let img =
                    image::GrayImage::from_raw(w, h, gray).ok_or_else(|| OcrError::Other("raster size does not match its data".into()))?;
                w = ((f64::from(w) * scale).floor() as u32).max(1);
                h = ((f64::from(h) * scale).floor() as u32).max(1);
                gray = image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle).into_raw();
            }
            let writer = DataWriter::new().map_err(werr("buffer"))?;
            writer.WriteBytes(&gray).map_err(werr("buffer"))?;
            let buffer = writer.DetachBuffer().map_err(werr("buffer"))?;
            let to_i32 = |v: u32| i32::try_from(v).map_err(|_| OcrError::Other("image too large".into()));
            let bitmap =
                SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Gray8, to_i32(w)?, to_i32(h)?).map_err(werr("bitmap"))?;
            let engine = Self::take_engine(tag)?;
            let result = run(&engine, &bitmap, timeout);
            // On timeout the recognition may still be running: do not reuse the instance.
            if !matches!(result, Err(OcrError::SubprocessOutput(ref m)) if m.contains("timed out")) {
                Self::give_back(tag, engine);
            }
            let lines = convert(&result?)?;
            Ok(page_from_lines(&lines, raster.width, raster.height, scale))
        }
    }

    fn run(engine: &WinEngine, bitmap: &SoftwareBitmap, timeout: Option<Duration>) -> Result<OcrResult> {
        let op = engine.RecognizeAsync(bitmap).map_err(werr("recognize"))?;
        let (tx, rx) = mpsc::channel();
        op.when(move |r| {
            let _ = tx.send(r);
        })
        .map_err(werr("recognize"))?;
        let r = match timeout {
            Some(t) => match rx.recv_timeout(t) {
                Ok(r) => r,
                Err(_) => {
                    let _ = op.Cancel();
                    return Err(OcrError::SubprocessOutput("Windows OCR timed out".into()));
                }
            },
            None => rx.recv().map_err(|_| OcrError::SubprocessOutput("Windows OCR: no result".into()))?,
        };
        r.map_err(werr("recognize"))
    }

    fn convert(r: &OcrResult) -> Result<Vec<RawLine>> {
        let e = werr("result");
        let lines = r.Lines().map_err(&e)?;
        let n = lines.Size().map_err(&e)?;
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            let line = lines.GetAt(i).map_err(&e)?;
            let words = line.Words().map_err(&e)?;
            let m = words.Size().map_err(&e)?;
            let mut ws = Vec::with_capacity(m as usize);
            for j in 0..m {
                let w = words.GetAt(j).map_err(&e)?;
                let b = w.BoundingRect().map_err(&e)?;
                ws.push((w.Text().map_err(&e)?.to_string(), [b.X, b.Y, b.Width, b.Height]));
            }
            out.push(RawLine { text: line.Text().map_err(&e)?.to_string(), words: ws });
        }
        Ok(out)
    }

    impl OcrEngine for WindowsOcr {
        fn name(&self) -> String {
            let tags = Self::installed_tags().map(|t| t.join(",")).unwrap_or_default();
            format!("windows-ocr (Windows.Media.Ocr, languages {tags})")
        }

        fn recognize(&self, image: &Path, req: &OcrRequest) -> Result<PageOcr> {
            let img = image::open(image).map_err(|e| OcrError::InputFile(format!("{}: {e}", image.display())))?;
            let raster = Raster::from_dynamic(&img, f64::from(req.dpi.unwrap_or(0)));
            let plan = self.plan(&req.languages)?;
            self.recognize_with_tag(&raster, &plan.tag, req.timeout)
        }

        fn orientation(&self, image: &Path, timeout: Option<Duration>) -> Result<Option<Orientation>> {
            match &self.osd {
                Some(t) => t.orientation(image, timeout),
                None => Ok(None),
            }
        }

        fn languages(&self) -> Result<Vec<String>> {
            let mut out: Vec<String> = Vec::new();
            for t in Self::installed_tags()? {
                if let Some(c) = tess_for_tag(&t)
                    && !out.iter().any(|o| o == c)
                {
                    out.push(c.to_string());
                }
            }
            Ok(out)
        }

        fn check_languages(&self, languages: &str) -> Result<()> {
            let plan = self.plan(languages)?;
            if !plan.substituted.is_empty() {
                tracing::warn!(
                    "Windows OCR: no language pack for {}; recognized with {} (same script). {}",
                    plan.substituted.join(", "),
                    plan.tag,
                    crate::engine_select::install_hint(&plan.substituted[0])
                );
            }
            Ok(())
        }

        fn raster_file_ext(&self) -> &'static str {
            // For Tesseract OSD via the program only.
            "tif"
        }

        fn recognize_raster(&self, raster: &Raster, _file: &Path, req: &OcrRequest) -> Result<PageOcr> {
            let plan = self.plan(&req.languages)?;
            self.recognize_with_tag(raster, &plan.tag, req.timeout)
        }

        fn orientation_raster(&self, raster: &Raster, file: &Path, timeout: Option<Duration>) -> Result<Option<Orientation>> {
            match &self.osd {
                Some(t) => t.orientation_raster(raster, file, timeout),
                None => {
                    static WARNED: std::sync::Once = std::sync::Once::new();
                    WARNED.call_once(|| {
                        tracing::warn!(
                            "Windows OCR cannot detect page orientation and tesseract (OSD) is not available; pages are not rotated"
                        );
                    });
                    Ok(None)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, words: &[(&str, [f32; 4])]) -> RawLine {
        RawLine { text: text.into(), words: words.iter().map(|(t, r)| (t.to_string(), *r)).collect() }
    }

    #[test]
    fn words_boxes_lines_and_text() {
        let lines = vec![
            line("Hallo Welt", &[("Hallo", [10.0, 10.0, 50.0, 20.0]), ("Welt", [70.0, 12.0, 40.0, 18.0])]),
            line("zweite Zeile", &[("zweite", [10.0, 35.0, 60.0, 20.0]), ("Zeile", [80.0, 35.0, 50.0, 20.0])]),
            // large gap → new paragraph
            line("Absatz", &[("Absatz", [10.0, 120.0, 60.0, 20.0])]),
        ];
        let p = page_from_lines(&lines, 800, 600, 1.0);
        assert_eq!((p.width, p.height), (800, 600));
        assert_eq!(p.words.len(), 5);
        assert_eq!(p.words[1].text, "Welt");
        assert_eq!((p.words[1].x0, p.words[1].y0, p.words[1].x1, p.words[1].y1), (70.0, 12.0, 110.0, 30.0));
        assert_eq!(p.words[0].conf, -1.0);
        let ls = p.lines();
        assert_eq!(ls.len(), 3);
        assert_eq!(ls[0].len(), 2);
        assert_eq!(ls[0][0].line, (1, 1, 1));
        assert_eq!(ls[1][0].line, (1, 1, 2));
        assert_eq!(ls[2][0].line, (1, 2, 3));
        assert_eq!(p.text, "Hallo Welt\nzweite Zeile\n\nAbsatz\n\x0c");
        assert_eq!(crate::text::post_process_text(Some(&p.text)).unwrap(), "Hallo Welt\nzweite Zeile\n\nAbsatz");
    }

    #[test]
    fn scaled_boxes_back_to_original() {
        let lines = vec![line("A", &[("A", [5.0, 10.0, 20.0, 30.0])])];
        let p = page_from_lines(&lines, 20000, 10000, 0.5);
        let w = &p.words[0];
        assert_eq!((w.x0, w.y0, w.x1, w.y1), (10.0, 20.0, 50.0, 80.0));
    }

    #[test]
    fn column_jump_starts_paragraph_and_empty_page() {
        let lines = vec![
            line("links", &[("links", [10.0, 100.0, 50.0, 20.0])]),
            line("rechts", &[("rechts", [400.0, 10.0, 50.0, 20.0])]),
            line("", &[]),
        ];
        let p = page_from_lines(&lines, 800, 600, 1.0);
        assert_eq!(p.text, "links\n\nrechts\n\x0c");
        assert_eq!(p.words[1].line.1, 2);
        let empty = page_from_lines(&[], 10, 10, 1.0);
        assert!(empty.words.is_empty() && empty.text.is_empty());
    }

    #[test]
    fn idle_pool_releases_only_old_instances() {
        let t0 = Instant::now();
        let mut p: IdlePool<u32> = IdlePool::default();
        p.give_back("de-DE", 1, t0);
        p.give_back("de-DE", 2, t0 + Duration::from_secs(100));
        p.give_back("en-US", 3, t0 + Duration::from_secs(10));
        assert_eq!(p.len(), 3);
        // After 150 s from t0: instance 1 (150 s) and 3 (140 s) are free for ≥ 120 s, 2 only 50 s.
        let mut old = p.take_idle(Duration::from_secs(120), t0 + Duration::from_secs(150));
        old.sort_unstable();
        assert_eq!(old, vec![1, 3]);
        assert_eq!(p.len(), 1);
        assert_eq!(p.take("en-US"), None);
        assert_eq!(p.take("de-DE"), Some(2));
        assert!(p.is_empty());
    }

    #[test]
    fn idle_pool_take_is_lifo_and_release_without_pool_is_zero() {
        let t0 = Instant::now();
        let mut p: IdlePool<u32> = IdlePool::default();
        p.give_back("x", 1, t0);
        p.give_back("x", 2, t0);
        assert_eq!(p.take("x"), Some(2));
        assert!(p.take_idle(Duration::from_secs(1), t0).is_empty());
        assert_eq!(release_idle_all(Duration::from_secs(3600)), 0);
    }

    #[test]
    fn gray_and_scale() {
        assert_eq!(to_gray(1, &[1, 2, 3]), vec![1, 2, 3]);
        assert_eq!(to_gray(3, &[255, 255, 255, 0, 0, 0, 255, 0, 0]), vec![255, 0, 76]);
        assert_eq!(fit_scale(1000, 2000, 10000), 1.0);
        assert_eq!(fit_scale(20000, 5000, 10000), 0.5);
        assert_eq!(fit_scale(5, 5, 0), 1.0);
    }
}
