//! pl-matching – see docs/ARCHITECTURE.md and docs/contract/pl-matching.md.
//!
//! Pure library (no DB, no routes) with the matching algorithms of
//! `documents/matching.py`, the workflow trigger checks and the content date
//! detection of `documents/plugins/date_parsing` (including a port of the used
//! subset of the Python library `dateparser`).

// Nested `if`s are kept on purpose: they mirror the upstream Python condition
// structure line by line, which makes reviewing the port against upstream easier.
#![allow(clippy::collapsible_if, clippy::collapsible_match)]

pub mod cache;
pub mod dates;
pub mod fnmatch;
pub mod fuzz;
pub mod matching;
pub mod pyext;
pub mod pyre;
pub mod regex_compat;
pub mod workflow;

pub use dates::{DateParserConfig, RegexDateParser};
pub use matching::{Classifier, MatchDocument, Matchable, matches};
pub use regex_compat::{safe_regex_search, validate_regex_pattern};
pub use workflow::{
    ConsumableDocument, DocumentSource, ExistingDocument, WorkflowTrigger, consumable_document_matches_workflow,
    document_matches_workflow, existing_document_matches_workflow, prefilter_matches,
};
