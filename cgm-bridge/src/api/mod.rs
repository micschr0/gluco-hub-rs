use axum::Router;
use axum::routing::get;
use tower_http::trace::TraceLayer;

mod health;

/// Build the public HTTP router. Endpoints added in later iterations
/// (`/glucose/latest`, `/metrics`) will be wired here.
pub fn router() -> Router {
    Router::new()
        .route("/healthz", get(health::healthz))
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
        let app = router();
        let resp = app
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
