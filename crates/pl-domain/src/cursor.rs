//! Opaque cursors of the lists (`next_cursor`). Content: sort key + `legacy_id` as
//! tie-breaker, base64url(JSON) with prefix `k1.`. Clients must not interpret the content.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::DomainError;

/// Position after the last delivered element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeysetCursor {
    /// Sort field the cursor applies to (protection against changing the sort order).
    pub s: String,
    /// Value of the sort field (string/number; `null` when sorting by `id`).
    pub v: Value,
    /// `legacy_id` of the last element.
    pub id: i64,
}

impl KeysetCursor {
    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        format!("k1.{}", URL_SAFE_NO_PAD.encode(json))
    }

    pub fn decode(s: &str) -> Result<Self, DomainError> {
        let bad = || DomainError::field("cursor", "Invalid cursor.");
        let body = s.strip_prefix("k1.").ok_or_else(bad)?;
        let raw = URL_SAFE_NO_PAD.decode(body).map_err(|_| bad())?;
        serde_json::from_slice(&raw).map_err(|_| bad())
    }

    /// Decode and check that the cursor belongs to the requested sort order.
    pub fn decode_for(s: &str, sort: &str) -> Result<Self, DomainError> {
        let c = Self::decode(s)?;
        if c.s != sort {
            return Err(DomainError::field("cursor", "Cursor does not match the requested sort order."));
        }
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let c = KeysetCursor { s: "-created".into(), v: Value::String("2024-01-31".into()), id: 42 };
        let e = c.encode();
        assert!(e.starts_with("k1."));
        assert_eq!(KeysetCursor::decode(&e).unwrap(), c);
        assert!(KeysetCursor::decode_for(&e, "title").is_err());
        assert!(KeysetCursor::decode("k1.!!").is_err());
        assert!(KeysetCursor::decode("x").is_err());
    }
}
