// upstream: src/paperless/settings/__init__.py@v3.4.2 (REMOTE_OCR_*), pathlib.Path.resolve
//
// All paths (MEDIA_ROOT, ORIGINALS_DIR, ..., SCRATCH_DIR, STATIC_ROOT, LOGIN_URL, APP_LOGO,
// TIME_ZONE, REMOTE_OCR_*) come from `pl_core::Config`.

use std::path::{Path, PathBuf};

use pl_core::Config;

/// `settings.REMOTE_OCR_ENGINE / _API_KEY / _ENDPOINT` (`os.getenv`, so `""` stays `Some("")`).
pub fn remote_ocr_env(cfg: &Config) -> (Option<String>, Option<String>, Option<String>) {
    (cfg.remote_ocr_engine.clone(), cfg.remote_ocr_api_key.clone(), cfg.remote_ocr_endpoint.clone())
}

/// `Path.resolve()` without requiring existence: canonicalize if possible, otherwise keep the
/// (absolute) path.
pub fn resolve(p: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(p) {
        return strip_verbatim(c);
    }
    if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf()) }
}

/// On Windows `canonicalize` yields `\\?\C:\...`; keep paths comparable with joined ones.
pub fn strip_verbatim(p: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\")
            && !rest.starts_with("UNC\\")
        {
            return PathBuf::from(rest.to_string());
        }
    }
    p
}
