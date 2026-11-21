//! pl-timeline – data basis of the PaperRust UI (design 2).
//!
//! | Module | Content |
//! |---|---|
//! | [`extract`] | Deadlines/terms in the document content (key phrase + upstream date detection) |
//! | [`fields`] | which custom fields mean which kind of date (setting or field name) |
//! | [`areas`] | Life areas: validation, rule matching, suggestions |
//! | [`build`] | Build timeline/upcoming from rows (pure, without DB) |
//! | [`store`] | Side tables, queries (visibility comes as SQL from the caller, `pl-store`) |
//! | [`refresh`] | Save content detection: after ingestion, outdated ones, backfill |
//! | [`history`] | Conversation history of `/ask` |
//!
//! Contract: docs/contract/pl-api-v2.md, section "PaperRust-Oberfläche".

pub mod areas;
pub mod build;
pub mod extract;
pub mod fields;
pub mod history;
pub mod refresh;
pub mod store;

use serde::{Deserialize, Serialize};

/// Setting: content detection on/off (default off).
pub const SETTING_EXTRACT: &str = "PAPERRUST_TIMELINE_EXTRACT_DATES";
/// Setting: assignment kind → custom fields (JSON object).
pub const SETTING_FIELDS: &str = "PAPERRUST_TIMELINE_DATE_FIELDS";

/// Date kind of a timeline entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateKind {
    Created,
    Due,
    CancelBy,
    ValidFrom,
    ValidUntil,
    WarrantyUntil,
}

impl DateKind {
    pub const ALL: [DateKind; 6] =
        [DateKind::Created, DateKind::Due, DateKind::CancelBy, DateKind::ValidFrom, DateKind::ValidUntil, DateKind::WarrantyUntil];
    /// Deadlines ("Upcoming", done/snooze make sense).
    pub const DEADLINES: [DateKind; 4] = [DateKind::Due, DateKind::CancelBy, DateKind::ValidUntil, DateKind::WarrantyUntil];

    pub fn as_str(self) -> &'static str {
        match self {
            DateKind::Created => "created",
            DateKind::Due => "due",
            DateKind::CancelBy => "cancel_by",
            DateKind::ValidFrom => "valid_from",
            DateKind::ValidUntil => "valid_until",
            DateKind::WarrantyUntil => "warranty_until",
        }
    }

    pub fn parse(s: &str) -> Option<DateKind> {
        DateKind::ALL.into_iter().find(|k| k.as_str() == s.trim())
    }

    pub fn is_deadline(self) -> bool {
        DateKind::DEADLINES.contains(&self)
    }

    /// Has a span (term) up to this date.
    pub fn has_span(self) -> bool {
        matches!(self, DateKind::ValidUntil | DateKind::WarrantyUntil)
    }
}

/// Origin of a date (precedence in this order, see [`Source::rank`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Created,
    CustomField,
    User,
    Content,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Created => "created",
            Source::CustomField => "custom_field",
            Source::User => "user",
            Source::Content => "content",
        }
    }
    pub fn parse(s: &str) -> Option<Source> {
        [Source::Created, Source::CustomField, Source::User, Source::Content].into_iter().find(|x| x.as_str() == s)
    }
    /// Smaller = more important.
    pub fn rank(self) -> u8 {
        match self {
            Source::Created => 0,
            Source::CustomField => 1,
            Source::User => 2,
            Source::Content => 3,
        }
    }
}

/// Error of this crate.
#[derive(Debug, thiserror::Error)]
pub enum TimelineError {
    /// Input error (field, message).
    #[error("{0}: {1}")]
    Invalid(String, String),
    #[error("not found")]
    NotFound,
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

pub type Result<T> = std::result::Result<T, TimelineError>;

pub(crate) fn invalid(field: &str, msg: impl Into<String>) -> TimelineError {
    TimelineError::Invalid(field.to_string(), msg.into())
}

/// `true`/`yes`/`1`/`on` (like the bool settings).
pub fn truthy(v: Option<&str>) -> bool {
    matches!(v.map(|s| s.trim().to_ascii_lowercase()).as_deref(), Some("1" | "y" | "yes" | "t" | "true" | "on"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_roundtrip() {
        for k in DateKind::ALL {
            assert_eq!(DateKind::parse(k.as_str()), Some(k));
            assert_eq!(serde_json::to_value(k).unwrap(), k.as_str());
        }
        assert!(DateKind::Due.is_deadline() && !DateKind::ValidFrom.is_deadline() && !DateKind::Created.is_deadline());
        assert!(DateKind::ValidUntil.has_span() && !DateKind::Due.has_span());
        assert!(truthy(Some(" Yes")) && !truthy(Some("no")) && !truthy(None));
    }
}
