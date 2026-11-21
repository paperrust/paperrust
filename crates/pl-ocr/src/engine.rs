//! OCR engine: page/image → text + word boxes.
//!
//! [`TesseractCli`]: the `tesseract` program (TSV + TXT output, OSD for page rotation).
//! For rasters ([`OcrEngine::recognize_raster`]/[`OcrEngine::orientation_raster`]) it uses, if
//! available, `libtesseract` of the same version in-process ([`crate::tesslib`]: no process start,
//! traineddata stays loaded, same output); otherwise the program with an uncompressed
//! TIFF file (Leptonica reads it faster than PNM, same pixels). The rest of the crate only knows
//! [`OcrEngine`].
//!
//! Model: OCRmyPDF `ocrmypdf/_exec/tesseract.py` (`generate_hocr`, `get_orientation`,
//! `get_languages`), environment `OMP_THREAD_LIMIT=1` like `RasterisedDocumentParser.parse`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::render::Raster;
use crate::tesslib::{self, TessLib};
use crate::{OcrError, Result};

/// A recognized word. Coordinates in pixels of the input image, origin top left.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    pub conf: f32,
    /// (block, paragraph, line) – words with the same identifier form a line.
    pub line: (u32, u32, u32),
}

/// OCR result of a page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PageOcr {
    pub width: u32,
    pub height: u32,
    pub words: Vec<Word>,
    /// Plain text of the engine (for the sidecar file; like Tesseract's `txt` output).
    pub text: String,
}

impl PageOcr {
    /// Words grouped by line (order of the engine).
    pub fn lines(&self) -> Vec<Vec<&Word>> {
        let mut out: Vec<Vec<&Word>> = Vec::new();
        let mut last: Option<(u32, u32, u32)> = None;
        for w in &self.words {
            if last != Some(w.line) {
                out.push(Vec::new());
                last = Some(w.line);
            }
            if let Some(l) = out.last_mut() {
                l.push(w);
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct OcrRequest {
    /// Tesseract languages, e.g. `deu+eng`.
    pub languages: String,
    /// Resolution of the image (for Tesseract `--dpi`).
    pub dpi: Option<u32>,
    pub timeout: Option<Duration>,
    /// OCRmyPDF Tesseract options from `OCR_USER_ARGS` (Tesseract only).
    pub tess: TessOptions,
}

/// OCRmyPDF `tesseract_pagesegmode`, `tesseract_oem`, `tesseract_thresholding`, `tesseract_config`,
/// `user_words`, `user_patterns`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TessOptions {
    pub psm: Option<u8>,
    pub oem: Option<u8>,
    /// `-c thresholding_method=N` (0 Otsu, 1 adaptive Otsu, 2 Sauvola).
    pub thresholding: Option<u8>,
    /// Additional Tesseract configuration files (`configfile …` at the end of the command line).
    pub config: Vec<String>,
    pub user_words: Option<std::path::PathBuf>,
    pub user_patterns: Option<std::path::PathBuf>,
}

impl TessOptions {
    /// Options that only the Tesseract program implements (the library instances are reused per
    /// language/psm and do not know them) → program path.
    pub fn needs_cli(&self) -> bool {
        self.oem.is_some()
            || self.thresholding.is_some()
            || !self.config.is_empty()
            || self.user_words.is_some()
            || self.user_patterns.is_some()
    }
    pub fn is_default(&self) -> bool {
        *self == TessOptions::default()
    }
}

/// Result of orientation detection (`--psm 0`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orientation {
    /// Clockwise rotation that straightens the image (0/90/180/270).
    pub rotate: u16,
    pub confidence: f64,
}

pub trait OcrEngine: Send + Sync {
    fn name(&self) -> String;
    /// Text + word boxes of an image (PNG/PNM/TIFF/JPEG, depending on the engine).
    fn recognize(&self, image: &Path, req: &OcrRequest) -> Result<PageOcr>;
    /// Orientation (for `rotate_pages`); `None` if it cannot be determined.
    fn orientation(&self, image: &Path, timeout: Option<Duration>) -> Result<Option<Orientation>>;
    /// Installed languages.
    fn languages(&self) -> Result<Vec<String>>;
    /// Checks before the first page whether the engine can recognize `languages` (Tesseract codes, `deu+eng`).
    /// Default: all in [`Self::languages`] (OCRmyPDF `check_options_languages`); engines with
    /// their own language mapping (e.g. [`crate::winocr`]) override this.
    fn check_languages(&self, languages: &str) -> Result<()> {
        check_installed(&self.languages()?, languages)
    }

    /// File extension for [`Self::recognize_raster`]/[`Self::orientation_raster`], if the engine
    /// needs an image file (default PNM, see [`write_raster`]).
    fn raster_file_ext(&self) -> &'static str {
        "pnm"
    }
    /// Recognize a raster. `file`: location for an image file (extension [`Self::raster_file_ext`]); it is
    /// written if it does not exist yet – the caller deletes it (also between rotations).
    fn recognize_raster(&self, raster: &Raster, file: &Path, req: &OcrRequest) -> Result<PageOcr> {
        ensure_raster_file(raster, file)?;
        self.recognize(file, req)
    }
    /// Orientation of a raster (file as with [`Self::recognize_raster`]).
    fn orientation_raster(&self, raster: &Raster, file: &Path, timeout: Option<Duration>) -> Result<Option<Orientation>> {
        ensure_raster_file(raster, file)?;
        self.orientation(file, timeout)
    }
}

/// Write a raster as an image file; format by extension: `tif`/`tiff` (uncompressed, without
/// resolution – like PNM, see [`Raster::write_tiff`]), otherwise PNM.
pub fn write_raster(raster: &Raster, file: &Path) -> Result<()> {
    match file.extension().and_then(|e| e.to_str()) {
        Some("tif" | "tiff") => raster.write_tiff(file),
        _ => raster.write_pnm(file),
    }
}

fn ensure_raster_file(raster: &Raster, file: &Path) -> Result<()> {
    if file.exists() { Ok(()) } else { write_raster(raster, file) }
}

/// `tesseract`-CLI.
#[derive(Debug, Clone)]
pub struct TesseractCli {
    pub program: PathBuf,
    /// Optional `--tessdata-dir`.
    pub tessdata_dir: Option<PathBuf>,
    /// `--psm` (OCRmyPDF-Standard: Tesseract-Standard 3).
    pub psm: Option<u8>,
}

impl TesseractCli {
    /// Path from `PAPERRUST_TESSERACT_PATH`, otherwise `tesseract` in `PATH`;
    /// `PAPERRUST_TESSDATA_DIR` optional.
    pub fn from_env() -> Self {
        Self {
            program: std::env::var_os("PAPERRUST_TESSERACT_PATH").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("tesseract")),
            tessdata_dir: std::env::var_os("PAPERRUST_TESSDATA_DIR").map(PathBuf::from),
            psm: None,
        }
    }

    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self { program: program.into(), tessdata_dir: None, psm: None }
    }

    /// Version (`tesseract --version`, first line) or `None` if not executable.
    pub fn version(&self) -> Option<String> {
        let out = Command::new(&self.program).arg("--version").stdin(Stdio::null()).output().ok()?;
        if !out.status.success() {
            return None;
        }
        // Older versions write to stderr.
        let s = if out.stdout.is_empty() { out.stderr } else { out.stdout };
        String::from_utf8_lossy(&s).lines().next().map(|l| l.trim().to_string())
    }

    pub fn is_available(&self) -> bool {
        self.version().is_some()
    }

    /// `libtesseract` of the same version as the program (see [`crate::tesslib`]), searched once per
    /// program path and cached process-wide; `None` → command line.
    pub fn library(&self) -> Option<Arc<TessLib>> {
        static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<Arc<TessLib>>>>> = OnceLock::new();
        let m = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        g.entry(self.program.clone())
            .or_insert_with(|| tesslib::resolve(&tesslib::lib_setting(), &self.program, self.version().as_deref()))
            .clone()
    }

    /// tessdata directory for the library. Like the CLI: `--tessdata-dir`, otherwise
    /// `TESSDATA_PREFIX`/built-in path – only on Windows does `tesseract.exe` look next to its
    /// *own* EXE (`<program>/tessdata`), whereas the library in-process looks next to paperrust.exe;
    /// therefore pass the program's path explicitly there.
    pub fn library_tessdata(&self) -> Option<PathBuf> {
        if let Some(d) = &self.tessdata_dir {
            return Some(d.clone());
        }
        if cfg!(windows) && std::env::var_os("TESSDATA_PREFIX").is_none() {
            let exe = if self.program.components().count() > 1 {
                Some(self.program.clone())
            } else {
                std::env::var_os("PATH").and_then(|p| {
                    std::env::split_paths(&p).find_map(|d| {
                        [d.join(&self.program), d.join(format!("{}.exe", self.program.display()))].into_iter().find(|p| p.is_file())
                    })
                })
            };
            return exe.and_then(|e| e.parent().map(|d| d.join("tessdata")));
        }
        None
    }

    /// Recognition always via the program (image file `file` is written if it is missing).
    pub fn recognize_raster_cli(&self, raster: &Raster, file: &Path, req: &OcrRequest) -> Result<PageOcr> {
        ensure_raster_file(raster, file)?;
        self.recognize(file, req)
    }

    /// Orientation always via the program.
    pub fn orientation_raster_cli(&self, raster: &Raster, file: &Path, timeout: Option<Duration>) -> Result<Option<Orientation>> {
        ensure_raster_file(raster, file)?;
        self.orientation(file, timeout)
    }

    fn command(&self) -> Command {
        let mut c = Command::new(&self.program);
        c.env("OMP_THREAD_LIMIT", "1").stdin(Stdio::null());
        if let Some(d) = &self.tessdata_dir {
            c.arg("--tessdata-dir").arg(d);
        }
        c
    }
}

/// Starts `cmd`, waits at most `timeout`; errors as `SubprocessOutputError`.
pub fn run_with_timeout(mut cmd: Command, timeout: Option<Duration>) -> Result<std::process::Output> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| OcrError::Unavailable(format!("could not start {:?}: {e}", cmd.get_program())))?;
    let start = Instant::now();
    // read stdout/stderr in threads so that full pipes do not block.
    let mut so = child.stdout.take();
    let mut se = child.stderr.take();
    let t_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(s) = so.as_mut() {
            let _ = std::io::Read::read_to_end(s, &mut b);
        }
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(s) = se.as_mut() {
            let _ = std::io::Read::read_to_end(s, &mut b);
        }
        b
    });
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if timeout.is_some_and(|t| start.elapsed() > t) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(OcrError::SubprocessOutput(format!("{:?} timed out", cmd.get_program())));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = t_out.join().unwrap_or_default();
    let stderr = t_err.join().unwrap_or_default();
    Ok(std::process::Output { status, stdout, stderr })
}

impl OcrEngine for TesseractCli {
    fn name(&self) -> String {
        match self.library() {
            Some(lib) => format!("tesseract-lib ({}, {})", lib.path().display(), lib.version()),
            None => format!("tesseract-cli ({})", self.program.display()),
        }
    }

    fn recognize(&self, image: &Path, req: &OcrRequest) -> Result<PageOcr> {
        let dir = image.parent().unwrap_or(Path::new("."));
        let stem = image.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "page".into());
        let base = dir.join(format!("{stem}_ocr"));
        let mut c = self.command();
        c.arg(image).arg(&base).arg("-l").arg(&req.languages);
        if let Some(d) = req.dpi {
            c.arg("--dpi").arg(d.to_string());
        }
        if let Some(p) = req.tess.psm.or(self.psm) {
            c.arg("--psm").arg(p.to_string());
        }
        if let Some(o) = req.tess.oem {
            c.arg("--oem").arg(o.to_string());
        }
        if let Some(t) = req.tess.thresholding {
            c.arg("-c").arg(format!("thresholding_method={t}"));
        }
        if let Some(w) = &req.tess.user_words {
            c.arg("--user-words").arg(w);
        }
        if let Some(w) = &req.tess.user_patterns {
            c.arg("--user-patterns").arg(w);
        }
        // outputs via variables instead of config files (tessdata/configs often missing).
        c.arg("-c").arg("tessedit_create_tsv=1").arg("-c").arg("tessedit_create_txt=1");
        for cfg in &req.tess.config {
            c.arg(cfg);
        }
        let out = run_with_timeout(c, req.timeout)?;
        if !out.status.success() {
            return Err(OcrError::SubprocessOutput(format!(
                "tesseract failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let tsv_path = base.with_extension("tsv");
        let txt_path = base.with_extension("txt");
        let tsv = std::fs::read(&tsv_path)?;
        let txt = std::fs::read(&txt_path).unwrap_or_default();
        let _ = std::fs::remove_file(&tsv_path);
        let _ = std::fs::remove_file(&txt_path);
        let mut page = parse_tsv(&String::from_utf8_lossy(&tsv));
        page.text = String::from_utf8_lossy(&txt).into_owned();
        Ok(page)
    }

    fn orientation(&self, image: &Path, timeout: Option<Duration>) -> Result<Option<Orientation>> {
        let mut c = self.command();
        // OCRmyPDF `get_orientation`: `-l osd --psm 0` (with psm 0 Tesseract only loads osd anyway).
        c.arg(image).arg("stdout").arg("-l").arg("osd").arg("--psm").arg("0");
        let out = run_with_timeout(c, timeout)?;
        if !out.status.success() {
            // OCRmyPDF: "Too few characters" or similar → no rotation
            tracing::debug!("tesseract OSD failed: {}", String::from_utf8_lossy(&out.stderr).trim());
            return Ok(None);
        }
        Ok(parse_osd(&String::from_utf8_lossy(&out.stdout)))
    }

    fn raster_file_ext(&self) -> &'static str {
        // For the program path only; Leptonica reads uncompressed TIFF faster than PNM.
        "tif"
    }

    fn recognize_raster(&self, raster: &Raster, file: &Path, req: &OcrRequest) -> Result<PageOcr> {
        match self.library() {
            Some(lib) if !req.tess.needs_cli() => lib.recognize(self.library_tessdata().as_deref(), req.tess.psm.or(self.psm), raster, req),
            _ => self.recognize_raster_cli(raster, file, req),
        }
    }

    fn orientation_raster(&self, raster: &Raster, file: &Path, timeout: Option<Duration>) -> Result<Option<Orientation>> {
        match self.library() {
            Some(lib) => lib.orientation(self.library_tessdata().as_deref(), raster, timeout),
            None => self.orientation_raster_cli(raster, file, timeout),
        }
    }

    fn languages(&self) -> Result<Vec<String>> {
        let mut c = self.command();
        c.arg("--list-langs");
        let out = run_with_timeout(c, Some(Duration::from_secs(30)))?;
        let s = if out.stdout.is_empty() { out.stderr } else { out.stdout };
        Ok(String::from_utf8_lossy(&s)
            .lines()
            .skip(1) // "List of available languages in ... (N):"
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect())
    }
}

/// Tesseract-TSV (`level page block par line word left top width height conf text`).
pub fn parse_tsv(tsv: &str) -> PageOcr {
    let mut page = PageOcr::default();
    for (i, line) in tsv.lines().enumerate() {
        if i == 0 && line.starts_with("level") {
            continue;
        }
        let f: Vec<&str> = line.splitn(12, '\t').collect();
        if f.len() < 11 {
            continue;
        }
        let num = |k: usize| f.get(k).and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(0.0);
        let level = num(0) as u32;
        let (l, t, w, h) = (num(6), num(7), num(8), num(9));
        if level == 1 {
            page.width = w as u32;
            page.height = h as u32;
            continue;
        }
        if level != 5 {
            continue;
        }
        let text = f.get(11).map(|s| s.trim_end_matches(['\r', '\n'])).unwrap_or("");
        if text.trim().is_empty() {
            continue;
        }
        page.words.push(Word {
            text: text.to_string(),
            x0: l,
            y0: t,
            x1: l + w,
            y1: t + h,
            conf: num(10) as f32,
            line: (num(2) as u32, num(3) as u32, num(4) as u32),
        });
    }
    page
}

/// Output of `tesseract … --psm 0`.
pub fn parse_osd(s: &str) -> Option<Orientation> {
    let mut rotate = None;
    let mut conf = None;
    for line in s.lines() {
        if let Some(v) = line.strip_prefix("Rotate:") {
            rotate = v.trim().parse::<u16>().ok();
        } else if let Some(v) = line.strip_prefix("Orientation confidence:") {
            conf = v.trim().parse::<f64>().ok();
        }
    }
    Some(Orientation { rotate: rotate? % 360, confidence: conf? })
}

/// Checks whether all languages (`deu+eng`) are installed (OCRmyPDF `check_options_languages`).
pub fn check_languages(engine: &dyn OcrEngine, languages: &str) -> Result<()> {
    engine.check_languages(languages)
}

/// Default check: every requested language is in `have`.
pub fn check_installed(have: &[String], languages: &str) -> Result<()> {
    let missing: Vec<&str> = languages.split('+').filter(|l| !l.is_empty() && !have.iter().any(|h| h == l)).collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(OcrError::Other(format!(
            "MissingDependencyError: OCR engine does not have language data for the following requested languages: {}",
            missing.join(", ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsv() {
        let s = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
1\t1\t0\t0\t0\t0\t0\t0\t800\t600\t-1\t\n\
4\t1\t1\t1\t1\t0\t10\t10\t200\t20\t-1\t\n\
5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\tHello\n\
5\t1\t1\t1\t1\t2\t70\t10\t60\t20\t95\tWorld\n\
5\t1\t1\t1\t2\t1\t10\t40\t60\t20\t91\tnext\n\
5\t1\t1\t1\t2\t2\t10\t40\t60\t20\t-1\t \n";
        let p = parse_tsv(s);
        assert_eq!((p.width, p.height), (800, 600));
        assert_eq!(p.words.len(), 3);
        assert_eq!(p.words[1].x1, 130.0);
        let lines = p.lines();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), 2);
    }

    #[test]
    fn osd() {
        let s = "Page number: 0\nOrientation in degrees: 270\nRotate: 90\nOrientation confidence: 7.65\nScript: Latin\nScript confidence: 2.33\n";
        assert_eq!(parse_osd(s), Some(Orientation { rotate: 90, confidence: 7.65 }));
        assert_eq!(parse_osd("garbage"), None);
    }
}
