// whoosh_compat/errors.py (whoosh-compat 0.3.0)
//
// Diagnostic and error types. `message` is developer/log output only (as
// upstream); paperless branches on `kind`/`cause`/`field`/`raw_value`.

use super::fields::{FieldKind, FieldRef};

/// whoosh_compat/errors.py::Cause
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    InvalidInput,
    Unsupported,
    Misconfigured,
    Internal,
}

/// whoosh_compat/errors.py::DiagnosticKind
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    // Parse-time.
    BadDate,
    BadNumber,
    TooDeep,
    PatternOnNumeric,
    PatternOnBooleanExists,
    PatternOnSubpath,
    SingleCharBracketRange,
    // Emit-time, reachable from query text.
    ExistsRequiresFast,
    TextRange,
    PatternTooComplex,
    SchemaFieldMissing,
    // Emit-time backstops for caller-built ASTs.
    AstUnfieldedTerm,
    AstUnknownField,
    AstJsonNeedsSubpath,
    AstBadNumber,
    // Unreachable in Rust: a DateRange bound is typed DateTime<Utc>.
    #[allow(dead_code)]
    AstBadDate,
    AstPatternOnKind,
    AstKindNotImplemented,
    AstInvalidShape,
    BackendRejected,
}

/// whoosh_compat/errors.py::cause_for
pub fn cause_for(kind: DiagnosticKind) -> Cause {
    use DiagnosticKind::*;
    match kind {
        BadDate | BadNumber | TooDeep => Cause::InvalidInput,
        PatternOnNumeric | PatternOnBooleanExists | PatternOnSubpath | SingleCharBracketRange => Cause::Unsupported,
        ExistsRequiresFast => Cause::Misconfigured,
        TextRange | PatternTooComplex => Cause::Unsupported,
        SchemaFieldMissing => Cause::Misconfigured,
        AstUnfieldedTerm
        | AstUnknownField
        | AstJsonNeedsSubpath
        | AstBadNumber
        | AstBadDate
        | AstPatternOnKind
        | AstKindNotImplemented
        | AstInvalidShape
        | BackendRejected => Cause::Internal,
    }
}

/// whoosh_compat/errors.py::Diagnostic
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub cause: Cause,
    pub message: String,
    pub startchar: Option<usize>,
    pub endchar: Option<usize>,
    pub field: Option<FieldRef>,
    pub field_kind: Option<FieldKind>,
    pub raw_value: Option<String>,
    pub divergence: Option<u32>,
    pub suggestion: Option<String>,
}

impl Diagnostic {
    pub fn new(kind: DiagnosticKind, message: impl Into<String>) -> Self {
        Diagnostic {
            kind,
            cause: cause_for(kind),
            message: message.into(),
            startchar: None,
            endchar: None,
            field: None,
            field_kind: None,
            raw_value: None,
            divergence: None,
            suggestion: None,
        }
    }
}

/// whoosh_compat/errors.py::QueryError (raised by emit()).
#[derive(Debug, Clone)]
pub struct QueryError {
    pub diagnostic: Box<Diagnostic>,
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.diagnostic.message)
    }
}
