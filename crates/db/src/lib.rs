//! Postgres schema + typed queries. Bootstrap stub — sqlx integration lands in
//! the next PR once Neon is provisioned. The canonical schema lives in
//! `migrations/0001_init.up.sql` (paired with `0001_init.down.sql` — sqlx-cli's
//! reversible-migration convention).

pub const LATEST_MIGRATION: &str = "0001_init";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Asserts every up-migration in `crates/db/migrations/` has a matching down.
    /// Catches a real bug (forgetting the down) at test time, not in prod.
    #[test]
    fn every_up_migration_has_a_down() {
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
        // Sanity: at least one migration pair exists.
        assert!(
            entries.iter().any(|f| f.ends_with(".up.sql")),
            "no up migrations found"
        );
        assert!(LATEST_MIGRATION.starts_with("0001"));
    }
}
