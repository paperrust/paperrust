//! Tool groups and token scopes of the MCP server (PaperRust extension).
//!
//! Three groups: `read` (read, search, ask, resources, prompts), `write` (change metadata,
//! create taxonomy, aliases, notes, custom field values) and `ingest` (`add_document`).
//!
//! Effective for a request is the **intersection** of
//! 1. the groups enabled by the administrator (`PAPERRUST_AI_MCP_TOOL_GROUPS`,
//!    default `read,write`), and
//! 2. the scopes of the token (table `paperrust_ai_mcp_scope`, key SHA-256 of the token –
//!    applies equally to DRF tokens and device tokens `prt_…`; otherwise one entry per user; without
//!    an entry all enabled groups apply).
//!
//! Scopes **never extend** the user's permissions: every tool afterwards still checks the
//! model and object permissions of the token user (like the REST API).

use std::collections::BTreeSet;

use sha2::Digest as _;

/// All known groups (= scopes).
pub const GROUPS: [&str; 3] = ["read", "write", "ingest"];

/// Group of a tool (`None` = unknown tool).
pub fn tool_group(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "list_taxonomy" | "find_similar" | "search_documents" | "get_document" | "suggest_metadata" | "ask_documents"
        | "get_document_content" | "list_recent_documents" | "list_saved_views" | "run_saved_view" | "get_custom_fields"
        | "list_notes" | "get_task_status" => "read",
        "update_document" | "create_taxonomy" | "add_alias" | "set_custom_field" | "add_note" => "write",
        "add_document" => "ingest",
        _ => return None,
    })
}

/// Group set from a list (unknown names are ignored, case does not matter).
pub fn parse(list: &[String]) -> BTreeSet<&'static str> {
    list.iter().filter_map(|s| GROUPS.iter().find(|g| g.eq_ignore_ascii_case(s.trim())).copied()).collect()
}

/// Effective groups: enabled ∩ token scopes (without a token entry: all enabled ones).
pub fn effective(enabled: &BTreeSet<&'static str>, token: Option<&[String]>) -> BTreeSet<&'static str> {
    match token {
        None => enabled.clone(),
        Some(t) => parse(t).intersection(enabled).copied().collect(),
    }
}

/// Key of a token in `paperrust_ai_mcp_scope` (SHA-256, hex; never the plaintext).
pub fn token_key(token: &str) -> String {
    let d = sha2::Sha256::digest(token.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn intersection() {
        let enabled = parse(&v(&["read", "write"]));
        assert_eq!(effective(&enabled, None), enabled);
        assert_eq!(effective(&enabled, Some(&v(&["read", "ingest"]))), parse(&v(&["read"])));
        assert!(effective(&enabled, Some(&v(&[]))).is_empty());
        assert_eq!(parse(&v(&["READ", "bogus"])), parse(&v(&["read"])));
    }

    #[test]
    fn groups_of_tools() {
        assert_eq!(tool_group("add_document"), Some("ingest"));
        assert_eq!(tool_group("add_note"), Some("write"));
        assert_eq!(tool_group("ask_documents"), Some("read"));
        assert_eq!(tool_group("delete_document"), None);
        assert_eq!(token_key("abc").len(), 64);
    }
}
