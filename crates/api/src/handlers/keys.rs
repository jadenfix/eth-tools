//! Admin handlers for `/api/v1/keys/*`.
//!
//! These are the dashboard's CRUD surface. They are protected by
//! `auth::require_internal` (shared-secret + injected user identity headers
//! from the Next.js Route Handler that already verified the Auth.js session).
//!
//! Public bearer-token writes (e.g. `/api/v1/agents/:id/invoke`) live
//! elsewhere and use `auth::bearer_required` instead.

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::InternalCaller;
use crate::keys::{self, KeyError};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub name: String,
    #[serde(default)]
    pub scopes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct IssuedResp {
    pub id: Uuid,
    pub prefix: String,
    /// Plaintext bearer. Echoed exactly **once** — the dashboard must show
    /// this and prompt the user to copy it; we will never echo it again.
    pub plaintext: String,
    pub name: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct KeyView {
    pub id: Uuid,
    pub prefix: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Serialize)]
pub struct ListResp {
    pub data: Vec<KeyView>,
}

/// Map KeyError to the same JSON-envelope shape the rest of the API uses.
fn render_err(e: KeyError) -> (StatusCode, Json<serde_json::Value>) {
    let (status, code, hint) = match &e {
        KeyError::InvalidName => (StatusCode::BAD_REQUEST, "INVALID_NAME", "name must be 1-128 non-whitespace chars"),
        KeyError::InvalidScope(_) => (
            StatusCode::BAD_REQUEST,
            "INVALID_SCOPE",
            "allowed scopes: read, write, wallet",
        ),
        KeyError::Bcrypt(_) | KeyError::Db(_) | KeyError::Join(_) => {
            tracing::error!(error = ?e, "issue key failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL", "retry; if persistent, file an issue")
        }
    };
    (
        status,
        Json(serde_json::json!({
            "error": {
                "code": code,
                "policy_version": "v1",
                "evaluator": "api.keys",
                "override_hint": hint,
            }
        })),
    )
}

/// POST /api/v1/keys — mint a new key for the caller (identified by the
/// `X-Github-*` headers that `require_internal` already verified).
pub async fn create(
    State(state): State<AppState>,
    Extension(caller): Extension<InternalCaller>,
    Json(body): Json<CreateBody>,
) -> Result<(StatusCode, Json<IssuedResp>), (StatusCode, Json<serde_json::Value>)> {
    let scopes_input = if body.scopes.is_empty() {
        vec!["read".to_string()]
    } else {
        body.scopes.clone()
    };
    let issued = keys::issue(caller.user_id, &caller.login, &body.name, &scopes_input, &state.pool)
        .await
        .map_err(render_err)?;
    let resp = IssuedResp {
        id: issued.id,
        prefix: issued.prefix,
        plaintext: issued.plaintext,
        name: body.name.trim().to_string(),
        scopes: scopes_input,
    };
    Ok((StatusCode::CREATED, Json(resp)))
}

/// GET /api/v1/keys — list the caller's active keys. Never includes the
/// plaintext or hash.
pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<InternalCaller>,
) -> Result<Json<ListResp>, (StatusCode, Json<serde_json::Value>)> {
    let rows = keys::list_for_user(caller.user_id, &state.pool)
        .await
        .map_err(render_err)?;
    let data = rows
        .into_iter()
        .map(|r| KeyView {
            id: r.id,
            prefix: r.key_prefix,
            name: r.name,
            scopes: r.scopes,
            created_at: r.created_at,
            last_used_at: r.last_used_at,
        })
        .collect();
    Ok(Json(ListResp { data }))
}

/// DELETE /api/v1/keys/:id — revoke (owner-checked). Returns 204 on success,
/// 404 if not found / not owned.
pub async fn delete(
    State(state): State<AppState>,
    Extension(caller): Extension<InternalCaller>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    // Look up the prefix so we can DEL the cached entry on success — without
    // this, a freshly-revoked key would still validate for up to CACHE_TTL_SECS.
    // We do this BEFORE the revoke to avoid leaving the cache populated if
    // the lookup races a parallel revoke.
    let rows = keys::list_for_user(caller.user_id, &state.pool)
        .await
        .map_err(render_err)?;
    let prefix_to_clear = rows.iter().find(|r| r.id == id).map(|r| r.key_prefix.clone());

    let ok = keys::revoke(id, caller.user_id, &state.pool)
        .await
        .map_err(render_err)?;
    if !ok {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "code": "KEY_NOT_FOUND",
                    "policy_version": "v1",
                    "evaluator": "api.keys",
                    "override_hint": "key id is unknown, already revoked, or not owned by you",
                }
            })),
        ));
    }
    if let (Some(redis), Some(p)) = (state.redis.as_ref(), prefix_to_clear) {
        redis.del(&p).await;
    }
    Ok(StatusCode::NO_CONTENT)
}
