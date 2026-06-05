// SPDX-License-Identifier: AGPL-3.0-or-later

use std::sync::Arc;

use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::get;
use gluco_hub_core::ReadingCache;
use metrics_exporter_prometheus::PrometheusHandle;
use secrecy::SecretString;
use tokio::sync::{broadcast, watch};
use tower_http::trace::TraceLayer;

use crate::poll_status::PollStatus;

mod auth;
pub mod clock;
mod glucose;
mod health;
mod metrics;
mod status;

pub use clock::{ClockReadingEvent, build_reading_event};

/// Header attached to every API response so downstream consumers
/// (Home Assistant, dashboards, scripts) can detect the
/// not-for-medical-use posture without parsing the body.
const X_DISCLAIMER_HEADER: HeaderName = HeaderName::from_static("x-disclaimer");
const X_DISCLAIMER_VALUE: HeaderValue = HeaderValue::from_static("not-for-medical-use");

/// Shared application state passed to handlers via `State<AppState>`.
///
/// `bearer_token` is `Some` only when `GLUCO_HUB__HTTP__BEARER_TOKEN` was
/// set at startup. The auth middleware checks this on
/// every request and short-circuits to `401` on mismatch.
///
/// `poll_status_tx` is owned here to keep the channel alive for the
/// lifetime of the server. The poll task gets a clone of the `Sender`;
/// HTTP handlers read via `poll_status_rx`.
///
/// `clock_tx` is a broadcast sender owned here to keep the channel open even
/// when no SSE client is connected. The poll task publishes a
/// `ClockReadingEvent` after each successful reading; `GET /clock/events`
/// handlers subscribe via `clock_tx.subscribe()`.
#[derive(Clone)]
pub struct AppState {
    pub cache: ReadingCache,
    pub metrics_handle: PrometheusHandle,
    pub bearer_token: Option<SecretString>,
    pub poll_status_tx: Arc<watch::Sender<PollStatus>>,
    pub poll_status_rx: watch::Receiver<PollStatus>,
    pub clock_tx: Arc<broadcast::Sender<ClockReadingEvent>>,
}

/// Build the public HTTP router with state.
///
/// Routing layout:
/// - `/healthz` and `/metrics` are always public.
/// - `/glucose/*` runs through the Bearer middleware. If
///   `bearer_token` is `None` the middleware short-circuits to
///   passthrough so unauthenticated local-dev usage still works.
/// - `/api/v1/status` is public (Cache-Control: no-store).
pub fn router(state: AppState) -> Router {
    router_with_state(state)
}

pub(crate) fn router_with_state(state: AppState) -> Router {
    let public = Router::new()
        .route("/healthz", get(health::healthz))
        .route("/metrics", get(metrics::metrics));

    let glucose = Router::new()
        .route("/latest", get(glucose::latest))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_bearer,
        ));

    let api_v1 = Router::new().route("/status", get(status::status));

    public
        .nest("/glucose", glucose)
        .nest("/api/v1", api_v1)
        // Clock View routes are registered at the top level so the bare
        // `/clock` path (no trailing slash) resolves under the Ingress proxy.
        .route("/clock", get(clock::clock_html))
        .route("/clock/state", get(clock::clock_state))
        .route("/clock/events", get(clock::clock_events_sse))
        .with_state(state)
        .layer(axum::middleware::from_fn(add_disclaimer_header))
        .layer(TraceLayer::new_for_http())
}

/// Adds `X-Disclaimer: not-for-medical-use` to every outgoing response,
/// regardless of route or status code. Layered after the routing tree so
/// it covers `/healthz`, `/metrics`, and `/glucose/*` uniformly.
async fn add_disclaimer_header(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    response
        .headers_mut()
        .insert(X_DISCLAIMER_HEADER, X_DISCLAIMER_VALUE);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn state(bearer: Option<&str>) -> AppState {
        let handle = crate::metrics::init_recorder().expect("recorder");
        let (tx, rx) =
            tokio::sync::watch::channel(crate::poll_status::PollStatus::default());
        let (clock_tx, _clock_rx) = broadcast::channel(16);
        AppState {
            cache: ReadingCache::new(),
            metrics_handle: handle,
            bearer_token: bearer.map(|s| SecretString::from(s.to_string())),
            poll_status_tx: Arc::new(tx),
            poll_status_rx: rx,
            clock_tx: Arc::new(clock_tx),
        }
    }

    #[tokio::test]
    async fn healthz_returns_ok() {
        let app = router(state(None));
        let resp = app
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn glucose_passthrough_when_auth_disabled() {
        let app = router(state(None));
        let resp = app
            .oneshot(Request::get("/glucose/latest").body(Body::empty()).unwrap())
            .await
            .unwrap();
        // No bearer set → middleware passthrough; cache empty → 503.
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn glucose_unauthorized_without_header() {
        let app = router(state(Some("supersecret")));
        let resp = app
            .oneshot(Request::get("/glucose/latest").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error_code"], "AUTH001");
    }

    #[tokio::test]
    async fn glucose_unauthorized_with_wrong_token() {
        let app = router(state(Some("supersecret")));
        let resp = app
            .oneshot(
                Request::get("/glucose/latest")
                    .header(header::AUTHORIZATION, "Bearer nope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn glucose_unauthorized_with_non_bearer_scheme() {
        let app = router(state(Some("supersecret")));
        let resp = app
            .oneshot(
                Request::get("/glucose/latest")
                    .header(header::AUTHORIZATION, "Basic supersecret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn glucose_passes_with_correct_token() {
        let app = router(state(Some("supersecret")));
        let resp = app
            .oneshot(
                Request::get("/glucose/latest")
                    .header(header::AUTHORIZATION, "Bearer supersecret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Token accepted; handler runs; cache empty → 503 with API001.
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error_code"], "API001");
    }

    #[tokio::test]
    async fn metrics_remains_public_when_auth_enabled() {
        let app = router(state(Some("supersecret")));
        let resp = app
            .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// `X-Disclaimer` must appear on every API response, including
    /// healthz, metrics, and unauthorised / 503 paths — it's a posture
    /// signal, not a status signal.
    #[tokio::test]
    async fn x_disclaimer_header_present_on_all_responses() {
        let app = router(state(None));
        for path in ["/healthz", "/metrics", "/glucose/latest"] {
            let resp = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            let header = resp
                .headers()
                .get("x-disclaimer")
                .unwrap_or_else(|| panic!("X-Disclaimer missing on {path}"));
            assert_eq!(header.to_str().unwrap(), "not-for-medical-use", "{path}");
        }
    }

    #[tokio::test]
    async fn x_disclaimer_header_present_even_on_401() {
        let app = router(state(Some("supersecret")));
        let resp = app
            .oneshot(Request::get("/glucose/latest").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers()
                .get("x-disclaimer")
                .and_then(|h| h.to_str().ok()),
            Some("not-for-medical-use"),
        );
    }
}
