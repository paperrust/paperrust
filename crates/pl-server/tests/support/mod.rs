//! Test helpers shared by the integration tests.

use pl_core::AppState;
use pl_core::config::Env;

/// AppState without a database: the pool is lazy and never connects in these tests.
pub fn test_state() -> AppState {
    let config = pl_core::Config::from_env_map(Env::from_pairs([(
        "PAPERLESS_SECRET_KEY",
        "test-only-secret-key-0123456789",
    )]))
    .unwrap();
    let db = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://localhost/unused")
        .unwrap();
    AppState::with_pool(config, db).unwrap()
}
