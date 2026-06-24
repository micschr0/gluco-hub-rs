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
            // Zero-pad both tokens to a fixed buffer so ct_eq always
            // compares the same number of bytes — eliminates the length
            // timing leak that subtle's slice ct_eq has on mismatched
            // lengths.
            const BUF: usize = 512;
            let mut exp_buf = [0u8; BUF];
            let mut got_buf = [0u8; BUF];
            let exp = secret.expose_secret();
            let exp_b = exp.as_bytes();
            let got_b = token.as_bytes();
            exp_buf[..exp_b.len().min(BUF)].copy_from_slice(&exp_b[..exp_b.len().min(BUF)]);
            got_buf[..got_b.len().min(BUF)].copy_from_slice(&got_b[..got_b.len().min(BUF)]);
            bool::from(exp_buf.ct_eq(&got_buf))
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
