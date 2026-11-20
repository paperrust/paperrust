//! Name, version and identifiers of PaperRust in one place (docs/BRANDING.md).
//!
//! PaperRust is based on paperless-ngx (GPL-3.0), © the paperless-ngx authors. This acknowledgment
//! ([`ATTRIBUTION`]) is kept in LICENSE/NOTICE and in about information (GPL-3.0 §4/§5).
//!
//! Who uses what:
//! * [`USER_AGENT`] – outgoing HTTP requests to third-party services (webhooks, mail OAuth, OIDC, LLM,
//!   version check). Not for the Django proxy: it passes the client's header through.
//! * [`PAPERRUST_VERSION`] / [`REPO_URL`] – version check (pl-system), about dialog, MCP `serverInfo`.
//! * [`UPSTREAM_VERSION`] – the emulated paperless-ngx version. It stays in responses from which
//!   clients read the server capabilities (`X-Version` header, `/api/remote_version/`
//!   `version` comparison, export manifest): third-party apps unlock features based on it.

/// Product name in texts, UI and logs.
pub const PRODUCT_NAME: &str = "PaperRust";

/// Technical name (binary, Docker image, repo, MCP server name, DB `application_name`).
pub const BIN_NAME: &str = "paperrust";

/// PaperRust's own version (workspace version).
pub const PAPERRUST_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Emulated paperless-ngx version (API/schema level, `X-Version`).
pub const UPSTREAM_VERSION: &str = "3.4.2";

/// Source repository (GPL-3.0: source code offer, version check).
pub const REPO_URL: &str = "https://github.com/paperrust/paperrust";

/// Releases API of the repo for the version check (GitHub `releases/latest`).
pub const RELEASES_API_URL: &str = "https://api.github.com/repos/paperrust/paperrust/releases/latest";

/// `User-Agent` for outgoing requests: `PaperRust/<version> (+<repo>)`.
pub const USER_AGENT: &str = concat!("PaperRust/", env!("CARGO_PKG_VERSION"), " (+https://github.com/paperrust/paperrust)");

/// Subtitle (docs, about information).
pub const TAGLINE: &str = "PaperRust – schneller, schlanker, kompatibler Ersatz für paperless-ngx";

/// Mandatory acknowledgment (GPL-3.0, never remove).
pub const ATTRIBUTION: &str = "PaperRust basiert auf paperless-ngx (GPL-3.0), © die paperless-ngx-Autoren";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consistent() {
        assert!(USER_AGENT.starts_with(&format!("{PRODUCT_NAME}/{PAPERRUST_VERSION} ")));
        assert!(USER_AGENT.contains(REPO_URL));
        assert!(RELEASES_API_URL.ends_with("/repos/paperrust/paperrust/releases/latest"));
        assert_eq!(BIN_NAME, PRODUCT_NAME.to_lowercase());
        assert!(ATTRIBUTION.contains("paperless-ngx") && ATTRIBUTION.contains("GPL-3.0"));
    }
}
