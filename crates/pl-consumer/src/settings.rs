// upstream: src/paperless/settings/__init__.py@v3.4.2 (directories, CONSUMER_*, PRE/POST_CONSUME_SCRIPT,
//           DATE_ORDER, FILENAME_DATE_ORDER, IGNORE_DATES, AUDIT_LOG_ENABLED, MODEL_FILE,
//           CLASSIFIER_MATCH_THRESHOLD), src/paperless/settings/parsers.py@v3.4.2 (str_to_bool,
//           get_*_from_env), src/paperless/config.py@v3.4.2 (BarcodeConfig)

use std::path::PathBuf;

use chrono_tz::Tz;

/// Error while reading the settings (text like upstream `ValueError`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SettingsError(pub String);

/// `str_to_bool`.
pub fn str_to_bool(value: &str) -> Result<bool, SettingsError> {
    match value.trim().to_lowercase().as_str() {
        "true" | "1" | "t" | "y" | "yes" => Ok(true),
        "false" | "0" | "f" | "n" | "no" => Ok(false),
        _ => Err(SettingsError(format!("Cannot convert '{value}' to a boolean."))),
    }
}

/// Settings of the barcode plugin (`BarcodeConfig`). `ConsumerSettings::barcodes` holds the
/// values from the environment; [`BarcodeSettings::with_app_config`] layers the DB values
/// (`paperless_applicationconfiguration`) on top, like `BarcodeConfig.__post_init__`.
#[derive(Debug, Clone, PartialEq)]
pub struct BarcodeSettings {
    pub barcodes_enabled: bool,
    pub barcode_enable_tiff_support: bool,
    pub barcode_string: String,
    pub barcode_enable_asn: bool,
    pub barcode_asn_prefix: String,
    pub barcode_enable_tag: bool,
    /// `{regex: replacement}` in insertion order.
    pub barcode_tag_mapping: Vec<(String, String)>,
    pub barcode_tag_split: bool,
    pub barcode_store_values: bool,
    pub barcode_retain_split_pages: bool,
    /// `CONSUMER_BARCODE_UPSCALE` (0.0 = off)
    pub barcode_upscale: f64,
    /// `CONSUMER_BARCODE_DPI` (300)
    pub barcode_dpi: u32,
    /// `CONSUMER_BARCODE_MAX_PAGES` (0 = all)
    pub barcode_max_pages: u32,
}

/// Barcode columns of the `ApplicationConfiguration` (`None` = NULL).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BarcodeAppConfig {
    pub barcodes_enabled: Option<bool>,
    pub barcode_enable_tiff_support: Option<bool>,
    pub barcode_string: Option<String>,
    pub barcode_retain_split_pages: Option<bool>,
    pub barcode_enable_asn: Option<bool>,
    pub barcode_asn_prefix: Option<String>,
    pub barcode_upscale: Option<f64>,
    pub barcode_dpi: Option<i64>,
    pub barcode_max_pages: Option<i64>,
    pub barcode_enable_tag: Option<bool>,
    pub barcode_tag_mapping: Option<serde_json::Value>,
    pub barcode_tag_split: Option<bool>,
    pub barcode_store_values: Option<bool>,
}

impl BarcodeSettings {
    /// `BarcodeConfig.__post_init__`: booleans apply if not NULL (`is not None`);
    /// texts, numbers and the mapping only if "truthy" (`app_config.x or settings.X`: empty
    /// text, 0, 0.0, empty object → environment value).
    pub fn with_app_config(&self, c: &BarcodeAppConfig) -> Self {
        let mut s = self.clone();
        let b = |v: Option<bool>, d: bool| v.unwrap_or(d);
        s.barcodes_enabled = b(c.barcodes_enabled, s.barcodes_enabled);
        s.barcode_enable_tiff_support = b(c.barcode_enable_tiff_support, s.barcode_enable_tiff_support);
        if let Some(v) = c.barcode_string.as_ref().filter(|v| !v.is_empty()) {
            s.barcode_string.clone_from(v);
        }
        s.barcode_retain_split_pages = b(c.barcode_retain_split_pages, s.barcode_retain_split_pages);
        s.barcode_enable_asn = b(c.barcode_enable_asn, s.barcode_enable_asn);
        if let Some(v) = c.barcode_asn_prefix.as_ref().filter(|v| !v.is_empty()) {
            s.barcode_asn_prefix.clone_from(v);
        }
        if let Some(v) = c.barcode_upscale.filter(|v| *v != 0.0) {
            s.barcode_upscale = v;
        }
        if let Some(v) = c.barcode_dpi.filter(|v| *v != 0) {
            s.barcode_dpi = u32::try_from(v).unwrap_or(s.barcode_dpi);
        }
        if let Some(v) = c.barcode_max_pages.filter(|v| *v != 0) {
            s.barcode_max_pages = u32::try_from(v).unwrap_or(s.barcode_max_pages);
        }
        s.barcode_enable_tag = b(c.barcode_enable_tag, s.barcode_enable_tag);
        if let Some(serde_json::Value::Object(m)) = &c.barcode_tag_mapping
            && !m.is_empty()
        {
            s.barcode_tag_mapping =
                m.iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect();
        }
        s.barcode_tag_split = b(c.barcode_tag_split, s.barcode_tag_split);
        s.barcode_store_values = b(c.barcode_store_values, s.barcode_store_values);
        s
    }

    /// Raster settings for `scan_pdf`.
    pub fn scan_settings(&self) -> pl_pdfops::barcodes::ScanSettings {
        pl_pdfops::barcodes::ScanSettings { dpi: self.barcode_dpi, upscale: self.barcode_upscale, max_pages: self.barcode_max_pages }
    }

    /// `BarcodePlugin.able_to_run` (without scanner availability).
    pub fn able_to_run(&self, mime_type: &str) -> bool {
        let scannable = mime_type == "application/pdf" || (self.barcode_enable_tiff_support && mime_type == "image/tiff");
        (self.barcode_enable_asn || self.barcodes_enabled || self.barcode_enable_tag || self.barcode_store_values) && scannable
    }
}

impl Default for BarcodeSettings {
    fn default() -> Self {
        Self {
            barcodes_enabled: false,
            barcode_enable_tiff_support: false,
            barcode_string: "PATCHT".into(),
            barcode_enable_asn: false,
            barcode_asn_prefix: "ASN".into(),
            barcode_enable_tag: false,
            barcode_tag_mapping: vec![("TAG:(.*)".into(), "\\g<1>".into())],
            barcode_tag_split: false,
            barcode_store_values: false,
            barcode_retain_split_pages: false,
            barcode_upscale: 0.0,
            barcode_dpi: 300,
            barcode_max_pages: 0,
        }
    }
}

/// All settings that ingestion needs.
#[derive(Debug, Clone)]
pub struct ConsumerSettings {
    pub media_root: PathBuf,
    pub originals_dir: PathBuf,
    pub archive_dir: PathBuf,
    pub thumbnail_dir: PathBuf,
    pub data_dir: PathBuf,
    pub scratch_dir: PathBuf,
    pub consumption_dir: PathBuf,
    pub time_zone: Tz,
    pub secret_key: String,

    pub consumer_delete_duplicates: bool,
    pub consumer_recursive: bool,
    pub consumer_subdirs_as_tags: bool,
    pub consumer_polling_interval: f64,
    pub consumer_stability_delay: f64,
    pub consumer_ignore_patterns: Vec<String>,
    pub consumer_ignore_dirs: Vec<String>,
    pub collate_double_sided: bool,
    pub collate_double_sided_subdir_name: String,
    pub collate_double_sided_tiff_support: bool,
    pub barcodes: BarcodeSettings,

    pub pre_consume_script: Option<PathBuf>,
    pub post_consume_script: Option<PathBuf>,
    pub audit_log_enabled: bool,

    pub date_order: String,
    pub filename_date_order: Option<String>,
    /// raw value of `PAPERLESS_IGNORE_DATES` (evaluated with `parse_ignore_dates`)
    pub ignore_dates_raw: Option<String>,
    pub date_parser_languages: Option<Vec<String>>,

    /// PaperRust classifier (never the pickle file from upstream, see pl-classifier §5).
    pub classifier_model_file: PathBuf,
    pub classifier_match_threshold: f64,

    pub templating: pl_templating::Options,
}

fn env(key: &str) -> Option<String> {
    pl_core::config::var(key)
}

fn env_bool(key: &str, default: &str) -> Result<bool, SettingsError> {
    str_to_bool(&env(key).unwrap_or_else(|| default.into()))
}

fn env_float(key: &str, default: f64) -> Result<f64, SettingsError> {
    match env(key) {
        None => Ok(default),
        Some(v) => v.trim().parse::<f64>().map_err(|_| SettingsError(format!("could not convert string to float: '{v}'"))),
    }
}

/// `__get_int`: `int(os.environ.get(key, default))`.
fn env_uint(key: &str, default: u32) -> Result<u32, SettingsError> {
    match env(key) {
        None => Ok(default),
        Some(v) => v.trim().parse::<u32>().map_err(|_| SettingsError(format!("invalid literal for int() with base 10: '{v}'"))),
    }
}

fn env_json_list(key: &str) -> Result<Vec<String>, SettingsError> {
    match env(key) {
        None => Ok(Vec::new()),
        Some(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(serde_json::Value::Array(a)) => {
                Ok(a.into_iter().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).collect())
            }
            Ok(_) => Err(SettingsError(format!("Environment variable {} must be a JSON list", pl_core::config::primary_name(key)))),
            Err(e) => Err(SettingsError(format!("Environment variable {} is not valid JSON: {e}", pl_core::config::primary_name(key)))),
        },
    }
}

fn env_json_map(key: &str, default: Vec<(String, String)>) -> Result<Vec<(String, String)>, SettingsError> {
    match env(key) {
        None => Ok(default),
        Some(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(serde_json::Value::Object(m)) => {
                Ok(m.into_iter().map(|(k, v)| (k, v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect())
            }
            Ok(_) => Err(SettingsError(format!("Environment variable {} must be a JSON object", pl_core::config::primary_name(key)))),
            Err(e) => Err(SettingsError(format!("Environment variable {} is not valid JSON: {e}", pl_core::config::primary_name(key)))),
        },
    }
}

impl ConsumerSettings {
    /// Default values under a root directory (tests, desktop).
    pub fn with_root(media_root: PathBuf, data_dir: PathBuf, scratch_dir: PathBuf, consumption_dir: PathBuf) -> Self {
        Self {
            originals_dir: media_root.join("documents").join("originals"),
            archive_dir: media_root.join("documents").join("archive"),
            thumbnail_dir: media_root.join("documents").join("thumbnails"),
            classifier_model_file: data_dir.join(pl_core::config::CLASSIFIER_MODEL_FILENAME),
            media_root,
            data_dir,
            scratch_dir,
            consumption_dir,
            time_zone: chrono_tz::UTC,
            secret_key: String::new(),
            consumer_delete_duplicates: false,
            consumer_recursive: false,
            consumer_subdirs_as_tags: false,
            consumer_polling_interval: 0.0,
            consumer_stability_delay: 5.0,
            consumer_ignore_patterns: Vec::new(),
            consumer_ignore_dirs: Vec::new(),
            collate_double_sided: false,
            collate_double_sided_subdir_name: "double-sided".into(),
            collate_double_sided_tiff_support: false,
            barcodes: BarcodeSettings::default(),
            pre_consume_script: None,
            post_consume_script: None,
            audit_log_enabled: true,
            date_order: "DMY".into(),
            filename_date_order: None,
            ignore_dates_raw: None,
            date_parser_languages: None,
            classifier_match_threshold: 0.3,
            templating: pl_templating::Options::default(),
        }
    }

    /// From the `PAPERLESS_*` environment variables like upstream.
    pub fn from_env() -> Result<Self, SettingsError> {
        // Installation root like pl_core::Config (`PAPERRUST_BASE_DIR`, otherwise working directory).
        // In the container the working directory is /usr/src/paperless/src, the folders are one
        // level higher – without this the worker looked for media/consume under src/ (Z6 smoke test arm64).
        let cwd = env("PAPERRUST_BASE_DIR")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let media_root = env("PAPERLESS_MEDIA_ROOT").map(PathBuf::from).unwrap_or_else(|| cwd.join("media"));
        let data_dir = env("PAPERLESS_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| cwd.join("data"));
        let scratch_dir = env("PAPERLESS_SCRATCH_DIR").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("paperrust"));
        let consumption_dir = env("PAPERLESS_CONSUMPTION_DIR").map(PathBuf::from).unwrap_or_else(|| cwd.join("consume"));
        let mut s = Self::with_root(media_root, data_dir.clone(), scratch_dir, consumption_dir);
        let tz_name = env("PAPERLESS_TIME_ZONE").unwrap_or_else(|| "UTC".into());
        s.time_zone = tz_name.parse::<Tz>().map_err(|_| SettingsError(format!("No time zone found with key {tz_name}")))?;
        s.secret_key = env("PAPERLESS_SECRET_KEY").unwrap_or_default();
        s.consumer_delete_duplicates = env_bool("PAPERLESS_CONSUMER_DELETE_DUPLICATES", "NO")?;
        s.consumer_recursive = env_bool("PAPERLESS_CONSUMER_RECURSIVE", "NO")?;
        s.consumer_subdirs_as_tags = env_bool("PAPERLESS_CONSUMER_SUBDIRS_AS_TAGS", "NO")?;
        s.consumer_polling_interval = env_float("PAPERLESS_CONSUMER_POLLING_INTERVAL", 0.0)?;
        s.consumer_stability_delay = env_float("PAPERLESS_CONSUMER_STABILITY_DELAY", 5.0)?;
        s.consumer_ignore_patterns = env_json_list("PAPERLESS_CONSUMER_IGNORE_PATTERNS")?;
        s.consumer_ignore_dirs = env_json_list("PAPERLESS_CONSUMER_IGNORE_DIRS")?;
        s.collate_double_sided = env_bool("PAPERLESS_CONSUMER_ENABLE_COLLATE_DOUBLE_SIDED", "NO")?;
        s.collate_double_sided_subdir_name =
            env("PAPERLESS_CONSUMER_COLLATE_DOUBLE_SIDED_SUBDIR_NAME").unwrap_or_else(|| "double-sided".into());
        s.collate_double_sided_tiff_support = env_bool("PAPERLESS_CONSUMER_COLLATE_DOUBLE_SIDED_TIFF_SUPPORT", "NO")?;
        s.barcodes = BarcodeSettings {
            barcodes_enabled: env_bool("PAPERLESS_CONSUMER_ENABLE_BARCODES", "NO")?,
            barcode_enable_tiff_support: env_bool("PAPERLESS_CONSUMER_BARCODE_TIFF_SUPPORT", "NO")?,
            barcode_string: env("PAPERLESS_CONSUMER_BARCODE_STRING").unwrap_or_else(|| "PATCHT".into()),
            barcode_enable_asn: env_bool("PAPERLESS_CONSUMER_ENABLE_ASN_BARCODE", "NO")?,
            barcode_asn_prefix: env("PAPERLESS_CONSUMER_ASN_BARCODE_PREFIX").unwrap_or_else(|| "ASN".into()),
            barcode_enable_tag: env_bool("PAPERLESS_CONSUMER_ENABLE_TAG_BARCODE", "NO")?,
            barcode_tag_mapping: env_json_map("PAPERLESS_CONSUMER_TAG_BARCODE_MAPPING", vec![("TAG:(.*)".into(), "\\g<1>".into())])?,
            barcode_tag_split: env_bool("PAPERLESS_CONSUMER_TAG_BARCODE_SPLIT", "NO")?,
            barcode_store_values: env_bool("PAPERLESS_CONSUMER_STORE_BARCODE_VALUES", "NO")?,
            barcode_retain_split_pages: env_bool("PAPERLESS_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES", "NO")?,
            barcode_upscale: env_float("PAPERLESS_CONSUMER_BARCODE_UPSCALE", 0.0)?,
            barcode_dpi: env_uint("PAPERLESS_CONSUMER_BARCODE_DPI", 300)?,
            barcode_max_pages: env_uint("PAPERLESS_CONSUMER_BARCODE_MAX_PAGES", 0)?,
        };
        s.pre_consume_script = env("PAPERLESS_PRE_CONSUME_SCRIPT").filter(|v| !v.is_empty()).map(PathBuf::from);
        s.post_consume_script = env("PAPERLESS_POST_CONSUME_SCRIPT").filter(|v| !v.is_empty()).map(PathBuf::from);
        s.audit_log_enabled = env_bool("PAPERLESS_AUDIT_LOG_ENABLED", "true")?;
        s.date_order = env("PAPERLESS_DATE_ORDER").unwrap_or_else(|| "DMY".into());
        s.filename_date_order = env("PAPERLESS_FILENAME_DATE_ORDER");
        s.ignore_dates_raw = env("PAPERLESS_IGNORE_DATES");
        s.date_parser_languages = match env("PAPERLESS_DATE_PARSER_LANGUAGES").filter(|v| !v.is_empty()) {
            Some(v) => Some(pl_matching::dates::parse_dateparser_languages(Some(&v)).map_err(SettingsError)?),
            None => None,
        };
        s.classifier_model_file = pl_core::config::classifier_model_file(env, &data_dir);
        s.classifier_match_threshold = env_float("PAPERLESS_CLASSIFIER_MATCH_THRESHOLD", 0.3)?;
        s.templating = pl_templating::Options::from_env().map_err(SettingsError)?;
        Ok(s)
    }

    /// `pre_check_directories`.
    pub fn ensure_directories(&self) -> std::io::Result<()> {
        for d in [&self.scratch_dir, &self.thumbnail_dir, &self.originals_dir, &self.archive_dir] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }

    /// `MEDIA_LOCK`.
    pub fn media_lock(&self) -> PathBuf {
        self.media_root.join("media.lock")
    }

    /// `Document.thumbnail_path` (`{pk:07}.webp`, versions included).
    pub fn thumbnail_path(&self, pk: i64) -> PathBuf {
        self.thumbnail_dir.join(format!("{pk:07}.webp"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bools() {
        assert_eq!(str_to_bool("Yes"), Ok(true));
        assert_eq!(str_to_bool(" 0 "), Ok(false));
        assert_eq!(str_to_bool("x").unwrap_err().0, "Cannot convert 'x' to a boolean.");
    }

    #[test]
    fn barcode_app_config_precedence() {
        let env = BarcodeSettings { barcode_dpi: 200, barcode_string: "SEP".into(), barcodes_enabled: true, ..Default::default() };
        // NULL everywhere → environment
        assert_eq!(env.with_app_config(&BarcodeAppConfig::default()), env);
        let c = BarcodeAppConfig {
            barcodes_enabled: Some(false),
            barcode_string: Some(String::new()),
            barcode_dpi: Some(600),
            barcode_upscale: Some(1.5),
            barcode_max_pages: Some(0),
            barcode_tag_mapping: Some(serde_json::json!({"X(.*)": "\\g<1>", "A": ""})),
            ..Default::default()
        };
        let s = env.with_app_config(&c);
        assert!(!s.barcodes_enabled, "False aus der DB gilt (is not None)");
        assert_eq!(s.barcode_string, "SEP", "leerer Text → Umgebung (or)");
        assert_eq!((s.barcode_dpi, s.barcode_upscale, s.barcode_max_pages), (600, 1.5, 0));
        assert_eq!(s.barcode_tag_mapping, vec![("X(.*)".to_string(), "\\g<1>".to_string()), ("A".to_string(), String::new())]);
        let sc = s.scan_settings();
        assert_eq!((sc.dpi, sc.upscale, sc.max_pages), (600, 1.5, 0));
        // empty object → environment
        let e = env.with_app_config(&BarcodeAppConfig { barcode_tag_mapping: Some(serde_json::json!({})), ..Default::default() });
        assert_eq!(e.barcode_tag_mapping, env.barcode_tag_mapping);
        assert!(s.with_app_config(&BarcodeAppConfig { barcodes_enabled: Some(true), ..Default::default() }).able_to_run("application/pdf"));
        assert!(!env.able_to_run("image/tiff"));
    }

    #[test]
    fn paths() {
        let s = ConsumerSettings::with_root("m".into(), "d".into(), "s".into(), "c".into());
        assert_eq!(s.thumbnail_path(12), PathBuf::from("m/documents/thumbnails/0000012.webp"));
        assert_eq!(s.originals_dir, PathBuf::from("m/documents/originals"));
    }
}
