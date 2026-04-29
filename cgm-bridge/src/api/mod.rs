use axum::Router;
use axum::routing::get;
use cgm_bridge_core::ReadingCache;
use tower_http::trace::TraceLayer;

mod glucose;
mod health;

/// Shared application state passed to handlers via `State<AppState>`.
#[derive(Clone)]
pub struct AppState {
    pub cache: ReadingCache,
}

/// Build the public HTTP router with state. The `/metrics` endpoint and
/// optional Bearer auth land in later iterations.
pub fn router(state: AppState) -> Router {
    router_with_state(state)
}

pub(crate) fn router_with_state(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/glucose/latest", get(glucose::latest))
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
        let app = router(AppState {
            cache: ReadingCache::new(),
        });
        let resp = app
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
