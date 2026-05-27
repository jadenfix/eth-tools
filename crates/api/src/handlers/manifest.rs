//! `/api/v1/manifest/{validate,hash,generate}`.
//!
//! Pure-compute endpoints: input → parse → respond. No DB access, no chain
//! reads. That makes them cheap to call (suitable for the anonymous rate
//! limit) and easy to test (no testcontainers needed).
//!
//! `validate` and `hash` accept either a `uri` (the function fetches it) OR
//! `bytes_b64` (the caller already has them). We never accept both — that
//! ambiguity would let a hostile caller hash bytes that don't match the URI.
//!
//! ## Network fetch (uri mode)
//! The fetch path is currently STUBBED — full networking lives behind the
//! manifest-fetcher worker on `feat/phase-4-w2-manifest-fetcher` and the
//! SSRF guard in `crates/mcp::guard`. Until that lands we return
//! `NotImplemented` for the `uri` branch, but `bytes_b64` works end-to-end.

use axum::extract::State;
use axum::Json;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::dto::{
    ManifestError, ManifestGenerateResponse, ManifestHashResponse, ManifestInput,
    ManifestValidateResponse, OneEnvelope,
};
use crate::error::ApiError;
use crate::AppState;

/// 64 KiB. ERC-8004 manifests are agent-cards — realistically <16 KiB.
/// Anything larger is suspicious + would dominate function CPU.
const MAX_MANIFEST_BYTES: usize = 64 * 1024;

/// Resolve the input to raw bytes + a `source` string used for tracing/echo.
/// Enforces the "exactly one of {uri, bytes_b64}" invariant.
fn resolve_input(input: &ManifestInput) -> Result<(Vec<u8>, String), ApiError> {
    match (input.uri.as_deref(), input.bytes_b64.as_deref()) {
        (Some(_), Some(_)) => Err(ApiError::InvalidInput(
            "set exactly one of `uri` or `bytes_b64` — not both".into(),
        )),
        (None, None) => Err(ApiError::InvalidInput(
            "set one of `uri` or `bytes_b64`".into(),
        )),
        (Some(_uri), None) => {
            // TODO(feat/phase-4-w2-manifest-fetcher): once the fetcher worker
            // exposes a re-usable client with the SSRF guard, call it here.
            // Until then, return NotImplemented rather than silently doing
            // an unguarded fetch.
            Err(ApiError::NotImplemented("feat/phase-4-w2-manifest-fetcher"))
        }
        (None, Some(b64)) => {
            let bytes = STANDARD
                .decode(b64.as_bytes())
                .map_err(|_| ApiError::InvalidInput("bytes_b64 is not valid base64".into()))?;
            if bytes.len() > MAX_MANIFEST_BYTES {
                return Err(ApiError::InvalidInput(format!(
                    "manifest exceeds {MAX_MANIFEST_BYTES} bytes ({} given)",
                    bytes.len()
                )));
            }
            Ok((bytes, String::new()))
        }
    }
}

/// Manifest validation rules.
///
/// We DON'T pull in `eth-tools-core::manifest` for this because that module
/// is still a stub (real schema lands in the manifest-fetcher branch).
/// Instead, the rules below codify the "good"/"bad" fixtures committed in
/// `fixtures/agent-card.{good,bad}.json`:
///   - `name` (string) required, non-empty
///   - `services` array — every entry must have `type` and `endpoint`
///   - `services[*].type` ∈ {web, A2A, MCP, OASF, ENS, DID, email}
///   - `services[*].endpoint` must parse as a URL
fn validate_manifest(bytes: &[u8]) -> Vec<ManifestError> {
    let mut errs = Vec::new();
    let v: Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(e) => {
            errs.push(ManifestError {
                pointer: "".into(),
                message: format!("not valid JSON: {e}"),
            });
            return errs;
        }
    };
    let obj = match v.as_object() {
        Some(o) => o,
        None => {
            errs.push(ManifestError {
                pointer: "".into(),
                message: "top-level value must be a JSON object".into(),
            });
            return errs;
        }
    };
    match obj.get("name").and_then(|x| x.as_str()) {
        Some(s) if !s.trim().is_empty() => {}
        _ => errs.push(ManifestError {
            pointer: "/name".into(),
            message: "required: non-empty string".into(),
        }),
    }
    const VALID_SERVICE_TYPES: &[&str] =
        &["web", "A2A", "MCP", "OASF", "ENS", "DID", "email"];
    if let Some(services) = obj.get("services").and_then(|x| x.as_array()) {
        for (i, svc) in services.iter().enumerate() {
            let pointer_base = format!("/services/{i}");
            let svc_obj = match svc.as_object() {
                Some(o) => o,
                None => {
                    errs.push(ManifestError {
                        pointer: pointer_base.clone(),
                        message: "must be an object".into(),
                    });
                    continue;
                }
            };
            match svc_obj.get("type").and_then(|x| x.as_str()) {
                Some(t) if VALID_SERVICE_TYPES.contains(&t) => {}
                Some(t) => errs.push(ManifestError {
                    pointer: format!("{pointer_base}/type"),
                    message: format!(
                        "must be one of {:?}, got {t:?}",
                        VALID_SERVICE_TYPES
                    ),
                }),
                None => errs.push(ManifestError {
                    pointer: format!("{pointer_base}/type"),
                    message: "required".into(),
                }),
            }
            match svc_obj.get("endpoint").and_then(|x| x.as_str()) {
                Some(ep) => {
                    if url::Url::parse(ep).is_err() {
                        errs.push(ManifestError {
                            pointer: format!("{pointer_base}/endpoint"),
                            message: format!("not a valid URL: {ep:?}"),
                        });
                    }
                }
                None => errs.push(ManifestError {
                    pointer: format!("{pointer_base}/endpoint"),
                    message: "required".into(),
                }),
            }
        }
    }
    errs
}

#[utoipa::path(
    post,
    path = "/api/v1/manifest/validate",
    tag = "manifest",
    operation_id = "manifest_validate",
    request_body = crate::dto::ManifestInput,
    responses(
        (status = 200, description = "Schema check result + structured errors", body = crate::dto::ManifestValidateResponse),
        (status = 400, description = "bad input (both/neither of uri+bytes; oversize)", body = crate::dto::ApiErrorBody),
        (status = 429, description = "rate limited", body = crate::dto::ApiErrorBody),
        (status = 503, description = "uri-mode requires the manifest-fetcher branch", body = crate::dto::ApiErrorBody),
        (status = 500, description = "internal error", body = crate::dto::ApiErrorBody),
    )
)]
pub async fn validate(
    State(_state): State<AppState>,
    Json(input): Json<ManifestInput>,
) -> Result<Json<ManifestValidateResponse>, ApiError> {
    let (bytes, source) = resolve_input(&input)?;
    let errs = validate_manifest(&bytes);
    Ok(Json(ManifestValidateResponse {
        valid: errs.is_empty(),
        errors: errs,
        source,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/manifest/hash",
    tag = "manifest",
    operation_id = "manifest_hash",
    request_body = crate::dto::ManifestInput,
    responses(
        (status = 200, description = "Hashes of the raw manifest bytes", body = crate::dto::ManifestHashResponse),
        (status = 400, description = "bad input", body = crate::dto::ApiErrorBody),
        (status = 503, description = "uri-mode requires the manifest-fetcher branch", body = crate::dto::ApiErrorBody),
        (status = 500, description = "internal error", body = crate::dto::ApiErrorBody),
    )
)]
pub async fn hash(
    State(_state): State<AppState>,
    Json(input): Json<ManifestInput>,
) -> Result<Json<ManifestHashResponse>, ApiError> {
    let (bytes, _src) = resolve_input(&input)?;
    let sha = Sha256::digest(&bytes);
    let mut sha_s = String::from("0x");
    for b in sha.iter() {
        sha_s.push_str(&format!("{b:02x}"));
    }
    // We use the core stub for now (returns zeros). The real keccak digest
    // lands when `alloy-primitives::keccak256` is wired into core — until
    // then, callers should treat `keccak256` as advisory.
    // TODO(feat/phase-4-w2-manifest-fetcher): swap stub for alloy keccak.
    let kec = eth_tools_core::manifest::keccak256_raw_bytes(&bytes);
    let mut kec_s = String::from("0x");
    for b in kec.iter() {
        kec_s.push_str(&format!("{b:02x}"));
    }
    Ok(Json(ManifestHashResponse {
        sha256: sha_s,
        keccak256: kec_s,
        byte_len: bytes.len(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/manifest/generate",
    tag = "manifest",
    operation_id = "manifest_generate",
    request_body(content = Object, description = "Free-form agent-card draft; required fields backfilled."),
    responses(
        (status = 200, description = "Canonical manifest JSON", body = crate::dto::ManifestGenerateResponse),
        (status = 400, description = "bad input", body = crate::dto::ApiErrorBody),
        (status = 500, description = "internal error", body = crate::dto::ApiErrorBody),
    )
)]
pub async fn generate(
    State(_state): State<AppState>,
    Json(input): Json<Value>,
) -> Result<Json<OneEnvelope<ManifestGenerateResponse>>, ApiError> {
    // Canonicalisation strategy: take the caller's object, normalise the
    // required keys, and emit a top-level JSON object with deterministic
    // field ordering (alphabetical, since serde_json::Map is sorted when we
    // construct it with sort_keys disabled — we use a BTreeMap-backed
    // serializer instead).
    //
    // We INTENTIONALLY don't canonicalise the bytes for hashing here (the
    // hash endpoint hashes raw bytes per `manifest::CANONICALIZATION_NOTE`).
    // This endpoint's output is just "a starting-point manifest a human can
    // commit to their repo".
    let obj = input.as_object().cloned().ok_or_else(|| {
        ApiError::InvalidInput("top-level value must be a JSON object".into())
    })?;
    let name = obj
        .get("name")
        .and_then(|x| x.as_str())
        .unwrap_or("untitled-agent")
        .to_string();
    let description = obj
        .get("description")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let version = obj
        .get("version")
        .and_then(|x| x.as_str())
        .unwrap_or("0.0.1")
        .to_string();
    let owner = obj.get("owner").cloned().unwrap_or(Value::Null);
    let services = obj
        .get("services")
        .cloned()
        .unwrap_or_else(|| Value::Array(vec![]));
    let skills = obj
        .get("skills")
        .cloned()
        .unwrap_or_else(|| Value::Array(vec![]));
    let manifest = json!({
        "name": name,
        "description": description,
        "version": version,
        "owner": owner,
        "services": services,
        "skills": skills,
    });
    Ok(Json(OneEnvelope {
        data: ManifestGenerateResponse { manifest },
        staleness_ms: 0,
        source: "compute",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_good_fixture_returns_empty_errors() {
        let bytes = include_bytes!("../../../../fixtures/agent-card.good.json");
        let errs = validate_manifest(bytes);
        assert!(errs.is_empty(), "expected zero errors, got {errs:?}");
    }

    #[test]
    fn validate_bad_fixture_flags_known_issues() {
        let bytes = include_bytes!("../../../../fixtures/agent-card.bad.json");
        let errs = validate_manifest(bytes);
        // Missing `name`, bad URL on /services/0/endpoint, bad type on
        // /services/1/type — at least 3 distinct pointers.
        assert!(errs.len() >= 3, "expected >=3 errors, got {errs:?}");
        let pointers: Vec<&str> = errs.iter().map(|e| e.pointer.as_str()).collect();
        assert!(pointers.contains(&"/name"));
        assert!(pointers.iter().any(|p| p.starts_with("/services/0/endpoint")));
        assert!(pointers.iter().any(|p| p.starts_with("/services/1/type")));
    }

    #[test]
    fn validate_non_object_top_level() {
        let errs = validate_manifest(b"42");
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].pointer, "");
    }

    #[test]
    fn validate_invalid_json() {
        let errs = validate_manifest(b"not json {");
        assert_eq!(errs.len(), 1);
    }
}
