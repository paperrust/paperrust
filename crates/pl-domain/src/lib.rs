//! pl-domain – domain core of the native API v2 (docs/ARCHITECTURE-V2.md §1–§2).
//!
//! * [`model`]: domain types (`Document`, `Tag`, …) with `uuid`, `legacy_id`, `rev` and the
//!   simple permission format [`model::Access`] – **not** the DRF shapes of v1.
//! * [`filter`]: resolved filter AST [`filter::DocFilter`] (L1), shared by v1 and v2.
//! * [`query`]: typed query AST [`query::DocQuery`] for `POST /rs/api/v2/documents/query`.
//! * [`cursor`]: opaque cursors for cursor pagination.
//! * [`repo`]: interfaces (`Repo`, `ChangeFeed`) implemented by `pl-store` and `pl-sync`.
//!
//! Dependency direction: `pl-api-v2 → pl-domain ← pl-store`. This crate knows neither axum nor
//! sqlx nor Django table names. Contract: docs/contract/pl-api-v2.md (section "pl-domain").

pub mod cursor;
pub mod error;
pub mod filter;
pub mod model;
pub mod query;
pub mod repo;

pub use error::{DomainError, DomainResult};
pub use model::Kind;
pub use repo::{BoxFut, ChangeFeed, Principal, Repo};
