//! Postgres schema + typed queries.
//!
//! Migrations live in `crates/db/migrations/` paired up/down per sqlx
//! convention. `connect()` opens a pgbouncer-friendly pool; `migrate()` runs
//! all pending migrations in a transaction.
//!
//! Query layer uses non-macro `sqlx::query`/`query_as` for now so CI builds
//! without a live DB. The `query!` macro upgrade lands in a follow-up once a
//! `.sqlx/` offline cache is committed and CI sets `SQLX_OFFLINE=true`.

use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

pub mod agents;
pub mod chains_seed;
pub mod cursors;
pub mod feedback;
pub mod trust_scores;
pub mod validations;
pub mod worker_runs;

pub use sqlx::PgPool as Pool;

pub const LATEST_MIGRATION: &str = "0003_feedback_created_at";

/// Build a Postgres pool sized for PgBouncer (Neon's pooler default is
/// transaction-mode with 100 connections). Conservative `max_connections` so
/// many concurrent function invocations don't exhaust the pooler.
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Duration::from_secs(300))
        .connect(database_url)
        .await
}

/// Apply all pending migrations from `crates/db/migrations/`. Wrapped in a
/// transaction by sqlx; failure rolls back. Call before serving requests.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use std::path::PathBuf;

    /// Every up-migration in `crates/db/migrations/` has a paired down.
    #[test]
    fn every_up_has_a_down() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .expect("migrations dir must exist")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        for up in entries.iter().filter(|f| f.ends_with(".up.sql")) {
            let down = up.replace(".up.sql", ".down.sql");
            assert!(
                entries.iter().any(|f| f == &down),
                "missing matching down migration for {up}"
            );
        }
        assert!(
            entries.iter().any(|f| f.ends_with(".up.sql")),
            "no up migrations found"
        );
        assert!(LATEST_MIGRATION.starts_with("0003"));
    }
}
