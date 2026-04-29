use axum::Router;
use axum::routing::get;
use cgm_bridge_core::ReadingCache;
use metrics_exporter_prometheus::PrometheusHandle;
use secrecy::SecretString;
use tower_http::trace::TraceLayer;

mod auth;
mod glucose;
mod health;
mod metrics;

/// Shared application state passed to handlers via `State<AppState>`.
///
/// `bearer_token` is `Some` only when `[http] bearer_token_env` resolved
/// to a non-empty value at startup. The auth middleware checks this on
/// every request and short-circuits to `401` on mismatch.
#[derive(Clone)]
pub struct AppState {
    pub cache: ReadingCache,
    pub metrics_handle: PrometheusHandle,
    pub bearer_token: Option<SecretString>,
}

/// Build the public HTTP router with state.
///
/// Routing layout:
/// - `/healthz` and `/metrics` are always public.
/// - `/glucose/*` runs through the Bearer middleware. If
///   `bearer_token` is `None` the middleware short-circuits to
///   passthrough so unauthenticated local-dev usage still works.
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

    public
        .nest("/glucose", glucose)
        .with_state(state)
        .layer(TraceLayer::new_for_http())
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
        AppState {
            cache: ReadingCache::new(),
            metrics_handle: handle,
            bearer_token: bearer.map(|s| SecretString::from(s.to_string())),
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
}
