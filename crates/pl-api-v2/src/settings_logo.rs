//! Logo upload (`PUT /rs/api/v2/settings/app_logo`, docs/SETTINGS.md §2.8) with the same rules
//! as v1. The implementation lives in [`pl_settings::logo`] (shared with v1
//! `PATCH /api/config/{pk}/` in pl-system).

pub use pl_settings::logo::*;
