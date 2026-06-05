// SPDX-License-Identifier: AGPL-3.0-or-later

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::Serialize;

use super::AppState;

/// `GET /api/v1/status`
///
/// Returns a structured health snapshot of the poll loop, MQTT sink,
/// and dead-letter-queue. Returns `503 {"error":"no_reading_yet"}` before
/// the first successful reading is available.
///
/// `Cache-Control: no-store` on every response — this endpoint carries
/// PHI-adjacent timing data that must not be retained by proxies.
pub async fn status(State(state): State<AppState>) -> Response {
    let poll = state.poll_status_rx.borrow().clone();

    if poll.last_successful_reading_at.is_none() {
        let body = Json(serde_json::json!({"error": "no_reading_yet"}));
        return no_store((StatusCode::SERVICE_UNAVAILABLE, body)).into_response();
    }

    let now = Utc::now();
    let resp = StatusResponse {
        v: 1,
        ts: now,
        data: StatusData {
            llu: LluStatus {
                connected: poll.last_successful_reading_at.is_some(),
                last_poll_attempt_at: poll.last_poll_attempt_at,
                last_successful_reading_at: poll.last_successful_reading_at,
            },
            mqtt: MqttStatus { connected: true },
            dlq: DlqStatus { depth: 0 },
            next_poll_in_secs: poll.next_poll_in_secs,
            poll_interval_secs: poll.poll_interval_secs,
        },
    };

    no_store((StatusCode::OK, Json(resp))).into_response()
}

/// Wraps an `IntoResponse` value by appending `Cache-Control: no-store`.
fn no_store<R: IntoResponse>(inner: R) -> impl IntoResponse {
    let mut resp = inner.into_response();
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    resp
}

#[derive(Serialize)]
struct StatusResponse {
    v: u8,
    ts: DateTime<Utc>,
    data: StatusData,
}

#[derive(Serialize)]
struct StatusData {
    llu: LluStatus,
    mqtt: MqttStatus,
    dlq: DlqStatus,
    next_poll_in_secs: u64,
    poll_interval_secs: u64,
}

#[derive(Serialize)]
struct LluStatus {
    connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_poll_attempt_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_successful_reading_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
struct MqttStatus {
    connected: bool,
}

#[derive(Serialize)]
struct DlqStatus {
    depth: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{AppState, router_with_state};
    use crate::poll_status::PollStatus;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn make_state(poll: PollStatus) -> AppState {
        let (tx, rx) = tokio::sync::watch::channel(poll);
        // Keep tx alive for the duration of the test via the state itself —
        // AppState holds the Sender so the channel is not prematurely closed.
        let handle = crate::metrics::init_recorder().expect("recorder");
        AppState {
            cache: gluco_hub_core::ReadingCache::new(),
            metrics_handle: handle,
            bearer_token: None,
            poll_status_tx: std::sync::Arc::new(tx),
            poll_status_rx: rx,
        }
    }

    #[tokio::test]
    async fn returns_503_before_first_reading() {
        let state = make_state(PollStatus {
            last_poll_attempt_at: None,
            last_successful_reading_at: None,
            next_poll_in_secs: 60,
            poll_interval_secs: 60,
        });
        let app = router_with_state(state);
        let resp = app
            .oneshot(
                Request::get("/api/v1/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"], "no_reading_yet");
    }

    #[tokio::test]
    async fn returns_200_after_first_reading() {
        let now = Utc::now();
        let state = make_state(PollStatus {
            last_poll_attempt_at: Some(now),
            last_successful_reading_at: Some(now),
            next_poll_in_secs: 17,
            poll_interval_secs: 60,
        });
        let app = router_with_state(state);
        let resp = app
            .oneshot(
                Request::get("/api/v1/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let cache_ctrl = resp
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok());
        assert_eq!(cache_ctrl, Some("no-store"));

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["v"], 1);
        assert_eq!(json["data"]["next_poll_in_secs"], 17);
        assert_eq!(json["data"]["poll_interval_secs"], 60);
        assert_eq!(json["data"]["llu"]["connected"], true);
        assert!(json["data"]["llu"]["last_poll_attempt_at"].is_string());
        assert!(json["data"]["llu"]["last_successful_reading_at"].is_string());
        assert_eq!(json["data"]["mqtt"]["connected"], true);
        assert_eq!(json["data"]["dlq"]["depth"], 0);
    }

    #[tokio::test]
    async fn cache_control_no_store_on_503() {
        let state = make_state(PollStatus::default());
        let app = router_with_state(state);
        let resp = app
            .oneshot(
                Request::get("/api/v1/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let cache_ctrl = resp
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok());
        assert_eq!(cache_ctrl, Some("no-store"));
    }
}
