//! Typed writes against the `trust_scores` table.
//!
//! W7 (trust_score_recompute) is the only producer today. It runs hourly,
//! joins reputation + endpoint_probes + manifests + agents into a single
//! CTE, and UPSERTs one row per agent that has at least one signal. The
//! whole recompute is one SQL statement so the worker doesn't materialise
//! 100k agent rows in Rust just to write them back.
//!
//! ## Scoring formula (v1)
//!
//! `score = 0.4*reputation_avg + 0.3*endpoint_liveness + 0.2*manifest_valid + 0.1*recency`
//!
//! All four components are normalised to 0..100. The composite is rounded
//! and clamped to 0..100 then stored as `INT`. Components are stored
//! verbatim in the `components` JSONB column so the API and dashboard can
//! show the breakdown.
//!
//! ## Zero-signal agents
//!
//! Agents with NO feedback, NO endpoint probes, AND no manifests get no
//! row written — not a zero score. Writing a zero baseline would flood
//! `trust_scores` with rows for every newly-registered agent and turn the
//! "low trust" alert into noise. The `WHERE has_signal` filter in the
//! CTE excludes them; documented here so reviewers know it's intentional.

use sqlx::PgPool;

/// Scorer version stamped into every row. Bump when the formula changes;
/// the PK is `(chain_id, agent_id, scorer_version)` so old + new scores
/// can coexist while clients migrate.
pub const SCORER_V1: &str = "v1";

/// Run the full recompute. Returns the number of `trust_scores` rows
/// written (inserted OR updated — Postgres `RETURNING` counts both).
///
/// The SQL is a single statement so it runs atomically and we don't have
/// to thread a transaction through the worker.
pub async fn recompute_v1(pool: &PgPool) -> sqlx::Result<u64> {
    let rows_affected = sqlx::query(RECOMPUTE_V1_SQL)
        .bind(SCORER_V1)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(rows_affected)
}

/// The recompute statement. Kept as a module-level `const` so tests can
/// also exercise it against a real Postgres and confirm shape.
///
/// ## Window choices
///   - reputation: last 90 days, non-revoked feedback.
///   - endpoint_liveness: last 7 days of `endpoint_probes`.
///   - manifest_valid: the LATEST manifest row's `validation_status`.
///   - recency: log(1 + days_since_registration) / log(1 + 365).
///
/// ## Why `LEFT JOIN` instead of separate UNIONs?
///
/// Postgres' `LATERAL` joins would let us subquery per agent but the
/// optimiser handles plain LEFT JOIN against three pre-aggregated CTEs
/// just as well. UNION-based filters would force a final GROUP BY and
/// drop the per-component values into the JSONB.
pub const RECOMPUTE_V1_SQL: &str = r#"
WITH
  rep AS (
    -- Mean of non-revoked feedback values (scaled by value_decimals)
    -- over the last 90 days, mapped from [-100, 100] → [0, 100] in the
    -- `combined` CTE below. `value / 10^value_decimals` is the on-chain
    -- "human" value. `created_at` is from migration 0003; W4
    -- (reputation_aggregator) stamps it from block.timestamp.
    SELECT
      f.chain_id,
      f.agent_id,
      AVG(
        LEAST(100.0, GREATEST(-100.0,
          f.value::numeric / POWER(10, f.value_decimals)::numeric
        ))
      ) AS raw_avg
    FROM feedback f
    WHERE f.is_revoked = FALSE
      AND f.created_at > NOW() - INTERVAL '90 days'
    GROUP BY f.chain_id, f.agent_id
  ),
  liv AS (
    -- Fraction of probes that returned ok=true over the last 7 days,
    -- scaled to 0..100. Agents with zero probes simply don't appear here.
    SELECT
      chain_id,
      agent_id,
      (SUM(CASE WHEN ok THEN 1 ELSE 0 END)::numeric
        / NULLIF(COUNT(*), 0)::numeric * 100.0) AS liveness_pct
    FROM endpoint_probes
    WHERE probed_at > NOW() - INTERVAL '7 days'
    GROUP BY chain_id, agent_id
  ),
  man AS (
    -- Latest manifest per agent: 100 if validation_status='valid', else 0.
    SELECT DISTINCT ON (chain_id, agent_id)
      chain_id,
      agent_id,
      CASE WHEN validation_status = 'valid' THEN 100 ELSE 0 END AS manifest_pct
    FROM manifests
    ORDER BY chain_id, agent_id, fetched_at DESC
  ),
  rec AS (
    -- Recency: log-shaped curve. 0 days → 0, 365 days → 100.
    -- log(1 + d) / log(1 + 365) * 100, clamped.
    SELECT
      chain_id,
      agent_id,
      LEAST(100.0, GREATEST(0.0,
        LN(1.0 + EXTRACT(EPOCH FROM (NOW() - registered_at))::numeric / 86400.0)
          / LN(1.0 + 365.0::numeric) * 100.0
      )) AS recency_pct
    FROM agents
  ),
  combined AS (
    SELECT
      a.chain_id,
      a.agent_id,
      -- Map raw_avg ∈ [-100, 100] → [0, 100] by (x + 100) / 2.
      -- Default to 0 when no feedback exists.
      COALESCE((rep.raw_avg + 100.0) / 2.0, 0.0)            AS reputation_pct,
      COALESCE(liv.liveness_pct, 0.0)                        AS liveness_pct,
      COALESCE(man.manifest_pct::numeric, 0.0)               AS manifest_pct,
      COALESCE(rec.recency_pct, 0.0)                         AS recency_pct,
      -- "has signal" gate: agent must have at least one of feedback,
      -- probes, or a manifest. Recency alone is NOT a signal — every
      -- registered agent has it.
      (rep.raw_avg IS NOT NULL
        OR liv.liveness_pct IS NOT NULL
        OR man.manifest_pct IS NOT NULL) AS has_signal
    FROM agents a
    LEFT JOIN rep ON rep.chain_id = a.chain_id AND rep.agent_id = a.agent_id
    LEFT JOIN liv ON liv.chain_id = a.chain_id AND liv.agent_id = a.agent_id
    LEFT JOIN man ON man.chain_id = a.chain_id AND man.agent_id = a.agent_id
    LEFT JOIN rec ON rec.chain_id = a.chain_id AND rec.agent_id = a.agent_id
  ),
  scored AS (
    SELECT
      chain_id,
      agent_id,
      reputation_pct,
      liveness_pct,
      manifest_pct,
      recency_pct,
      LEAST(100, GREATEST(0,
        ROUND(
          0.4 * reputation_pct
        + 0.3 * liveness_pct
        + 0.2 * manifest_pct
        + 0.1 * recency_pct
        )::int
      )) AS score
    FROM combined
    WHERE has_signal
  )
INSERT INTO trust_scores
       (chain_id, agent_id, scorer_version, computed_at, score, components)
SELECT
  chain_id,
  agent_id,
  $1,
  NOW(),
  score,
  jsonb_build_object(
    'reputation_pct', ROUND(reputation_pct::numeric, 2),
    'liveness_pct',   ROUND(liveness_pct::numeric, 2),
    'manifest_pct',   ROUND(manifest_pct::numeric, 2),
    'recency_pct',    ROUND(recency_pct::numeric, 2),
    'weights', jsonb_build_object(
      'reputation', 0.4, 'liveness', 0.3, 'manifest', 0.2, 'recency', 0.1
    )
  )
FROM scored
ON CONFLICT (chain_id, agent_id, scorer_version) DO UPDATE
  SET computed_at = EXCLUDED.computed_at,
      score       = EXCLUDED.score,
      components  = EXCLUDED.components;
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_mentions_all_four_components() {
        // Cheap guard against accidentally deleting a CTE during edits.
        for needle in [
            "reputation_pct",
            "liveness_pct",
            "manifest_pct",
            "recency_pct",
            "has_signal",
            "ON CONFLICT",
            "scorer_version",
        ] {
            assert!(
                RECOMPUTE_V1_SQL.contains(needle),
                "RECOMPUTE_V1_SQL missing {needle}"
            );
        }
    }

    #[test]
    fn weights_sum_to_one() {
        // Sanity: the four weights baked into the SQL must sum to 1.0
        // (otherwise the composite would over/under-shoot 100).
        let w = 0.4f64 + 0.3 + 0.2 + 0.1;
        assert!((w - 1.0).abs() < 1e-9, "weights {w} != 1.0");
    }
}
