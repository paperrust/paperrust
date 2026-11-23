// upstream: src/paperless/config.py@v3.4.2 (OutputTypeConfig, OcrConfig, RemoteOCRConfig)
// upstream: src/paperless/models.py@v3.4.2 (OutputTypeChoices, ModeChoices, ArchiveFileGenerationChoices,
//           CleanChoices, ColorConvertChoices, RemoteOCRMode, ApplicationConfiguration)
// upstream: src/paperless/settings/__init__.py@v3.4.2 (OCR_*, ARCHIVE_FILE_GENERATION, MAX_IMAGE_PIXELS,
//           THREADS_PER_WORKER, REMOTE_OCR_*), src/paperless/settings/parsers.py (get_*_from_env)
//
// Two levels like upstream:
// 1. `OcrSettings` = Django settings from the environment (read at startup; `OCR_MODE`,
//    `ARCHIVE_FILE_GENERATION`, numbers/bools are already validated here).
// 2. `OcrConfig::new(settings, app_config)` = `OcrConfig()`: DB values (`ApplicationConfiguration`)
//    override with Python `or` semantics (falsy → environment), `deskew`/`rotate_pages` with
//    `is not None`. `OutputTypeChoices(...)`, `CleanChoices(...)`, `ColorConvertChoices(...)` are
//    only validated here (invalid environment → `ValueError` on parsing, not at startup).

use serde::{Deserialize, Serialize};

use crate::OcrError;

macro_rules! choices {
    ($(#[$m:meta])* $name:ident, $py:literal, { $($var:ident = $val:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name { $( #[serde(rename = $val)] $var ),+ }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$var),+];
            pub fn as_str(self) -> &'static str { match self { $($name::$var => $val),+ } }
            /// Python `Choices(value)`; error text like `ValueError`.
            pub fn parse(value: &str) -> Result<Self, OcrError> {
                match value {
                    $($val => Ok($name::$var),)+
                    _ => Err(OcrError::Config(format!("'{}' is not a valid {}", value, $py))),
                }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.as_str()) }
        }
    };
}

choices!(
    /// `--output-type`.
    OutputType, "OutputTypeChoices", {
    Pdf = "pdf", PdfA = "pdfa", PdfA1 = "pdfa-1", PdfA2 = "pdfa-2", PdfA3 = "pdfa-3",
});
choices!(
    /// `PAPERLESS_OCR_MODE` (v3.4.2: auto/force/redo/off; `skip`/`skip_noarchive` → auto).
    Mode, "ModeChoices", { Auto = "auto", Force = "force", Redo = "redo", Off = "off" });
choices!(ArchiveFileGeneration, "ArchiveFileGenerationChoices", {
    Auto = "auto", Always = "always", Never = "never" });
choices!(
    /// `--clean` / `--clean-final`.
    Clean, "CleanChoices", { Clean = "clean", Final = "clean-final", None = "none" });
choices!(
    /// Ghostscript `-sColorConversionStrategy`.
    ColorConversion, "ColorConvertChoices", {
    LeaveColorUnchanged = "LeaveColorUnchanged", Rgb = "RGB",
    UseDeviceIndependentColor = "UseDeviceIndependentColor", Gray = "Gray", Cmyk = "CMYK",
});
choices!(RemoteOcrMode, "RemoteOCRMode", { Always = "always", WorkflowOnly = "workflow_only" });

impl OutputType {
    pub fn is_pdfa(self) -> bool {
        // upstream: `"pdfa" in ocrmypdf_args["output_type"]`
        self.as_str().contains("pdfa")
    }
    /// `_convert_pdf_to_pdfa`: pdfa→2, pdfa-N→N; `None` for `pdf`.
    pub fn pdfa_part(self) -> Option<u8> {
        match self {
            OutputType::Pdf => None,
            OutputType::PdfA | OutputType::PdfA2 => Some(2),
            OutputType::PdfA1 => Some(1),
            OutputType::PdfA3 => Some(3),
        }
    }
}

/// Django settings (environment). Fields are named like the settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrSettings {
    pub ocr_pages: Option<i64>,
    pub ocr_language: String,
    /// Raw (only validated in `OcrConfig` with `OutputTypeChoices(...)`).
    pub ocr_output_type: String,
    pub ocr_mode: Mode,
    pub archive_file_generation: ArchiveFileGeneration,
    pub ocr_image_dpi: Option<i64>,
    pub ocr_clean: String,
    pub ocr_deskew: bool,
    pub ocr_rotate_pages: bool,
    pub ocr_rotate_pages_threshold: f64,
    pub ocr_max_image_pixels: Option<i64>,
    pub ocr_color_conversion_strategy: String,
    pub ocr_user_args: Option<String>,
    /// `PAPERLESS_MAX_IMAGE_PIXELS` (Pillow limit in paperless itself).
    pub max_image_pixels: Option<i64>,
    pub threads_per_worker: i64,
    pub remote_ocr_engine: Option<String>,
    pub remote_ocr_api_key: Option<String>,
    pub remote_ocr_endpoint: Option<String>,
    pub remote_ocr_mode: RemoteOcrMode,
    pub remote_ocr_allow_internal_endpoints: bool,
}

/// OCR languages when `PAPERLESS_OCR_LANGUAGE` is not set. paperless-ngx uses `eng`; PaperRust
/// `deu+eng` (Jetson measurement 2026-11-21, results/benchmark/2026-11-21-ocr-lang: German pages
/// with `eng` only 88.6 % of the words – umlauts/ß lost; `deu+eng` 100 % for German and English at
/// +3–7 % time). All PaperRust packages ship `deu` and `eng`.
pub const DEFAULT_OCR_LANGUAGE: &str = "deu+eng";
/// Used instead of [`DEFAULT_OCR_LANGUAGE`] if the engine has no German data (own builds).
pub const DEFAULT_OCR_LANGUAGE_FALLBACK: &str = "eng";

impl Default for OcrSettings {
    fn default() -> Self {
        Self {
            ocr_pages: None,
            ocr_language: DEFAULT_OCR_LANGUAGE.into(),
            ocr_output_type: "pdfa".into(),
            ocr_mode: Mode::Auto,
            archive_file_generation: ArchiveFileGeneration::Auto,
            ocr_image_dpi: None,
            ocr_clean: "clean".into(),
            ocr_deskew: true,
            ocr_rotate_pages: true,
            ocr_rotate_pages_threshold: 12.0,
            ocr_max_image_pixels: None,
            ocr_color_conversion_strategy: "RGB".into(),
            ocr_user_args: None,
            max_image_pixels: None,
            threads_per_worker: default_threads_per_worker(1, available_cores()),
            remote_ocr_engine: None,
            remote_ocr_api_key: None,
            remote_ocr_endpoint: None,
            remote_ocr_mode: RemoteOcrMode::Always,
            remote_ocr_allow_internal_endpoints: true,
        }
    }
}

fn available_cores() -> i64 {
    std::thread::available_parallelism().map(|n| n.get() as i64).unwrap_or(1)
}

/// `default_threads_per_worker(task_workers)`: `max(floor(cores / workers), 1)`.
/// (Python: `task_workers == 0` → `ZeroDivisionError`, not caught; here 1.)
pub fn default_threads_per_worker(task_workers: i64, cores: i64) -> i64 {
    let cores = cores.max(1);
    if task_workers <= 0 {
        return 1;
    }
    (cores.div_euclid(task_workers)).max(1)
}

/// Python `int(str)`: whitespace around, sign, `_` between digits.
pub fn py_int(key: &str, value: &str) -> Result<i64, OcrError> {
    let t = value.trim();
    let err = || OcrError::Config(format!("invalid literal for int() with base 10: '{value}' ({key})"));
    let (sign, digits) = match t.strip_prefix('-') {
        Some(r) => (-1i64, r),
        None => (1, t.strip_prefix('+').unwrap_or(t)),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return Err(err());
    }
    let n: i64 = digits.replace('_', "").parse().map_err(|_| err())?;
    Ok(sign * n)
}

/// Name for messages: `PAPERLESS_X` → `PAPERRUST_X` (both are read; like
/// `pl_core::config::primary_name`, without a dependency on pl-core).
pub fn primary_name(key: &str) -> String {
    match key.strip_prefix("PAPERLESS_RS_").or_else(|| key.strip_prefix("PAPERLESS_")) {
        Some(rest) => format!("PAPERRUST_{rest}"),
        None => key.to_string(),
    }
}

/// Python `float(str)` (subset: decimal/exponent notation, inf/nan).
pub fn py_float(key: &str, value: &str) -> Result<f64, OcrError> {
    let t = value.trim().replace('_', "");
    let lower = t.to_ascii_lowercase();
    let v = match lower.trim_start_matches(['+', '-']) {
        "inf" | "infinity" => Some(f64::INFINITY),
        "nan" => Some(f64::NAN),
        _ => t.parse::<f64>().ok(),
    };
    match v {
        Some(x) if lower.starts_with('-') && x.is_infinite() => Ok(-x.abs()),
        Some(x) => Ok(x),
        None => Err(OcrError::Config(format!("could not convert string to float: '{value}' ({})", primary_name(key)))),
    }
}

/// `str_to_bool`.
pub fn str_to_bool(value: &str) -> Result<bool, OcrError> {
    match value.trim().to_lowercase().as_str() {
        "true" | "1" | "t" | "y" | "yes" => Ok(true),
        "false" | "0" | "f" | "n" | "no" => Ok(false),
        _ => Err(OcrError::Config(format!("Cannot convert '{value}' to a boolean."))),
    }
}

fn choice(env_key: &str, value: Option<String>, choices: &[&str], default: &str) -> Result<String, OcrError> {
    let v = value.unwrap_or_else(|| default.to_string());
    if choices.contains(&v.as_str()) {
        return Ok(v);
    }
    let mut sorted: Vec<&str> = choices.to_vec();
    sorted.sort_unstable();
    Err(OcrError::Config(format!(
        "Environment variable '{}' has invalid value '{v}'. Valid choices are: {}",
        primary_name(env_key),
        sorted.join(", ")
    )))
}

impl OcrSettings {
    /// Reads the settings like `paperless/settings/__init__.py` from an environment.
    pub fn from_env<F: Fn(&str) -> Option<String>>(get: F) -> Result<Self, OcrError> {
        let int_opt = |k: &str| -> Result<Option<i64>, OcrError> { get(k).map(|v| py_int(k, &v)).transpose() };
        let bool_env = |k: &str, d: &str| str_to_bool(&get(k).unwrap_or_else(|| d.into()));
        let d = OcrSettings::default();

        // `if os.environ.get("PAPERLESS_OCR_MODE", "") in ("skip", "skip_noarchive"): OCR_MODE = "auto"`
        let raw_mode = get("PAPERLESS_OCR_MODE");
        let mode = match raw_mode.as_deref() {
            Some("skip") | Some("skip_noarchive") => "auto".to_string(),
            _ => choice("PAPERLESS_OCR_MODE", raw_mode, &["auto", "force", "redo", "off"], "auto")?,
        };
        let afg =
            choice("PAPERLESS_ARCHIVE_FILE_GENERATION", get("PAPERLESS_ARCHIVE_FILE_GENERATION"), &["auto", "always", "never"], "auto")?;
        // Unset: 1 (upstream), so one document uses all cores. `auto` (PaperRust opt-in): same
        // automatic task count as the worker pool (pl-queue), so that tasks × threads matches
        // the cores. Anything else: `int()` with the ngx error text.
        let task_workers = match get("PAPERLESS_TASK_WORKERS") {
            Some(v) if pl_core::resources::is_auto(&v) => {
                pl_core::resources::task_workers_or_auto(None, int_opt("PAPERLESS_THREADS_PER_WORKER")?)
            }
            Some(v) => py_int("PAPERLESS_TASK_WORKERS", &v)?,
            None => pl_core::resources::DEFAULT_TASK_WORKERS,
        };
        let threads = match int_opt("PAPERLESS_THREADS_PER_WORKER")? {
            Some(t) => t,
            None => default_threads_per_worker(task_workers, available_cores()),
        };
        let remote_mode = choice("PAPERLESS_REMOTE_OCR_MODE", get("PAPERLESS_REMOTE_OCR_MODE"), &["always", "workflow_only"], "always")?;
        Ok(Self {
            ocr_pages: int_opt("PAPERLESS_OCR_PAGES")?,
            ocr_language: get("PAPERLESS_OCR_LANGUAGE").unwrap_or(d.ocr_language),
            ocr_output_type: get("PAPERLESS_OCR_OUTPUT_TYPE").unwrap_or(d.ocr_output_type),
            ocr_mode: Mode::parse(&mode)?,
            archive_file_generation: ArchiveFileGeneration::parse(&afg)?,
            ocr_image_dpi: int_opt("PAPERLESS_OCR_IMAGE_DPI")?,
            ocr_clean: get("PAPERLESS_OCR_CLEAN").unwrap_or(d.ocr_clean),
            ocr_deskew: bool_env("PAPERLESS_OCR_DESKEW", "true")?,
            ocr_rotate_pages: bool_env("PAPERLESS_OCR_ROTATE_PAGES", "true")?,
            ocr_rotate_pages_threshold: match get("PAPERLESS_OCR_ROTATE_PAGES_THRESHOLD") {
                Some(v) => py_float("PAPERLESS_OCR_ROTATE_PAGES_THRESHOLD", &v)?,
                None => 12.0,
            },
            ocr_max_image_pixels: int_opt("PAPERLESS_OCR_MAX_IMAGE_PIXELS")?,
            ocr_color_conversion_strategy: get("PAPERLESS_OCR_COLOR_CONVERSION_STRATEGY").unwrap_or(d.ocr_color_conversion_strategy),
            ocr_user_args: get("PAPERLESS_OCR_USER_ARGS"),
            max_image_pixels: int_opt("PAPERLESS_MAX_IMAGE_PIXELS")?,
            threads_per_worker: threads,
            remote_ocr_engine: get("PAPERLESS_REMOTE_OCR_ENGINE"),
            remote_ocr_api_key: get("PAPERLESS_REMOTE_OCR_API_KEY"),
            remote_ocr_endpoint: get("PAPERLESS_REMOTE_OCR_ENDPOINT"),
            remote_ocr_mode: RemoteOcrMode::parse(&remote_mode)?,
            remote_ocr_allow_internal_endpoints: bool_env("PAPERLESS_REMOTE_OCR_ALLOW_INTERNAL_ENDPOINTS", "true")?,
        })
    }

    pub fn from_process_env() -> Result<Self, OcrError> {
        Self::from_env(|k| std::env::var(k).ok())
    }
}

/// Columns of `paperless_applicationconfiguration` (all nullable). Strings as they are in the
/// DB (not validated – Django validates choices only in the form/serializer).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppConfigOverrides {
    pub output_type: Option<String>,
    pub pages: Option<i64>,
    pub language: Option<String>,
    pub mode: Option<String>,
    pub archive_file_generation: Option<String>,
    pub image_dpi: Option<i64>,
    pub unpaper_clean: Option<String>,
    pub deskew: Option<bool>,
    pub rotate_pages: Option<bool>,
    pub rotate_pages_threshold: Option<f64>,
    pub max_image_pixels: Option<f64>,
    pub color_conversion_strategy: Option<String>,
    pub user_args: Option<serde_json::Value>,
    pub remote_ocr_engine: Option<String>,
    pub remote_ocr_api_key: Option<String>,
    pub remote_ocr_endpoint: Option<String>,
    pub remote_ocr_mode: Option<String>,
}

/// Python truth value of a JSON value (`if app_config.user_args:`).
pub fn py_truthy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Array(a) => !a.is_empty(),
        serde_json::Value::Object(o) => !o.is_empty(),
    }
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|v| !v.is_empty())
}

/// `paperless.config.OcrConfig` (resolved).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrConfig {
    pub output_type: OutputType,
    pub pages: Option<i64>,
    pub language: String,
    pub mode: Mode,
    pub archive_file_generation: ArchiveFileGeneration,
    pub image_dpi: Option<i64>,
    pub clean: Clean,
    pub deskew: bool,
    pub rotate: bool,
    pub rotate_threshold: f64,
    /// `app_config.max_image_pixels or settings.OCR_MAX_IMAGE_PIXELS` (float from DB, int from env).
    pub max_image_pixel: Option<f64>,
    pub color_conversion_strategy: ColorConversion,
    /// `None` or any JSON value (upstream expects a dict; anything else → warning when merging).
    pub user_args: Option<serde_json::Value>,
    /// not part of `OcrConfig`, but read by `construct_ocrmypdf_parameters`.
    pub threads_per_worker: i64,
    /// `settings.MAX_IMAGE_PIXELS` (for `maybe_override_pixel_limit`).
    pub max_image_pixels_setting: Option<i64>,
}

impl OcrConfig {
    /// `OcrConfig()`.
    pub fn new(s: &OcrSettings, app: &AppConfigOverrides) -> Result<Self, OcrError> {
        let output_type = match nonempty(&app.output_type) {
            Some(v) => OutputType::parse(v)?,
            None => OutputType::parse(&s.ocr_output_type)?,
        };
        let pages = app.pages.filter(|p| *p != 0).or(s.ocr_pages);
        let language = nonempty(&app.language).map(str::to_string).unwrap_or_else(|| s.ocr_language.clone());
        let mode = match nonempty(&app.mode) {
            Some(v) => Mode::parse(v)?,
            None => s.ocr_mode,
        };
        let archive_file_generation = match nonempty(&app.archive_file_generation) {
            Some(v) => ArchiveFileGeneration::parse(v)?,
            None => s.archive_file_generation,
        };
        let image_dpi = app.image_dpi.filter(|d| *d != 0).or(s.ocr_image_dpi);
        let clean = match nonempty(&app.unpaper_clean) {
            Some(v) => Clean::parse(v)?,
            None => Clean::parse(&s.ocr_clean)?,
        };
        let deskew = app.deskew.unwrap_or(s.ocr_deskew);
        let rotate = app.rotate_pages.unwrap_or(s.ocr_rotate_pages);
        // `or`: 0.0 in the DB falls back to the environment (upstream behavior).
        let rotate_threshold = app.rotate_pages_threshold.filter(|t| *t != 0.0).unwrap_or(s.ocr_rotate_pages_threshold);
        let max_image_pixel = app.max_image_pixels.filter(|p| *p != 0.0).or(s.ocr_max_image_pixels.map(|v| v as f64));
        let color_conversion_strategy = match nonempty(&app.color_conversion_strategy) {
            Some(v) => ColorConversion::parse(v)?,
            None => ColorConversion::parse(&s.ocr_color_conversion_strategy)?,
        };
        let user_args = match &app.user_args {
            Some(v) if py_truthy(v) => Some(v.clone()),
            _ => s.ocr_user_args.as_ref().map(|raw| {
                serde_json::from_str::<serde_json::Value>(raw).unwrap_or_else(|_| serde_json::Value::Object(Default::default()))
            }),
        };
        Ok(Self {
            output_type,
            pages,
            language,
            mode,
            archive_file_generation,
            image_dpi,
            clean,
            deskew,
            rotate,
            rotate_threshold,
            max_image_pixel,
            color_conversion_strategy,
            user_args,
            threads_per_worker: s.threads_per_worker,
            max_image_pixels_setting: s.max_image_pixels,
        })
    }

    /// Default values (environment empty, no DB overrides).
    pub fn defaults() -> Self {
        // Default settings are valid; an error is not possible.
        match Self::new(&OcrSettings::default(), &AppConfigOverrides::default()) {
            Ok(c) => c,
            Err(_) => unreachable_default(),
        }
    }
}

#[cold]
fn unreachable_default() -> OcrConfig {
    OcrConfig {
        output_type: OutputType::PdfA,
        pages: None,
        language: "eng".into(),
        mode: Mode::Auto,
        archive_file_generation: ArchiveFileGeneration::Auto,
        image_dpi: None,
        clean: Clean::Clean,
        deskew: true,
        rotate: true,
        rotate_threshold: 12.0,
        max_image_pixel: None,
        color_conversion_strategy: ColorConversion::Rgb,
        user_args: None,
        threads_per_worker: 1,
        max_image_pixels_setting: None,
    }
}

/// `paperless.config.RemoteOCRConfig` + `paperless.parsers.remote.RemoteEngineConfig`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteOcrConfig {
    pub engine: Option<String>,
    pub api_key: Option<String>,
    pub endpoint: Option<String>,
    pub mode: RemoteOcrMode,
    pub allow_internal_endpoints: bool,
}

impl RemoteOcrConfig {
    pub fn new(s: &OcrSettings, app: &AppConfigOverrides) -> Result<Self, OcrError> {
        let pick = |a: &Option<String>, b: &Option<String>| nonempty(a).map(str::to_string).or_else(|| b.clone());
        Ok(Self {
            engine: pick(&app.remote_ocr_engine, &s.remote_ocr_engine),
            api_key: pick(&app.remote_ocr_api_key, &s.remote_ocr_api_key),
            endpoint: pick(&app.remote_ocr_endpoint, &s.remote_ocr_endpoint),
            mode: match nonempty(&app.remote_ocr_mode) {
                Some(v) => RemoteOcrMode::parse(v)?,
                None => s.remote_ocr_mode,
            },
            allow_internal_endpoints: s.remote_ocr_allow_internal_endpoints,
        })
    }

    /// `RemoteEngineConfig.engine_is_valid()`.
    pub fn engine_is_valid(&self) -> bool {
        self.engine.as_deref() == Some("azureai") && self.api_key.is_some() && self.endpoint.is_some()
    }

    /// `remote_ocr_by_default`.
    pub fn remote_ocr_by_default(&self) -> bool {
        self.mode == RemoteOcrMode::Always
    }

    /// `RemoteDocumentParser.score()`: 20 for a valid engine and supported type, otherwise `None`.
    pub fn score(&self, mime_type: &str) -> Option<i32> {
        if !self.engine_is_valid() {
            return None;
        }
        REMOTE_SUPPORTED_MIME_TYPES.iter().any(|(m, _)| *m == mime_type).then_some(20)
    }
}

/// `paperless.parsers.remote._SUPPORTED_MIME_TYPES` (without HEIC).
pub const REMOTE_SUPPORTED_MIME_TYPES: &[(&str, &str)] = &[
    ("application/pdf", ".pdf"),
    ("image/png", ".png"),
    ("image/jpeg", ".jpg"),
    ("image/tiff", ".tiff"),
    ("image/bmp", ".bmp"),
    ("image/gif", ".gif"),
    ("image/webp", ".webp"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn legacy_modes_map_to_auto() {
        for m in ["skip", "skip_noarchive"] {
            let s = OcrSettings::from_env(env(&[("PAPERLESS_OCR_MODE", m)])).unwrap();
            assert_eq!(s.ocr_mode, Mode::Auto);
        }
        let e = OcrSettings::from_env(env(&[("PAPERLESS_OCR_MODE", "skipp")])).unwrap_err();
        assert_eq!(
            e.message(),
            "Environment variable 'PAPERRUST_OCR_MODE' has invalid value 'skipp'. Valid choices are: auto, force, off, redo"
        );
    }

    // upstream: paperless/tests/test_ocr_config.py
    #[test]
    fn archive_file_generation_from_settings_and_db() {
        for v in ["auto", "always", "never"] {
            let s = OcrSettings::from_env(env(&[("PAPERLESS_ARCHIVE_FILE_GENERATION", v)])).unwrap();
            let c = OcrConfig::new(&s, &AppConfigOverrides::default()).unwrap();
            assert_eq!(c.archive_file_generation.as_str(), v);
        }
        let s = OcrSettings::from_env(env(&[("PAPERLESS_ARCHIVE_FILE_GENERATION", "always")])).unwrap();
        let app = AppConfigOverrides { archive_file_generation: Some("never".into()), ..Default::default() };
        assert_eq!(OcrConfig::new(&s, &app).unwrap().archive_file_generation, ArchiveFileGeneration::Never);
    }

    #[test]
    fn falsy_db_values_fall_back() {
        let s = OcrSettings { ocr_rotate_pages_threshold: 30.0, ocr_pages: Some(3), ..Default::default() };
        let app = AppConfigOverrides {
            rotate_pages_threshold: Some(0.0),
            pages: Some(0),
            language: Some(String::new()),
            deskew: Some(false),
            user_args: Some(serde_json::json!({})),
            ..Default::default()
        };
        let c = OcrConfig::new(&s, &app).unwrap();
        assert_eq!(c.rotate_threshold, 30.0);
        assert_eq!(c.pages, Some(3));
        assert_eq!(c.language, DEFAULT_OCR_LANGUAGE, "empty app language → built-in default (PaperRust: deu+eng)");
        assert!(!c.deskew); // `is not None`
        assert_eq!(c.user_args, None);
    }

    #[test]
    fn invalid_env_choice_fails_at_config_time() {
        let s = OcrSettings { ocr_output_type: "pdfx".into(), ..Default::default() };
        let e = OcrConfig::new(&s, &AppConfigOverrides::default()).unwrap_err();
        assert_eq!(e.message(), "'pdfx' is not a valid OutputTypeChoices");
    }

    #[test]
    fn user_args_bad_json_is_empty_dict() {
        let s = OcrSettings { ocr_user_args: Some("{nope".into()), ..Default::default() };
        let c = OcrConfig::new(&s, &AppConfigOverrides::default()).unwrap();
        assert_eq!(c.user_args, Some(serde_json::json!({})));
    }

    #[test]
    fn ints_like_python() {
        assert_eq!(py_int("k", " 1_000 ").unwrap(), 1000);
        assert_eq!(py_int("k", "-5").unwrap(), -5);
        assert!(py_int("k", "1.5").is_err());
        assert!(py_int("k", "_1").is_err());
        assert_eq!(default_threads_per_worker(1, 12), 12);
        assert_eq!(default_threads_per_worker(5, 12), 2);
        assert_eq!(default_threads_per_worker(20, 12), 1);
    }

    #[test]
    fn task_workers_default_auto_and_number() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|(a, _)| *a == k).map(|(_, v)| (*v).to_string())
        };
        let cores = available_cores();
        // unset: 1 task -> all cores for one document
        let s = OcrSettings::from_env(env(&[])).unwrap();
        assert_eq!(s.threads_per_worker, default_threads_per_worker(1, cores));
        // number: as before
        let s = OcrSettings::from_env(env(&[("PAPERLESS_TASK_WORKERS", "2")])).unwrap();
        assert_eq!(s.threads_per_worker, default_threads_per_worker(2, cores));
        // `auto` (any case): same task count as pl-queue
        let auto = pl_core::resources::task_workers_or_auto(None, None);
        for v in ["auto", "AUTO"] {
            let s = OcrSettings::from_env(|k| (k == "PAPERLESS_TASK_WORKERS").then(|| v.to_string())).unwrap();
            assert_eq!(s.threads_per_worker, default_threads_per_worker(auto, cores));
        }
        // explicit threads win
        let s = OcrSettings::from_env(env(&[("PAPERLESS_TASK_WORKERS", "auto"), ("PAPERLESS_THREADS_PER_WORKER", "3")])).unwrap();
        assert_eq!(s.threads_per_worker, 3);
        // other invalid values keep the ngx error text
        let e = OcrSettings::from_env(env(&[("PAPERLESS_TASK_WORKERS", "autox")])).unwrap_err();
        assert!(e.message().contains("invalid literal for int() with base 10: 'autox'"), "{}", e.message());
    }

    #[test]
    fn remote_config() {
        let s = OcrSettings { remote_ocr_engine: Some("azureai".into()), remote_ocr_api_key: Some("k".into()), ..Default::default() };
        let c = RemoteOcrConfig::new(&s, &AppConfigOverrides::default()).unwrap();
        assert!(!c.engine_is_valid());
        let app = AppConfigOverrides { remote_ocr_endpoint: Some("https://x.invalid".into()), ..Default::default() };
        let c = RemoteOcrConfig::new(&s, &app).unwrap();
        assert!(c.engine_is_valid());
        assert_eq!(c.score("image/png"), Some(20));
        assert_eq!(c.score("image/heic"), None);
        assert!(c.remote_ocr_by_default());
    }
}
