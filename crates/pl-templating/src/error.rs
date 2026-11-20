//! Error types.
//!
//! upstream: Jinja2 exceptions that src/documents/templating/filepath.py@v3.4.2 and
//! workflows.py@v3.4.2 distinguish (UndefinedError, TemplateSyntaxError, SecurityError, Exception),
//! as well as documents.file_handling.UnsafeFilePathError.

/// Error when rendering a template.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TemplateError {
    /// `jinja2.UndefinedError` (undefined variable/attribute/method).
    #[error("Template variable error: {0}")]
    Undefined(String),
    /// `jinja2.TemplateSyntaxError` / `TemplateAssertionError` (e.g. unknown filter).
    #[error("Template syntax error: {0}")]
    Syntax(String),
    /// `jinja2.sandbox.SecurityError` or a sandbox limit (size, runtime).
    #[error("Template attempted restricted operation: {0}")]
    Security(String),
    /// Any other exception when rendering (e.g. ValueError from a filter).
    #[error("Unknown error in template rendering: {0}")]
    Other(String),
    /// The result is absolute or contains ".." (upstream: `is_safe_relative_path` -> None).
    #[error("Template rendered an unsafe path (absolute or containing traversal).")]
    UnsafePath,
    /// Error while building the context (upstream: exception outside the try block).
    #[error("{0}")]
    Context(String),
}

/// `documents.file_handling.UnsafeFilePathError`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct UnsafeFilePathError(pub String);

/// Error from `validate_workflow_template` (upstream: ValueError or KeyError, text = `e.args[0]`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkflowTemplateError {
    /// "Template syntax error: ..." or "Template assertion error: ..."
    #[error("{0}")]
    ValueError(String),
    /// "Template references unknown placeholders: ..."
    #[error("{0}")]
    KeyError(String),
}

impl WorkflowTemplateError {
    /// Text that the serializer returns as a field error (`f"{e.args[0]}"`).
    pub fn message(&self) -> &str {
        match self {
            WorkflowTemplateError::ValueError(m) | WorkflowTemplateError::KeyError(m) => m,
        }
    }
}
