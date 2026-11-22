//! Tesseract as a library (C API of `libtesseract`, loaded dynamically like PDFium in
//! [`crate::render`]) – saves two process starts per page including loading the `traineddata` and
//! reading of the image file by Leptonica.
//!
//! Result equality with the command-line program: the calls reproduce what
//! `tesseract <image> <base> -l <languages> [--dpi N] -c tessedit_create_tsv=1 -c tessedit_create_txt=1`
//! or `tesseract <image> stdout -l osd --psm 0` do internally (Tesseract 5, `tesseract.cpp`,
//! `TessBaseAPI::ProcessPage`, `TessTsvRenderer`/`TessTextRenderer`, `TessOsdRenderer`/`GetOsdText`):
//!
//! - Init with the same variables (`-c …` are init variables), page mode `PSM_AUTO` (or `--psm`),
//!   `--dpi` = variable `user_defined_dpi`.
//! - Recognition: `SetImage` (same pixels as the PNM/TIFF file, resolution 0 as there) →
//!   `Recognize` → `GetTsvText(0)` + `GetUTF8Text`.
//! - OSD: `SetImage` → `AnalyseLayout` (= `FindLines`, like `ProcessPage` in mode `PSM_OSD_ONLY`; removes
//!   lines in the binary image) → `DetectOrientationScript`; "Rotate" and "Orientation confidence" like
//!   `GetOsdText` (`(360 - degrees) % 360`, confidence to two decimal places).
//!
//! Instances are kept per (languages, page mode) in a process-wide pool – at most as
//! many as pages are recognized simultaneously (pipeline workers). After every page `Clear` +
//! `ClearAdaptiveClassifier`, so that no page influences the next (like a new process).
//!
//! **Release when idle** ([`idle_release_after`], `PAPERRUST_OCR_ENGINE_IDLE_SECS`, default 180 s):
//! Every instance holds the loaded language models (LSTM network, dictionaries; with deu+eng with
//! legacy data tens of MiB per instance). With 4 page workers plus OSD, ~850 MiB stayed resident after the first ingestion
//! on the Jetson (benchmark 2026-11-18, `results/benchmark/2026-11-18-jetson`). A
//! cleanup thread ([`start_reaper`]) therefore deletes instances that have been unused longer than the deadline
//! in the pool, and then returns the memory to the operating system ([`release_to_os`]).
//! During a batch an instance is reused every few seconds and is kept;
//! after a pause, reloading costs ~0.1–0.3 s once per instance.
//!
//! Location ([`find_library`]): `PAPERRUST_TESSERACT_LIB` (file or directory; `off`/`0` switches off),
//! otherwise next to the `tesseract` program (`libtesseract*.dll`, `libtesseract.so.5`, `../lib/…`), otherwise
//! (not Windows) the system name `libtesseract.so.5`. The library is used only if its
//! version (`TessVersion`) equals that of the program (`tesseract --version`).

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_float, c_int, c_uchar, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::engine::{OcrRequest, Orientation, PageOcr, parse_tsv};
use crate::render::Raster;
use crate::{OcrError, Result};

type Handle = *mut c_void;

/// `TessOcrEngineMode::OEM_DEFAULT` (default of the CLI).
const OEM_DEFAULT: c_int = 3;
/// `PSM_OSD_ONLY`.
const PSM_OSD_ONLY: c_int = 0;
/// `PSM_AUTO` (default of the CLI, `tesseract.cpp`).
const PSM_AUTO: c_int = 3;

struct Api {
    version: unsafe extern "C" fn() -> *const c_char,
    create: unsafe extern "C" fn() -> Handle,
    delete: unsafe extern "C" fn(Handle),
    #[allow(clippy::type_complexity)]
    init4: unsafe extern "C" fn(
        Handle,
        *const c_char,
        *const c_char,
        c_int,
        *mut *mut c_char,
        c_int,
        *mut *mut c_char,
        *mut *mut c_char,
        usize,
        c_int,
    ) -> c_int,
    set_psm: unsafe extern "C" fn(Handle, c_int),
    set_variable: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    set_image: unsafe extern "C" fn(Handle, *const c_uchar, c_int, c_int, c_int, c_int),
    recognize: unsafe extern "C" fn(Handle, *mut c_void) -> c_int,
    tsv_text: unsafe extern "C" fn(Handle, c_int) -> *mut c_char,
    utf8_text: unsafe extern "C" fn(Handle) -> *mut c_char,
    delete_text: unsafe extern "C" fn(*const c_char),
    analyse_layout: unsafe extern "C" fn(Handle) -> *mut c_void,
    page_iterator_delete: unsafe extern "C" fn(*mut c_void),
    detect_os: unsafe extern "C" fn(Handle, *mut c_int, *mut c_float, *mut *const c_char, *mut c_float) -> c_int,
    clear: unsafe extern "C" fn(Handle),
    clear_adaptive: unsafe extern "C" fn(Handle),
    monitor_create: unsafe extern "C" fn() -> *mut c_void,
    monitor_delete: unsafe extern "C" fn(*mut c_void),
    monitor_deadline: unsafe extern "C" fn(*mut c_void, c_int),
}

/// Loaded `libtesseract` (function pointers + instance pool).
pub struct TessLib {
    path: PathBuf,
    version: String,
    api: Api,
    pool: Mutex<HashMap<PoolKey, Vec<Idle>>>,
    /// `Init` serialized (global parameters when loading the traineddata); value: `debug_file` already set.
    init_lock: Mutex<bool>,
    /// See [`openmp_set_levels`].
    omp_levels: Option<SetLevels>,
    // Last, so that the library would only be released after `api` (it is never unloaded).
    _lib: libloading::Library,
}

// Function pointers of a thread-safe C library; instances are never used concurrently.
unsafe impl Send for TessLib {}
unsafe impl Sync for TessLib {}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PoolKey {
    tessdata: Option<PathBuf>,
    /// Languages; `osd` for orientation detection.
    lang: String,
    psm: c_int,
}

struct Instance(Handle);
unsafe impl Send for Instance {}

/// Free instance in the pool with the time since which it has been unused.
struct Idle {
    inst: Instance,
    since: Instant,
}

fn sym<T: Copy>(lib: &libloading::Library, name: &str) -> std::result::Result<T, String> {
    // SAFETY: signatures per tesseract/capi.h (5.x).
    unsafe { lib.get::<T>(name.as_bytes()).map(|s| *s).map_err(|e| format!("{name}: {e}")) }
}

fn load_dll(path: &Path) -> std::result::Result<libloading::Library, String> {
    #[cfg(windows)]
    {
        // Load dependent DLLs (Leptonica, libstdc++, …) from the library's directory.
        const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x0000_0008;
        let abs = std::path::absolute(path).map_err(|e| e.to_string())?;
        // SAFETY: loading a library; initialization code of libtesseract/Leptonica.
        unsafe { libloading::os::windows::Library::load_with_flags(&abs, LOAD_WITH_ALTERED_SEARCH_PATH) }
            .map(Into::into)
            .map_err(|e| format!("{}: {e}", path.display()))
    }
    #[cfg(not(windows))]
    {
        // SAFETY: as above.
        unsafe { libloading::Library::new(path) }.map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// `omp_set_max_active_levels` of the OpenMP runtime that libtesseract loads along (e.g. Debian and
/// MSYS2 builds); without OpenMP (UB-Mannheim build) `None`.
///
/// Like `OMP_THREAD_LIMIT=1` with the CLI: `omp_set_max_active_levels(0)` makes all parallel regions
/// inactive. The value is in libgomp (GCC ≥ 11) an ICV **per thread** – therefore
/// [`TessLib::with_instance`] calls it in every worker thread before the Tesseract work (otherwise every
/// LSTM layer starts up to 10 OpenMP threads: measured 1 worker thread 18.9 s instead of 11 s).
type SetLevels = unsafe extern "C" fn(c_int);

fn openmp_set_levels(lib_path: &Path) -> Option<SetLevels> {
    #[cfg(windows)]
    let gomp = lib_path.parent().map(|d| d.join("libgomp-1.dll")).filter(|p| p.is_file()).and_then(|p| load_dll(&p).ok());
    #[cfg(not(windows))]
    let gomp = {
        let _ = lib_path;
        // Only if libtesseract has already loaded it along (GCC libgomp; on FreeBSD the base
        // system's LLVM libomp, which exports the same function).
        OPENMP_RUNTIMES.iter().find_map(|name| {
            // SAFETY: RTLD_NOLOAD loads nothing new.
            unsafe { libloading::os::unix::Library::open(Some(*name), libc_rtld_noload() | 1) }.ok().map(libloading::Library::from)
        })
    };
    let g = gomp?;
    let f = sym::<SetLevels>(&g, "omp_set_max_active_levels").ok();
    // libgomp stays loaded (belongs to libtesseract, never unloaded) – function pointer stays valid.
    std::mem::forget(g);
    f
}

/// OpenMP runtimes that libtesseract may have loaded (probed with `RTLD_NOLOAD`, in this order).
#[cfg(all(not(windows), not(target_os = "freebsd")))]
const OPENMP_RUNTIMES: &[&str] = &["libgomp.so.1"];
#[cfg(target_os = "freebsd")]
const OPENMP_RUNTIMES: &[&str] = &["libomp.so", "libgomp.so.1"];

#[cfg(all(not(windows), target_os = "macos"))]
fn libc_rtld_noload() -> c_int {
    0x10
}
/// FreeBSD `<dlfcn.h>`: `RTLD_NOLOAD` is 0x2000 (0x4 is not a defined flag there).
#[cfg(target_os = "freebsd")]
fn libc_rtld_noload() -> c_int {
    0x2000
}
#[cfg(all(not(windows), not(target_os = "macos"), not(target_os = "freebsd")))]
fn libc_rtld_noload() -> c_int {
    0x4
}

/// `tesseract v5.4.0.20240606` / `tesseract 5.5.4` / `5.5.4` → `5.5.4…` without prefix.
pub fn normalize_version(v: &str) -> String {
    let v = v.trim();
    let v = v.strip_prefix("tesseract").unwrap_or(v).trim();
    v.strip_prefix('v').unwrap_or(v).trim().to_string()
}

impl TessLib {
    /// Load the library and bind the functions.
    pub fn load(path: &Path) -> std::result::Result<Self, String> {
        let lib = load_dll(path)?;
        let api = Api {
            version: sym(&lib, "TessVersion")?,
            create: sym(&lib, "TessBaseAPICreate")?,
            delete: sym(&lib, "TessBaseAPIDelete")?,
            init4: sym(&lib, "TessBaseAPIInit4")?,
            set_psm: sym(&lib, "TessBaseAPISetPageSegMode")?,
            set_variable: sym(&lib, "TessBaseAPISetVariable")?,
            set_image: sym(&lib, "TessBaseAPISetImage")?,
            recognize: sym(&lib, "TessBaseAPIRecognize")?,
            tsv_text: sym(&lib, "TessBaseAPIGetTsvText")?,
            utf8_text: sym(&lib, "TessBaseAPIGetUTF8Text")?,
            delete_text: sym(&lib, "TessDeleteText")?,
            analyse_layout: sym(&lib, "TessBaseAPIAnalyseLayout")?,
            page_iterator_delete: sym(&lib, "TessPageIteratorDelete")?,
            detect_os: sym(&lib, "TessBaseAPIDetectOrientationScript")?,
            clear: sym(&lib, "TessBaseAPIClear")?,
            clear_adaptive: sym(&lib, "TessBaseAPIClearAdaptiveClassifier")?,
            monitor_create: sym(&lib, "TessMonitorCreate")?,
            monitor_delete: sym(&lib, "TessMonitorDelete")?,
            monitor_deadline: sym(&lib, "TessMonitorSetDeadlineMSecs")?,
        };
        // SAFETY: returns a static string.
        let version = unsafe {
            let p = (api.version)();
            if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
        };
        let omp_levels = openmp_set_levels(path);
        if omp_levels.is_some() {
            tracing::debug!("libtesseract uses OpenMP; limiting each worker thread to one OpenMP thread");
        }
        Ok(Self {
            path: path.to_path_buf(),
            version,
            api,
            omp_levels,
            pool: Mutex::new(HashMap::new()),
            init_lock: Mutex::new(false),
            _lib: lib,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `TessVersion()`, z. B. `5.5.4`.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Number of free instances in the pool (for tests/diagnostics).
    pub fn idle_instances(&self) -> usize {
        self.pool.lock().map(|p| p.values().map(Vec::len).sum()).unwrap_or(0)
    }

    /// Deletes free instances that have been unused for at least `max_idle` (measured at `now`);
    /// returns: number of deleted instances. Instances in use are not in the pool and
    /// are never touched.
    pub fn release_idle(&self, max_idle: Duration, now: Instant) -> usize {
        let stale: Vec<Instance> = match self.pool.lock() {
            Ok(mut p) => {
                let mut out = Vec::new();
                for v in p.values_mut() {
                    let (old, keep): (Vec<Idle>, Vec<Idle>) =
                        std::mem::take(v).into_iter().partition(|i| now.saturating_duration_since(i.since) >= max_idle);
                    *v = keep;
                    out.extend(old.into_iter().map(|i| i.inst));
                }
                p.retain(|_, v| !v.is_empty());
                out
            }
            Err(_) => Vec::new(),
        };
        let n = stale.len();
        for inst in stale {
            // SAFETY: instance is no longer in the pool, nobody else holds it.
            unsafe { (self.api.delete)(inst.0) };
        }
        n
    }

    fn new_instance(&self, key: &PoolKey) -> Result<Instance> {
        let mut debug_file_set = self.init_lock.lock().unwrap_or_else(|e| e.into_inner());
        let cstr = |s: &str| CString::new(s).map_err(|_| OcrError::Other(format!("invalid string for tesseract: {s:?}")));
        let datapath = match &key.tessdata {
            Some(d) => Some(cstr(&d.to_string_lossy())?),
            None => None,
        };
        let lang = cstr(&key.lang)?;
        // Like the `-c` variables of the CLI call (recognition only). In addition once per process `debug_file`
        // (global parameter): tprintf outputs ("Estimating resolution …", "Too few characters …") not
        // to the server's stderr; does not influence recognition. On Windows equal to `nul` – `/dev/null`
        // would first rewrite tprintf on every call (from arbitrary threads).
        let mut vars: Vec<(CString, CString)> = Vec::new();
        if !*debug_file_set {
            vars.push((cstr("debug_file")?, cstr(if cfg!(windows) { "nul" } else { "/dev/null" })?));
        }
        if key.psm != PSM_OSD_ONLY {
            vars.push((cstr("tessedit_create_tsv")?, cstr("1")?));
            vars.push((cstr("tessedit_create_txt")?, cstr("1")?));
        }
        let mut names: Vec<*mut c_char> = vars.iter().map(|(n, _)| n.as_ptr().cast_mut()).collect();
        let mut values: Vec<*mut c_char> = vars.iter().map(|(_, v)| v.as_ptr().cast_mut()).collect();
        // SAFETY: C API; pointers live until the end of the call, Init copies the values.
        unsafe {
            let h = (self.api.create)();
            if h.is_null() {
                return Err(OcrError::Unavailable("TessBaseAPICreate failed".into()));
            }
            let rc = (self.api.init4)(
                h,
                datapath.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()),
                lang.as_ptr(),
                OEM_DEFAULT,
                std::ptr::null_mut(),
                0,
                if names.is_empty() { std::ptr::null_mut() } else { names.as_mut_ptr() },
                if values.is_empty() { std::ptr::null_mut() } else { values.as_mut_ptr() },
                names.len(),
                0,
            );
            if rc != 0 {
                (self.api.delete)(h);
                return Err(OcrError::SubprocessOutput(format!(
                    "tesseract library: could not initialize language '{}' (tessdata {:?})",
                    key.lang, key.tessdata
                )));
            }
            (self.api.set_psm)(h, key.psm);
            *debug_file_set = true;
            Ok(Instance(h))
        }
    }

    /// Get an instance (pool or new), run `f`, release the page and put it back.
    fn with_instance<R>(&self, key: &PoolKey, f: impl FnOnce(Handle) -> Result<R>) -> Result<R> {
        let inst = self.pool.lock().ok().and_then(|mut p| p.get_mut(key).and_then(Vec::pop)).map(|i| i.inst);
        let inst = match inst {
            Some(i) => i,
            None => self.new_instance(key)?,
        };
        if let Some(f) = self.omp_levels {
            // SAFETY: OpenMP runtime function without preconditions; acts on the calling thread.
            unsafe { f(0) };
        }
        let r = f(inst.0);
        // SAFETY: instance belongs exclusively to this call.
        unsafe {
            (self.api.clear)(inst.0);
            (self.api.clear_adaptive)(inst.0);
        }
        match self.pool.lock() {
            Ok(mut p) => p.entry(key.clone()).or_default().push(Idle { inst, since: Instant::now() }),
            // SAFETY: poisoned lock – do not put the instance back, release it.
            Err(_) => unsafe { (self.api.delete)(inst.0) },
        }
        start_reaper();
        r
    }

    fn set_image(&self, h: Handle, raster: &Raster) -> Result<()> {
        let ch = c_int::from(raster.channels);
        let (w, hgt) = (c_int::try_from(raster.width), c_int::try_from(raster.height));
        let (Ok(w), Ok(hgt)) = (w, hgt) else { return Err(OcrError::Other("image too large for tesseract".into())) };
        let bpl = w.checked_mul(ch).ok_or_else(|| OcrError::Other("image too large for tesseract".into()))?;
        if !(ch == 1 || ch == 3) || raster.data.len() != (bpl as usize) * (hgt as usize) {
            return Err(OcrError::Other(format!("unexpected raster layout ({} channels, {} bytes)", ch, raster.data.len())));
        }
        // SAFETY: buffer matches width/height/channels; Tesseract copies the pixels.
        unsafe { (self.api.set_image)(h, raster.data.as_ptr(), w, hgt, ch, bpl) };
        Ok(())
    }

    fn take_text(&self, p: *mut c_char) -> Option<String> {
        if p.is_null() {
            return None;
        }
        // SAFETY: null-terminated string allocated by Tesseract; free with TessDeleteText.
        unsafe {
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            (self.api.delete_text)(p);
            Some(s)
        }
    }

    /// Like `tesseract <image> <base> -l <lang> [--dpi N] [--psm P] -c tessedit_create_tsv=1 -c tessedit_create_txt=1`.
    pub fn recognize(&self, tessdata: Option<&Path>, psm: Option<u8>, raster: &Raster, req: &OcrRequest) -> Result<PageOcr> {
        let key =
            PoolKey { tessdata: tessdata.map(Path::to_path_buf), lang: req.languages.clone(), psm: psm.map_or(PSM_AUTO, c_int::from) };
        self.with_instance(&key, |h| {
            let dpi = CString::new(req.dpi.unwrap_or(0).to_string()).unwrap_or_default();
            // SAFETY: valid instance; strings live until the end of the call.
            unsafe { (self.api.set_variable)(h, c"user_defined_dpi".as_ptr(), dpi.as_ptr()) };
            self.set_image(h, raster)?;
            let start = Instant::now();
            // SAFETY: monitor used only during Recognize.
            let rc = unsafe {
                match req.timeout {
                    Some(t) => {
                        let m = (self.api.monitor_create)();
                        (self.api.monitor_deadline)(m, c_int::try_from(t.as_millis()).unwrap_or(c_int::MAX));
                        let rc = (self.api.recognize)(h, m);
                        (self.api.monitor_delete)(m);
                        rc
                    }
                    None => (self.api.recognize)(h, std::ptr::null_mut()),
                }
            };
            if req.timeout.is_some_and(|t| start.elapsed() >= t) {
                return Err(OcrError::SubprocessOutput("tesseract (library) timed out".into()));
            }
            if rc < 0 {
                return Err(OcrError::SubprocessOutput("tesseract failed (library: Recognize)".into()));
            }
            // SAFETY: valid instance after Recognize.
            let tsv = self.take_text(unsafe { (self.api.tsv_text)(h, 0) });
            let txt = self.take_text(unsafe { (self.api.utf8_text)(h) });
            let Some(tsv) = tsv else { return Err(OcrError::SubprocessOutput("tesseract failed (library: no TSV)".into())) };
            let mut page = parse_tsv(&tsv);
            page.text = txt.unwrap_or_default();
            Ok(page)
        })
    }

    /// Like `tesseract <image> stdout -l osd --psm 0` + [`crate::engine::parse_osd`].
    pub fn orientation(&self, tessdata: Option<&Path>, raster: &Raster, timeout: Option<Duration>) -> Result<Option<Orientation>> {
        let key = PoolKey { tessdata: tessdata.map(Path::to_path_buf), lang: "osd".into(), psm: PSM_OSD_ONLY };
        self.with_instance(&key, |h| {
            self.set_image(h, raster)?;
            let start = Instant::now();
            let (mut deg, mut conf, mut script, mut sconf): (c_int, c_float, *const c_char, c_float) = (0, 0.0, std::ptr::null(), 0.0);
            // SAFETY: valid instance; output pointers to local variables.
            let ok = unsafe {
                // ProcessPage (PSM_OSD_ONLY) calls FindLines before the renderer; AnalyseLayout does that too.
                let it = (self.api.analyse_layout)(h);
                if !it.is_null() {
                    (self.api.page_iterator_delete)(it);
                }
                (self.api.detect_os)(h, &mut deg, &mut conf, &mut script, &mut sconf)
            };
            if timeout.is_some_and(|t| start.elapsed() >= t) {
                return Err(OcrError::SubprocessOutput("tesseract (library) OSD timed out".into()));
            }
            if ok == 0 {
                tracing::debug!("tesseract OSD (library) failed (too few characters?)");
                return Ok(None);
            }
            Ok(Some(osd_result(deg, conf)))
        })
    }
}

/// `GetOsdText`: `Rotate` = `OrientationIdToValue(deg / 90)`, confidence with two decimal places
/// (`std::fixed`, `precision(2)`) – here read back to f64 like when parsing the CLI output.
pub fn osd_result(orient_deg: c_int, orient_conf: c_float) -> Orientation {
    let rotate = match orient_deg.rem_euclid(360) / 90 {
        1 => 270,
        2 => 180,
        3 => 90,
        _ => 0,
    };
    let text = format!("{:.2}", f64::from(orient_conf));
    Orientation { rotate, confidence: text.parse().unwrap_or(0.0) }
}

/// Possible library names in a directory.
fn candidates_in(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(windows) {
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut names: Vec<PathBuf> = rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    let n = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                    n.ends_with(".dll")
                        && (n.starts_with("libtesseract")
                            || (n.starts_with("tesseract") && n[9..].starts_with(|c: char| c.is_ascii_digit())))
                })
                .collect();
            names.sort();
            out.extend(names);
        }
    } else {
        for d in [dir.to_path_buf(), dir.join("../lib"), dir.join("../lib64")] {
            for n in ["libtesseract.so.5", "libtesseract.so", "libtesseract.5.dylib", "libtesseract.dylib"] {
                let p = d.join(n);
                if p.is_file() {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// Search the program in `PATH` (for `tesseract` without a path).
fn which(program: &Path) -> Option<PathBuf> {
    if program.components().count() > 1 {
        return Some(program.to_path_buf());
    }
    let exts: &[&str] = if cfg!(windows) { &["", ".exe"] } else { &[""] };
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|d| exts.iter().map(|e| d.join(format!("{}{e}", program.display()))).find(|p| p.is_file()))
    })
}

/// Result of the search for the library.
#[derive(Debug, Clone, PartialEq)]
pub enum LibSetting {
    /// `PAPERRUST_TESSERACT_LIB=off` (or `0`/`false`/`no`).
    Disabled,
    /// Explicitly set (file or directory).
    Explicit(PathBuf),
    /// Not set: search automatically.
    Auto,
}

pub fn lib_setting() -> LibSetting {
    match std::env::var_os("PAPERRUST_TESSERACT_LIB") {
        None => LibSetting::Auto,
        Some(v) => {
            let s = v.to_string_lossy().trim().to_ascii_lowercase();
            if matches!(s.as_str(), "" | "0" | "off" | "false" | "no" | "none") {
                LibSetting::Disabled
            } else {
                LibSetting::Explicit(PathBuf::from(v))
            }
        }
    }
}

/// Candidates in search order (see module docs).
pub fn find_library(setting: &LibSetting, program: &Path) -> Vec<PathBuf> {
    match setting {
        LibSetting::Disabled => vec![],
        LibSetting::Explicit(p) if p.is_dir() => candidates_in(p),
        LibSetting::Explicit(p) => vec![p.clone()],
        LibSetting::Auto => {
            let mut out = Vec::new();
            if let Some(exe) = which(program) {
                // Resolve symlinks (e.g. /usr/bin/tesseract); not on Windows (\?-prefix).
                let exe = if cfg!(windows) { exe } else { std::fs::canonicalize(&exe).unwrap_or(exe) };
                if let Some(d) = exe.parent() {
                    out.extend(candidates_in(d));
                }
            }
            if !cfg!(windows) {
                // System library (Debian: libtesseract5 belongs to the package tesseract-ocr).
                for n in ["libtesseract.so.5", "libtesseract.5.dylib"] {
                    out.push(PathBuf::from(n));
                }
            }
            out
        }
    }
}

/// Process-wide loaded libraries (once per path), so that the pool lives across documents.
static LOADED: OnceLock<Mutex<HashMap<PathBuf, Arc<TessLib>>>> = OnceLock::new();

/// Load the library (cached per path).
pub fn load_cached(path: &Path) -> std::result::Result<Arc<TessLib>, String> {
    let m = LOADED.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(l) = g.get(path) {
        return Ok(l.clone());
    }
    let l = Arc::new(TessLib::load(path)?);
    g.insert(path.to_path_buf(), l.clone());
    Ok(l)
}

/// Deadline after which an unused instance is released: `PAPERRUST_OCR_ENGINE_IDLE_SECS`
/// (seconds, default 180; `0` = never release, behavior before 2026-11-18). Invalid values → default.
pub fn idle_release_after() -> Option<Duration> {
    parse_idle_secs(pl_envlive::var("PAPERRUST_OCR_ENGINE_IDLE_SECS").as_deref())
}

/// Default deadline for [`idle_release_after`].
pub const DEFAULT_IDLE_SECS: u64 = 180;

fn parse_idle_secs(v: Option<&str>) -> Option<Duration> {
    let secs = match v.map(str::trim).filter(|s| !s.is_empty()) {
        None => DEFAULT_IDLE_SECS,
        Some(s) => match s.parse::<u64>() {
            Ok(n) => n,
            Err(_) => {
                tracing::warn!("PAPERRUST_OCR_ENGINE_IDLE_SECS={s:?} is not a number of seconds; using {DEFAULT_IDLE_SECS}");
                DEFAULT_IDLE_SECS
            }
        },
    };
    (secs > 0).then(|| Duration::from_secs(secs.min(86_400 * 365)))
}

/// Check interval of the cleanup thread: a quarter of the deadline, 1–10 s.
fn reaper_tick(max_idle: Duration) -> Duration {
    (max_idle / 4).clamp(Duration::from_secs(1), Duration::from_secs(10))
}

/// Starts the cleanup thread once per process (sleeps almost always; per round a short lock per
/// loaded library). The deadline is re-read every round (setting from the UI).
pub(crate) fn start_reaper() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let r = std::thread::Builder::new().name("ocr-engine-reaper".into()).spawn(|| {
            loop {
                let max_idle = idle_release_after();
                std::thread::sleep(max_idle.map_or(Duration::from_secs(10), reaper_tick));
                let Some(max_idle) = max_idle else { continue };
                // Tesseract instances and (on Windows) Windows OCR recognizers.
                if release_idle_all(max_idle) + crate::winocr::release_idle_all(max_idle) > 0 {
                    release_to_os();
                }
            }
        });
        if let Err(e) = r {
            tracing::warn!("could not start the OCR engine reaper thread: {e}; engines stay loaded");
        }
    });
}

/// [`TessLib::release_idle`] for all process-wide loaded libraries.
pub fn release_idle_all(max_idle: Duration) -> usize {
    let libs: Vec<Arc<TessLib>> = match LOADED.get() {
        Some(m) => m.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect(),
        None => return 0,
    };
    let now = Instant::now();
    let n: usize = libs.iter().map(|l| l.release_idle(max_idle, now)).sum();
    if n > 0 {
        tracing::debug!("tesseract: released {n} idle engine instance(s)");
    }
    n
}

/// Returns free heap to the operating system (after releasing instances).
///
/// - Linux/glibc (Docker image, Debian): Tesseract/Leptonica allocate via the C runtime's `malloc`
///   (the Rust allocator does not replace `malloc`). glibc returns an arena only from the top
///   and the arenas of finished page worker threads are never touched again.
///   `malloc_trim(0)` walks **all** arenas (since glibc 2.8) and returns whole free pages even
///   in the middle of the heap via `madvise(MADV_DONTNEED)` (RSS/PSS drops immediately). Duration: a few ms.
/// - Windows: the process heap (UCRT `malloc`) returns large blocks (own `VirtualAlloc`) immediately
///   and decommits free segments itself; nothing to do here.
/// - Others (musl, macOS): no counterpart needed or available.
///
/// Afterwards the registered hook ([`set_release_hook`]), e.g. the collection of the Rust allocator.
pub fn release_to_os() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> c_int;
        }
        // SAFETY: glibc function without preconditions, thread-safe.
        unsafe { malloc_trim(0) };
    }
    if let Some(h) = RELEASE_HOOK.get() {
        h();
    }
}

static RELEASE_HOOK: OnceLock<fn()> = OnceLock::new();

/// Additional memory return after releasing instances (settable once; the server
/// registers the collection of its global allocator here). Returns `false` if already set.
pub fn set_release_hook(hook: fn()) -> bool {
    RELEASE_HOOK.set(hook).is_ok()
}

/// First loadable library whose version matches `cli_version` (`tesseract --version`, first line).
pub fn resolve(setting: &LibSetting, program: &Path, cli_version: Option<&str>) -> Option<Arc<TessLib>> {
    let want = normalize_version(cli_version?);
    for cand in find_library(setting, program) {
        match load_cached(&cand) {
            Ok(l) if normalize_version(l.version()) == want => {
                tracing::info!("tesseract: using library {} ({})", cand.display(), l.version());
                return Some(l);
            }
            Ok(l) => tracing::info!(
                "tesseract: library {} has version {} but the program reports {want}; using the program",
                cand.display(),
                l.version()
            ),
            Err(e) => tracing::debug!("tesseract: library {} not usable: {e}", cand.display()),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_secs_parsing() {
        assert_eq!(parse_idle_secs(None), Some(Duration::from_secs(DEFAULT_IDLE_SECS)));
        assert_eq!(parse_idle_secs(Some("")), Some(Duration::from_secs(DEFAULT_IDLE_SECS)));
        assert_eq!(parse_idle_secs(Some(" 5 ")), Some(Duration::from_secs(5)));
        assert_eq!(parse_idle_secs(Some("0")), None);
        assert_eq!(parse_idle_secs(Some("abc")), Some(Duration::from_secs(DEFAULT_IDLE_SECS)));
        assert_eq!(parse_idle_secs(Some("-3")), Some(Duration::from_secs(DEFAULT_IDLE_SECS)));
        assert_eq!(reaper_tick(Duration::from_secs(30)), Duration::from_millis(7500));
        assert_eq!(reaper_tick(Duration::from_secs(1)), Duration::from_secs(1));
        assert_eq!(reaper_tick(Duration::from_secs(600)), Duration::from_secs(10));
    }

    #[test]
    fn release_to_os_is_harmless() {
        // May be called at any time (also without a loaded library).
        release_to_os();
        assert_eq!(release_idle_all(Duration::from_secs(3600)), 0);
    }

    #[test]
    fn versions() {
        assert_eq!(normalize_version("tesseract v5.4.0.20240606"), "5.4.0.20240606");
        assert_eq!(normalize_version("tesseract 5.5.4"), "5.5.4");
        assert_eq!(normalize_version("5.5.4"), "5.5.4");
    }

    #[test]
    fn osd_like_get_osd_text() {
        // OrientationIdToValue: 0→0, 1→270, 2→180, 3→90
        assert_eq!(osd_result(0, 1.8249).rotate, 0);
        assert_eq!(osd_result(90, 5.0).rotate, 270);
        assert_eq!(osd_result(180, 5.0).rotate, 180);
        assert_eq!(osd_result(270, 5.0).rotate, 90);
        assert_eq!(osd_result(270, 7.654_321).confidence, 7.65);
        assert_eq!(osd_result(0, 1.825).confidence, 1.83); // f32 1.825 = 1.82500005… (like printf %.2f on double)
        assert_eq!(osd_result(0, 1.815).confidence, 1.82); // f32 1.815 = 1.81500005…
        assert_eq!(osd_result(0, 0.0).confidence, 0.0);
    }

    #[test]
    fn setting_parsing() {
        // Only the pure evaluation (do not change the environment).
        assert_eq!(find_library(&LibSetting::Disabled, Path::new("tesseract")), Vec::<PathBuf>::new());
        assert_eq!(find_library(&LibSetting::Explicit("x/lib.so".into()), Path::new("tesseract")), vec![PathBuf::from("x/lib.so")]);
    }
}
