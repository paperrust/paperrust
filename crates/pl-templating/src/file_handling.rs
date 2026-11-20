//! Generate filenames (storage path / PAPERLESS_FILENAME_FORMAT), collisions, path protection.
//!
//! upstream: src/documents/file_handling.py@v3.4.2 (validate_path_in_root,
//! create_source_path_directory, delete_empty_directories, generate_unique_filename,
//! format_filename, generate_filename); src/documents/views.py@v3.4.2 (StoragePathViewSet.test).

use std::path::{Component, Path, PathBuf};

use crate::context::DocumentContext;
use crate::convert::convert_format_str_to_template_format;
use crate::error::{TemplateError, UnsafeFilePathError};
use crate::filepath::{is_safe_relative_path, validate_filepath_template_and_render};
use crate::pystr::{name_suffix, path_join, path_name, path_parent, path_stem, path_str};
use crate::Options;

/// `Document.MAX_STORED_FILENAME_LENGTH` (length of the columns `filename`/`archive_filename`).
pub const MAX_STORED_FILENAME_LENGTH: usize = 1024;

/// `format_filename(document, template_str)`.
///
/// `Err` corresponds to upstream `None` (reason in the error), except `TemplateError::Context`.
pub fn format_filename(document: &DocumentContext, template_str: &str, opts: &Options) -> Result<String, TemplateError> {
    let mut rendered = validate_filepath_template_and_render(template_str, Some(document), opts)?;

    // Apply this setting.  It could become a filter in the future (or users could use |default)
    if opts.filename_format_remove_none {
        rendered = rendered.replace("/-none-/", "/");
        rendered = rendered.replace(" -none-", "");
        rendered = rendered.replace("-none-", "");
        rendered = rendered.trim_matches('/').to_string();
    }

    rendered = rendered.replace("-none-", "none"); // backward compatibility

    // Validate again after remove none
    if !is_safe_relative_path(&rendered) {
        tracing::warn!(
            target: "paperless.filehandling",
            "Filename became unsafe after placeholder removal, falling back to default naming"
        );
        return Err(TemplateError::UnsafePath);
    }
    if cfg!(windows) {
        // Security review 2026-11-19 F12: stream/device names, trailing dots of folders
        rendered = crate::filepath::windows_safe_parts(&rendered);
    }
    Ok(rendered)
}

/// Parameters of `generate_filename`.
#[derive(Debug, Clone, Copy)]
pub struct GenerateOptions {
    pub counter: u32,
    pub archive_filename: bool,
    pub use_format: bool,
}

impl Default for GenerateOptions {
    fn default() -> Self {
        GenerateOptions { counter: 0, archive_filename: false, use_format: true }
    }
}

/// `generate_filename(doc, counter=0, archive_filename=False, use_format=True)`.
///
/// Returns a relative POSIX path. `Err` only for `TemplateError::Context` (upstream: exception).
pub fn generate_filename(doc: &DocumentContext, gopt: GenerateOptions, opts: &Options) -> Result<String, TemplateError> {
    // version docs use the root document for formatting, just with a suffix
    let context_doc: &DocumentContext = doc.root_document.as_deref().unwrap_or(doc);
    let version_suffix = match (&doc.root_document, doc.version_index) {
        (Some(_), Some(v)) => format!("_v{v}"),
        _ => String::new(),
    };

    let filename_format: Option<String> = if gopt.use_format {
        if let Some(sp) = &context_doc.storage_path {
            Some(sp.path.clone())
        } else {
            opts.filename_format.as_deref().map(convert_format_str_to_template_format)
        }
    } else {
        None
    };

    let mut base_path: Option<String> = None;
    if let Some(fmt) = filename_format {
        match format_filename(context_doc, &fmt, opts) {
            Ok(r) if !r.is_empty() => base_path = Some(path_str(&r)),
            Ok(_) => {}
            Err(e @ TemplateError::Context(_)) => return Err(e),
            Err(_) => {}
        }
    }

    let counter_str = if gopt.counter != 0 { format!("_{:02}", gopt.counter) } else { String::new() };
    let filetype_str = if gopt.archive_filename { ".pdf" } else { doc.file_type.as_str() };

    Ok(match base_path {
        Some(base) => {
            let directory = path_parent(&base);
            let base_filename = path_name(&base);
            let final_filename = format!("{base_filename}{version_suffix}{counter_str}{filetype_str}");
            if directory != "." {
                path_join(&directory, &final_filename)
            } else {
                final_filename
            }
        }
        None => format!("{:07}{version_suffix}{counter_str}{filetype_str}", context_doc.pk),
    })
}

/// Maximum number of counter attempts in `generate_unique_filename` (upstream: unlimited).
pub const MAX_UNIQUE_ATTEMPTS: u32 = 1_000_000;

fn same_path(a: &str, b: Option<&str>) -> bool {
    b.is_some_and(|b| path_str(a) == path_str(b))
}

/// `generate_unique_filename(doc, archive_filename=...)`.
///
/// `exists(rel)` checks whether `root / rel` exists (root = ORIGINALS_DIR or ARCHIVE_DIR).
pub fn generate_unique_filename(
    doc: &DocumentContext,
    archive_filename: bool,
    opts: &Options,
    mut exists: impl FnMut(&str) -> bool,
) -> Result<String, TemplateError> {
    let old_filename: Option<&str> = if archive_filename {
        doc.archive_filename.as_deref().filter(|s| !s.is_empty())
    } else {
        doc.filename.as_deref().filter(|s| !s.is_empty())
    };

    let base_filename = generate_filename(doc, GenerateOptions { archive_filename, ..Default::default() }, opts)?;

    // If generating archive filenames, try to make a name that is similar to
    // the original filename first.
    if archive_filename
        && let Some(fname) = doc.filename.as_deref().filter(|s| !s.is_empty()) {
            let parent = path_parent(&base_filename);
            let simple_pdf_name = if parent != "." {
                path_join(&parent, &format!("{}.pdf", path_stem(fname)))
            } else {
                path_str(&format!("{}.pdf", path_stem(fname)))
            };
            if same_path(&simple_pdf_name, old_filename) || !exists(&simple_pdf_name) {
                return Ok(simple_pdf_name);
            }
        }

    let file_extension = if archive_filename { ".pdf" } else { doc.file_type.as_str() };
    let base_name = path_name(&base_filename).to_string();
    let filename_stem = base_name.strip_suffix(file_extension).unwrap_or(&base_name).to_string();
    let parent = path_parent(&base_filename);
    let mut counter: u32 = 0;
    loop {
        let new_filename = if counter != 0 {
            let name = format!("{filename_stem}_{counter:02}{file_extension}");
            if parent != "." {
                path_join(&parent, &name)
            } else {
                name
            }
        } else {
            base_filename.clone()
        };
        if same_path(&new_filename, old_filename) {
            // still the same as before.
            return Ok(new_filename);
        }
        if exists(&new_filename) {
            counter += 1;
            if counter > MAX_UNIQUE_ATTEMPTS {
                return Err(TemplateError::Other("no free filename found".into()));
            }
        } else {
            return Ok(new_filename);
        }
    }
}

/// Result of `POST /api/storage_paths/test/` (`StoragePathViewSet.test`): path with extension or
/// `None` (JSON `null`). The template is – like upstream – not converted from `{}` to Jinja.
pub fn storage_path_test_result(document: &DocumentContext, path: &str, opts: &Options) -> Result<Option<String>, TemplateError> {
    Ok(storage_path_test_response(document, path, opts)?.filter(|r| !r.is_empty()))
}

/// Response of `StoragePathViewSet.test` exactly like upstream (views.py: `result = format_filename(..)`,
/// `if result:` append extension, `Response(result)`): `Some("")` for an empty result (JSON `""`),
/// `None` on a render error (`Response(None)`). The template is rendered **once** here
/// (security review final B-N1: previously twice per request).
pub fn storage_path_test_response(document: &DocumentContext, path: &str, opts: &Options) -> Result<Option<String>, TemplateError> {
    let result = match format_filename(document, path, opts) {
        Ok(r) => r,
        Err(e @ TemplateError::Context(_)) => return Err(e),
        Err(_) => return Ok(None),
    };
    if result.is_empty() {
        return Ok(Some(result));
    }
    let mut extension = document.filename.as_deref().filter(|f| !f.is_empty()).map(|f| name_suffix(path_name(f))).unwrap_or("");
    if extension.is_empty() {
        extension = document.file_type.as_str();
    }
    let rp = path_str(&result);
    let name = format!("{}{extension}", path_name(&rp));
    let parent = path_parent(&rp);
    Ok(Some(if parent != "." { path_join(&parent, &name) } else { name }))
}

/// Normalize the path lexically and canonicalize the longest existing prefix
/// (like `Path.resolve()` with strict=False).
fn resolve(path: &Path) -> PathBuf {
    let base = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|d| d.join(path)).unwrap_or_else(|_| path.to_path_buf())
    };
    let mut out = PathBuf::new();
    for c in base.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
        if let Ok(real) = std::fs::canonicalize(&out) {
            out = real;
        }
    }
    out
}

/// `validate_path_in_root(path, root)`.
pub fn validate_path_in_root(path: &Path, root: &Path) -> Result<(), UnsafeFilePathError> {
    if !resolve(path).starts_with(resolve(root)) {
        let msg = format!("Refusing to write file outside of root {}: {}.", root.display(), path.display());
        tracing::warn!(target: "paperless.filehandling", "{msg}");
        return Err(UnsafeFilePathError(msg));
    }
    Ok(())
}

/// `create_source_path_directory(source_path)`.
pub fn create_source_path_directory(source_path: &Path) -> std::io::Result<()> {
    match source_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => std::fs::create_dir_all(p),
        _ => Ok(()),
    }
}

/// `delete_empty_directories(directory, root)`.
pub fn delete_empty_directories(directory: &Path, root: &Path) {
    if !directory.is_dir() {
        return;
    }
    if !directory.starts_with(root) {
        // don't do anything outside our originals folder.
        return;
    }
    let mut dir = directory.to_path_buf();
    while dir != root {
        let empty = match std::fs::read_dir(&dir) {
            Ok(mut it) => it.next().is_none(),
            Err(_) => return,
        };
        if !empty {
            return;
        }
        if std::fs::remove_dir(&dir).is_err() {
            return;
        }
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_in_root() {
        let root = std::env::temp_dir().join("pl_templating_root_test");
        let _ = std::fs::create_dir_all(&root);
        assert!(validate_path_in_root(&root.join("0000001.pdf"), &root).is_ok());
        assert!(validate_path_in_root(&root.join("..").join("..").join("pwned.pdf"), &root).is_err());
        let _ = std::fs::remove_dir(&root);
    }

    #[test]
    fn delete_empty() {
        let root = std::env::temp_dir().join("pl_templating_delete_test");
        let tmp = root.join("test_delete_empty");
        std::fs::create_dir_all(tmp.join("notempty").join("empty")).unwrap();
        std::fs::write(tmp.join("notempty").join("file"), b"").unwrap();
        delete_empty_directories(&tmp.join("notempty").join("empty"), &root);
        assert!(tmp.join("notempty").is_dir());
        assert!(tmp.join("notempty").join("file").is_file());
        assert!(!tmp.join("notempty").join("empty").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
