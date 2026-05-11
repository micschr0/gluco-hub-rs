// SPDX-License-Identifier: AGPL-3.0-or-later

//! Bearer-token middleware for `/glucose/*`.
//!
//! - Disabled (passthrough) when `AppState::bearer_token` is `None` —
//!   the operator opts in by setting `GLUCO_HUB__HTTP__BEARER_TOKEN`.
//! - Enabled: requires `Authorization: Bearer <token>` and compares the
//!   provided token against the resolved secret with `subtle::ConstantTimeEq`
//!   to keep the response timing flat across right/wrong tokens.
//! - On failure, returns `401` with a stable JSON error_code (`AUTH001`)
//!   so the metrics counter labels stay grep-friendly.

use axum::Json;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use secrecy::ExposeSecret;
use serde_json::json;
use subtle::ConstantTimeEq;

use super::AppState;

/// Middleware function. Wired via `axum::middleware::from_fn_with_state`
/// onto the protected sub-router; never as a global layer.
pub async fn require_bearer(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let Some(secret) = state.bearer_token.as_ref() else {
        return next.run(request).await;
    };

    let provided = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));

    let authorized = match provided {
        Some(token) => {
            let expected = secret.expose_secret().as_bytes();
            // `ct_eq` for `[u8]` short-circuits on length mismatch; that
            // length leak is acceptable here because deployment tokens are
            // a fixed length per operator. If equal length, the per-byte
            // comparison is constant-time.
            bool::from(token.as_bytes().ct_eq(expected))
        }
        None => false,
    };

    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error_code": "AUTH001",
                "message": "missing or invalid token",
            })),
        )
            .into_response();
    }
    next.run(request).await
}
