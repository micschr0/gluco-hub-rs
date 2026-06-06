// SPDX-License-Identifier: AGPL-3.0-or-later

use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};

use super::AppState;

/// `GET /metrics` — Prometheus text exposition (version 0.0.4).
///
/// Always public. The endpoint is idempotent and safe to scrape; it returns
/// the empty registry until the poller has written its first sample.
pub async fn metrics(State(state): State<AppState>) -> Response {
    let body = state.metrics_handle.render();
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::router_with_state;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gluco_hub_core::ReadingCache;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn metrics_returns_prometheus_text() {
        let handle = crate::metrics::init_recorder().expect("recorder");
        let (tx, rx) = tokio::sync::watch::channel(crate::poll_status::PollStatus::default());
        let (clock_tx, _clock_rx) = tokio::sync::broadcast::channel(16);
        let state = AppState {
            cache: ReadingCache::new(),
            metrics_handle: handle,
            bearer_token: None,
            poll_status_tx: std::sync::Arc::new(tx),
            poll_status_rx: rx,
            clock_tx: std::sync::Arc::new(clock_tx),
        };
        let app = router_with_state(state);
        let resp = app
            .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("content-type")
            .to_str()
            .unwrap()
            .to_string();
        assert!(ct.starts_with("text/plain"));
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        // Body is text — empty registry is allowed; assert it's UTF-8.
        assert!(std::str::from_utf8(&bytes).is_ok());
    }
}
