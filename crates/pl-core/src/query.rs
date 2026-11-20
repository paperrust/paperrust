//! Query parameters like Django `QueryDict`.
//!
//! upstream: django/http/request.py@5.2 (`QueryDict.__init__` → `parse_qsl(keep_blank_values=True)`,
//! `MultiValueDict.__getitem__/get` → **last** value, `getlist` → all values).

use std::convert::Infallible;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use crate::urlenc;

/// Ordered multi-map of the query parameters (order as in the request).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryParams {
    pairs: Vec<(String, String)>,
}

impl QueryParams {
    /// Parses a raw query string (without `?`) like Django `QueryDict`.
    pub fn parse(raw: &str) -> Self {
        Self { pairs: urlenc::parse_qsl(raw, true) }
    }

    /// From ready-made pairs (for tests).
    pub fn from_pairs<K: Into<String>, V: Into<String>>(pairs: impl IntoIterator<Item = (K, V)>) -> Self {
        Self { pairs: pairs.into_iter().map(|(k, v)| (k.into(), v.into())).collect() }
    }

    /// `QueryDict.get(key)`: the **last** value of the key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.pairs.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// `QueryDict.getlist(key)`: all values in order.
    pub fn get_list(&self, key: &str) -> Vec<&str> {
        self.pairs.iter().filter(|(k, _)| k == key).map(|(_, v)| v.as_str()).collect()
    }

    /// `key in QueryDict`.
    pub fn contains(&self, key: &str) -> bool {
        self.pairs.iter().any(|(k, _)| k == key)
    }

    /// All pairs in request order.
    pub fn pairs(&self) -> &[(String, String)] {
        &self.pairs
    }

    /// Keys in order of first occurrence (without duplicates).
    pub fn keys(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for (k, _) in &self.pairs {
            if !out.contains(&k.as_str()) {
                out.push(k);
            }
        }
        out
    }

    /// `true` if there are no parameters.
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }
}

impl<S: Send + Sync> FromRequestParts<S> for QueryParams {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(QueryParams::parse(parts.uri.query().unwrap_or("")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_value_wins() {
        let q = QueryParams::parse("page=1&page=3&x=&tags__id__in=1,2");
        assert_eq!(q.get("page"), Some("3"));
        assert_eq!(q.get_list("page"), vec!["1", "3"]);
        assert_eq!(q.get("x"), Some(""));
        assert!(q.contains("x"));
        assert_eq!(q.get("missing"), None);
        assert_eq!(q.keys(), vec!["page", "x", "tags__id__in"]);
    }
}
