//! `document_exporter` / `document_importer` in the upstream format (v3.4.2).
//!
//! The format is Django's "python" serializer (`model`, `pk`, `fields`) + `DjangoJSONEncoder`,
//! written like `documents/export/sinks.py`. Which models, fields (order, columns,
//! m2m queries) and queries (WHERE/ORDER BY) exist is defined in the embedded descriptor
//! (`baseline/export-descriptor.json`, generated from Django `_meta`) – so field order
//! and record order match upstream without hand-building the 40 models.

pub mod descriptor;
pub mod exporter;
pub mod extras;
pub mod fernet;
pub mod importer;
pub mod medialock;
pub mod pypath;
pub mod safepath;
pub mod tagtree;
pub mod value;
pub mod zip;

pub const EXPORTER_FILE_NAME: &str = "__exported_file_name__";
pub const EXPORTER_THUMBNAIL_NAME: &str = "__exported_thumbnail_name__";
pub const EXPORTER_ARCHIVE_NAME: &str = "__exported_archive_name__";
pub const EXPORTER_SHARE_LINK_BUNDLE_NAME: &str = "__exported_share_link_bundle_name__";
pub const EXPORTER_CRYPTO_SETTINGS_NAME: &str = "__crypto__";
pub const EXPORTER_CRYPTO_SALT_NAME: &str = "__salt_hex__";
pub const EXPORTER_CRYPTO_KEY_ITERATIONS_NAME: &str = "__key_iters__";
pub const EXPORTER_CRYPTO_KEY_SIZE_NAME: &str = "__key_size__";
pub const EXPORTER_CRYPTO_ALGO_NAME: &str = "__key_algo__";

/// `paperless.version.__full_version_str__`.
pub fn version() -> &'static str {
    &descriptor::get().version
}

/// `shutil.copy` + `copy_basic_file_stats` (atime/mtime).
pub fn copy_file_with_basic_stats(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::copy(src, dst)?;
    let meta = std::fs::metadata(src)?;
    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(dst) {
        let mut times = std::fs::FileTimes::new();
        if let Ok(m) = meta.modified() {
            times = times.set_modified(m);
        }
        if let Ok(a) = meta.accessed() {
            times = times.set_accessed(a);
        }
        let _ = f.set_times(times);
    }
    Ok(())
}
