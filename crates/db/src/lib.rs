//! Postgres schema + typed queries. Bootstrap stub — sqlx integration lands in
//! the next PR once Neon is provisioned. The canonical schema lives in
//! `migrations/0001_init.sql`.

pub const SCHEMA_VERSION: &str = "0001_init";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_follows_naming_convention() {
        // sqlx convention: NNNN_<name>.sql
        let parts: Vec<&str> = SCHEMA_VERSION.splitn(2, '_').collect();
        assert_eq!(parts.len(), 2, "expected NNNN_<name>; got {SCHEMA_VERSION}");
        assert_eq!(parts[0].len(), 4, "expected 4-digit prefix; got {}", parts[0]);
        assert!(
            parts[0].chars().all(|c| c.is_ascii_digit()),
            "prefix must be all digits"
        );
    }
}
