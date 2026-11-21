//! pl-templating – filename, storage path and workflow templates like paperless-ngx v3.4.2.
//!
//! Pure library (no DB, no routes). Contract: docs/contract/pl-templating.md.
//!
//! upstream: src/documents/templating/{environment,filters,filepath,utils,workflows}.py@v3.4.2,
//! src/documents/file_handling.py@v3.4.2.
//!
//! Entry points:
//! * [`generate_filename`] / [`generate_unique_filename`] – filename of a document
//! * [`format_filename`] / [`render_filename`] – render a template against a document
//! * [`validate_filepath_template_and_render`] – check a storage path (document optional)
//! * [`storage_path_test_result`] – `POST /api/storage_paths/test/`
//! * [`parse_w_workflow_placeholders`] / [`render_workflow_title`] / [`validate_workflow_template`]

#[allow(clippy::invisible_characters)]
mod cldr_data;
mod context;
mod convert;
mod env;
mod error;
mod extras;
mod file_handling;
mod filepath;
mod filters;
mod localize;
mod methods;
mod pathvalidate;
mod pylist;
mod pystr;
mod pyvalue;
mod strftime;
mod workflows;

#[cfg(test)]
mod tests;

pub use context::{CustomFieldInstance, CustomFieldValue, DocumentContext, NamedRef, OwnerRef, SelectOption, StoragePathRef, TagRef};
pub use convert::convert_format_str_to_template_format;
pub use env::{FUEL, MAX_OUTPUT_BYTES, MAX_TEMPLATE_BYTES};
pub use error::{TemplateError, UnsafeFilePathError, WorkflowTemplateError};
pub use file_handling::{
    GenerateOptions, MAX_STORED_FILENAME_LENGTH, MAX_UNIQUE_ATTEMPTS, create_source_path_directory, delete_empty_directories,
    format_filename, generate_filename, generate_unique_filename, storage_path_test_response, storage_path_test_result,
    validate_path_in_root,
};
pub use filepath::{clean_filepath, is_safe_relative_path, validate_filepath_template_and_render};
pub use pathvalidate::sanitize_filename;
pub use pystr::django_slugify;
pub use workflows::{
    KNOWN_PLACEHOLDER_NAMES, WorkflowDocId, WorkflowPlaceholders, parse_w_workflow_placeholders, parse_w_workflow_placeholders_with_id,
    render_workflow_title, validate_workflow_template,
};

/// Settings that upstream reads from `django.conf.settings`.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// `PAPERLESS_FILENAME_FORMAT` (raw; old `{}` syntax is converted on use).
    pub filename_format: Option<String>,
    /// `PAPERLESS_FILENAME_FORMAT_REMOVE_NONE`.
    pub filename_format_remove_none: bool,
    /// `PAPERLESS_TIME_ZONE` (for `timezone.localdate(document.added)`).
    pub time_zone: chrono_tz::Tz,
}

impl Default for Options {
    fn default() -> Self {
        Options { filename_format: None, filename_format_remove_none: false, time_zone: chrono_tz::UTC }
    }
}

impl Options {
    /// From the environment variables like `paperless/settings` (invalid values -> error text like upstream).
    pub fn from_env() -> Result<Options, String> {
        let filename_format = pl_envlive::var("PAPERLESS_FILENAME_FORMAT");
        let raw = pl_envlive::var("PAPERLESS_FILENAME_FORMAT_REMOVE_NONE").unwrap_or_else(|| "NO".into());
        let remove_none = match raw.trim().to_lowercase().as_str() {
            "true" | "1" | "t" | "y" | "yes" => true,
            "false" | "0" | "f" | "n" | "no" => false,
            _ => return Err(format!("Cannot convert '{raw}' to a boolean.")),
        };
        let tz_name = pl_envlive::var("PAPERLESS_TIME_ZONE").unwrap_or_else(|| "UTC".into());
        let time_zone = tz_name.parse().map_err(|_| format!("Incorrect timezone setting: {tz_name}"))?;
        Ok(Options { filename_format, filename_format_remove_none: remove_none, time_zone })
    }
}

/// Render a template against a document (= [`format_filename`]: incl. REMOVE_NONE and "none"
/// replacement, without file extension). For pure validation see [`validate_filepath_template_and_render`].
pub fn render_filename(template: &str, ctx: &DocumentContext, opts: &Options) -> Result<String, TemplateError> {
    format_filename(ctx, template, opts)
}

/// `get_default_file_extension(mime_type)` for the default parsers (Tesseract, Text, Mail, Tika)
/// and a small `mimetypes` table; "" if unknown.
///
/// upstream: src/documents/parsers.py@v3.4.2, src/paperless/parsers/{tesseract,text,mail,tika}.py
pub fn default_file_extension(mime_type: &str) -> &'static str {
    match mime_type {
        "application/pdf" => ".pdf",
        "image/jpeg" => ".jpg",
        "image/png" => ".png",
        "image/tiff" => ".tif",
        "image/gif" => ".gif",
        "image/bmp" => ".bmp",
        "image/webp" => ".webp",
        "image/heic" => ".heic",
        "text/plain" => ".txt",
        "text/csv" | "application/csv" => ".csv",
        "message/rfc822" => ".eml",
        "application/msword" => ".doc",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => ".docx",
        "application/vnd.ms-excel" => ".xls",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => ".xlsx",
        "application/vnd.ms-powerpoint" => ".ppt",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => ".pptx",
        "application/vnd.openxmlformats-officedocument.presentationml.slideshow" => ".ppsx",
        "application/vnd.oasis.opendocument.presentation" => ".odp",
        "application/vnd.oasis.opendocument.spreadsheet" => ".ods",
        "application/vnd.oasis.opendocument.text" => ".odt",
        "application/vnd.oasis.opendocument.graphics" => ".odg",
        "text/rtf" => ".rtf",
        "text/html" => ".html",
        "application/json" => ".json",
        "application/zip" => ".zip",
        "image/svg+xml" => ".svg",
        _ => "",
    }
}
