//! Validation of workflow title templates.
//! upstream: src/documents/templating/workflows.py@v3.4.2 (`validate_workflow_template`,
//! `_known_placeholder_names`), src/documents/templating/environment.py@v3.4.2
//! (`JinjaEnvironment`: trim_blocks, lstrip_blocks, loopcontrols), src/documents/templating/filepath.py
//! (additional filters on the same environment). Jinja2 3.1.6.

pub mod ast;
pub mod compiler;
pub mod lexer;
pub mod parser;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JKind {
    /// TemplateSyntaxError while parsing (translated → message only)
    Syntax,
    /// TemplateAssertionError while parsing (translated → message only)
    ParseAssertion,
    /// TemplateAssertionError from the code generator (not translated → with location)
    CompileAssertion,
    /// Python AssertionError or similar → upstream 500
    Internal,
}

#[derive(Debug, Clone)]
pub struct JErr {
    pub msg: String,
    pub lineno: usize,
    pub kind: JKind,
}

impl JErr {
    pub fn syntax(msg: impl Into<String>, lineno: usize) -> Self {
        JErr { msg: msg.into(), lineno, kind: JKind::Syntax }
    }
    pub fn parse_assertion(msg: impl Into<String>, lineno: usize) -> Self {
        JErr { msg: msg.into(), lineno, kind: JKind::ParseAssertion }
    }
    pub fn compile_assertion(msg: impl Into<String>, lineno: usize) -> Self {
        JErr { msg: msg.into(), lineno, kind: JKind::CompileAssertion }
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        JErr { msg: msg.into(), lineno: 0, kind: JKind::Internal }
    }
}

/// Jinja default filters (`jinja2.filters.FILTERS`) + paperless (`get_cf_value`, `datetime`,
/// `slugify`, `localize_date`).
pub const FILTERS: &[&str] = &[
    "abs", "attr", "batch", "capitalize", "center", "count", "d", "default", "dictsort", "e", "escape",
    "filesizeformat", "first", "float", "forceescape", "format", "groupby", "indent", "int", "items", "join",
    "last", "length", "list", "lower", "map", "max", "min", "pprint", "random", "reject", "rejectattr", "replace",
    "reverse", "round", "safe", "select", "selectattr", "slice", "sort", "string", "striptags", "sum", "title",
    "tojson", "trim", "truncate", "unique", "upper", "urlencode", "urlize", "wordcount", "wordwrap", "xmlattr",
    "get_cf_value", "datetime", "slugify", "localize_date",
];

/// `jinja2.tests.TESTS`
pub const TESTS: &[&str] = &[
    "!=", "<", "<=", "==", ">", ">=", "boolean", "callable", "defined", "divisibleby", "eq", "equalto", "escaped",
    "even", "false", "filter", "float", "ge", "greaterthan", "gt", "in", "integer", "iterable", "le", "lessthan",
    "lower", "lt", "mapping", "ne", "none", "number", "odd", "sameas", "sequence", "string", "test", "true",
    "undefined", "upper",
];

/// `jinja2.defaults.DEFAULT_NAMESPACE`
pub const GLOBALS: &[&str] = &["range", "dict", "lipsum", "cycler", "joiner", "namespace"];

pub const KNOWN_PLACEHOLDERS: &[&str] = &[
    "correspondent",
    "document_type",
    "added",
    "added_year",
    "added_year_short",
    "added_month",
    "added_month_name",
    "added_month_name_short",
    "added_day",
    "added_time",
    "owner_username",
    "original_filename",
    "filename",
    "created",
    "created_year",
    "created_year_short",
    "created_month",
    "created_month_name",
    "created_month_name_short",
    "created_day",
    "created_time",
    "doc_title",
    "doc_url",
    "doc_id",
    "custom_fields",
];

/// Result of the check.
#[derive(Debug, PartialEq, Eq)]
pub enum TemplateError {
    /// Text for `{"assign_title": [...]}`
    Invalid(String),
    /// upstream: unhandled exception → 500
    Internal(String),
}

/// `validate_workflow_template(text)`
pub fn validate_template(text: &str) -> Result<(), TemplateError> {
    let fmt = |e: JErr| -> TemplateError {
        match e.kind {
            JKind::Syntax => TemplateError::Invalid(format!("Template syntax error: {}", e.msg)),
            JKind::ParseAssertion => TemplateError::Invalid(format!("Template assertion error: {}", e.msg)),
            JKind::CompileAssertion => TemplateError::Invalid(format!(
                "Template assertion error: {}\n  File \"<introspection>\", line {}",
                e.msg, e.lineno
            )),
            JKind::Internal => TemplateError::Internal(e.msg),
        }
    };
    let tpl = parser::Parser::new(text).and_then(|p| p.parse()).map_err(fmt)?;
    let env = compiler::Env { filters: FILTERS, tests: TESTS, globals: GLOBALS };
    let undeclared = compiler::find_undeclared_variables(&tpl, &env).map_err(fmt)?;
    let mut unknown: Vec<&String> = undeclared.iter().filter(|v| !KNOWN_PLACEHOLDERS.contains(&v.as_str())).collect();
    if !unknown.is_empty() {
        unknown.sort();
        let names: Vec<&str> = unknown.iter().map(|s| s.as_str()).collect();
        return Err(TemplateError::Invalid(format!(
            "Template references unknown placeholders: {}",
            names.join(", ")
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
