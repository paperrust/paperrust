//! Rendering of filename/storage path templates.
//!
//! upstream: src/documents/templating/filepath.py@v3.4.2 (FilePathTemplate.render/clean_filepath,
//! is_safe_relative_path, validate_filepath_template_and_render).

use unicode_normalization::UnicodeNormalization;

use crate::context::{dummy_document, filepath_context, Created, DocumentContext};
use crate::error::TemplateError;
use crate::pystr::{path_parts, py_isspace, py_strip};
use crate::Options;

/// `FilePathTemplate.render` -> `clean_filepath`.
pub fn clean_filepath(value: &str) -> String {
    let value: String = value.nfc().collect();
    let value = value.replace(['\n', '\r'], "");
    // re.sub(r"\s*/\s*", "/", value)
    let mut out = String::with_capacity(value.len());
    let mut pending_ws = String::new();
    let mut after_slash = false;
    for c in value.chars() {
        if c == '/' {
            pending_ws.clear();
            out.push('/');
            after_slash = true;
        } else if py_isspace(c) {
            if !after_slash {
                pending_ws.push(c);
            }
        } else {
            out.push_str(&pending_ws);
            pending_ws.clear();
            out.push(c);
            after_slash = false;
        }
    }
    out.push_str(&pending_ws);
    // value.strip().strip(os.sep)
    py_strip(&out).trim_matches('/').to_string()
}

/// `is_safe_relative_path`: no absolute path, no "..". PaperRust additionally (review H1): no
/// backslash (a separator on Windows – `..\x` led out there; pl-documents-write reads
/// stored names with `\` as a separator) and no NUL.
pub fn is_safe_relative_path(value: &str) -> bool {
    if value.contains(['\\', '\0']) {
        return false;
    }
    if value.is_empty() {
        return true;
    }
    let (absolute, parts) = path_parts(value);
    if absolute {
        return false;
    }
    !parts.contains(&"..")
}

/// `validate_filepath_template_and_render(template_string, document)`.
///
/// `document = None` renders against the placeholder document of upstream (validation when saving
/// a storage path). Upstream returns `None` on every error; here the reason is in the `Err`.
/// `TemplateError::Context` corresponds to an exception that upstream does not catch.
/// Maximum length of a rendered path (Windows: 32,767 characters; Linux PATH_MAX 4096).
pub const MAX_RENDERED_PATH_BYTES: usize = 32 * 1024;

pub fn validate_filepath_template_and_render(
    template_string: &str,
    document: Option<&DocumentContext>,
    opts: &Options,
) -> Result<String, TemplateError> {
    let ctx = match document {
        Some(doc) => filepath_context(doc, Created::Date(doc.created), &opts.time_zone)?,
        None => {
            let (doc, created) = dummy_document();
            filepath_context(&doc, created, &opts.time_zone)?
        }
    };
    let rendered = match crate::env::render(template_string, ctx) {
        // Security review 2026-11-18 (fuzzing, finding S-8b): a path over 32 KiB is valid on no
        // file system; without a limit, e.g. the storage path test returned a 16 MB response and
        // seconds of compute time for a tiny request. Check before clean_filepath (saves the work).
        Ok(r) if r.len() > MAX_RENDERED_PATH_BYTES => {
            tracing::warn!(target: "paperless.templating", "Template rendered a path longer than {MAX_RENDERED_PATH_BYTES} bytes");
            return Err(TemplateError::Security("rendered path too long".into()));
        }
        Ok(r) => clean_filepath(&r),
        Err(e) => {
            match &e {
                TemplateError::Undefined(m) => {
                    tracing::warn!(target: "paperless.templating", "Template variable warning: {m}")
                }
                TemplateError::Syntax(m) => {
                    tracing::warn!(target: "paperless.templating", "Template syntax error in filename generation: {m}")
                }
                TemplateError::Security(m) => {
                    tracing::warn!(target: "paperless.templating", "Template attempted restricted operation: {m}")
                }
                other => {
                    tracing::warn!(target: "paperless.templating", "Unknown error in filename generation: {other}");
                    tracing::warn!(
                        target: "paperless.templating",
                        "Invalid filename_format '{template_string}', falling back to default"
                    );
                }
            }
            return Err(e);
        }
    };
    if !is_safe_relative_path(&rendered) {
        tracing::warn!(target: "paperless.templating", "Template rendered an unsafe path (absolute or containing traversal).");
        return Err(TemplateError::UnsafePath);
    }
    Ok(rendered)
}

/// Security review 2026-11-19 F12 (Windows only, called by `format_filename` after the last
/// security check): sanitize parts that Win32 reinterprets instead of rejecting them.
/// * `:` (drive/alternate data stream `name:stream`) → `-` (like pathvalidate for values),
/// * device name (`CON`, `nul.tar`, `COM1`, `lpt1 .txt`) → `_` after the name (`CON_`, `nul_.tar`),
/// * directory parts: trailing dots/spaces removed, empty → `-`.
///
/// The last part keeps trailing dots, because the extension is still appended (`x.` + `.pdf`).
/// Other characters forbidden on Windows (`<>"|?*`) stay as in upstream (creating the file
/// then fails, but no different path results).
pub(crate) fn windows_safe_parts(rendered: &str) -> String {
    let parts: Vec<&str> = rendered.split('/').collect();
    let last = parts.len().saturating_sub(1);
    parts
        .iter()
        .enumerate()
        .map(|(i, part)| {
            if part.is_empty() || *part == "." || *part == ".." {
                return part.to_string();
            }
            let mut s = part.replace(':', "-");
            if i != last {
                s = s.trim_end_matches(['.', ' ']).to_string();
                if s.is_empty() {
                    s = "-".into();
                }
            }
            let first = s.split('.').next().unwrap_or("");
            let stem_len = first.trim_end_matches(' ').len();
            if crate::pathvalidate::is_reserved(&s[..stem_len]) {
                s.insert(stem_len, '_');
            }
            s
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlong_rendered_path_rejected() {
        let opts = Options::default();
        let r = validate_filepath_template_and_render("{{ 'a' * 100000 }}", None, &opts);
        assert!(matches!(r, Err(TemplateError::Security(_))), "{r:?}");
        let ok = validate_filepath_template_and_render("{{ 'a' * 200 }}/x", None, &opts);
        assert!(ok.is_ok(), "{ok:?}");
    }

    /// Security review 2026-11-19 F12: literal text of the template can produce stream names
    /// (`name:stream`), device names and trailing dots/spaces on Windows (placeholder values are already
    /// sanitized by pathvalidate). Sanitizing instead of an error; called only on Windows (`format_filename`).
    #[test]
    fn windows_reserved_parts_sanitized() {
        let cases = [
            ("a:b/x.pdf:strom", "a-b/x.pdf-strom"),
            ("CON/nul.tar/Com1", "CON_/nul_.tar/Com1_"),
            ("abc./x..", "abc/x.."),
            ("a/.../b", "a/-/b"),
            ("lpt1 .txt/y", "lpt1_ .txt/y"),
            ("Rechnung März/console/COM10", "Rechnung März/console/COM10"),
        ];
        for (input, want) in cases {
            assert_eq!(windows_safe_parts(input), want, "{input:?}");
        }
    }

    #[test]
    fn clean() {
        assert_eq!(clean_filepath("\n  somepath/\n  some where/2020 \n /x \n"), "somepath/some where/2020/x");
        assert_eq!(clean_filepath(" / a / b / "), "a/b");
        assert_eq!(clean_filepath("//etc/something/doc1"), "etc/something/doc1");
    }

    #[test]
    fn safe() {
        assert!(is_safe_relative_path(""));
        assert!(is_safe_relative_path("a/b"));
        assert!(is_safe_relative_path("./a"));
        assert!(!is_safe_relative_path("/a"));
        assert!(!is_safe_relative_path("a/../b"));
        assert!(is_safe_relative_path("a/..b"));
        // Review H1
        assert!(!is_safe_relative_path("..\\archive\\0000042"));
        assert!(!is_safe_relative_path("a\\b"));
        assert!(!is_safe_relative_path("a\0b"));
        assert!(!is_safe_relative_path(&clean_filepath(" ..\\archive\\0000042 ")));
        assert!(validate_filepath_template_and_render("..\\archive\\0000042", None, &Options::default()).is_err());
    }
}
