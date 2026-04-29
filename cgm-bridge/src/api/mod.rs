use axum::Router;
use axum::routing::get;
use cgm_bridge_core::ReadingCache;
use metrics_exporter_prometheus::PrometheusHandle;
use tower_http::trace::TraceLayer;

mod glucose;
mod health;
mod metrics;

/// Shared application state passed to handlers via `State<AppState>`.
#[derive(Clone)]
pub struct AppState {
    pub cache: ReadingCache,
    pub metrics_handle: PrometheusHandle,
}

/// Build the public HTTP router with state. Bearer auth lands in a later
/// iteration.
pub fn router(state: AppState) -> Router {
    router_with_state(state)
}

pub(crate) fn router_with_state(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/glucose/latest", get(glucose::latest))
        .route("/metrics", get(metrics::metrics))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn healthz_returns_ok() {
        let handle = crate::metrics::init_recorder().expect("recorder");
        let app = router(AppState {
            cache: ReadingCache::new(),
            metrics_handle: handle,
        });
        let resp = app
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
