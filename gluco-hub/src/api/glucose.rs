// SPDX-License-Identifier: AGPL-3.0-or-later

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use gluco_hub_core::Reading;
use serde::Serialize;

use super::AppState;

#[derive(Serialize)]
struct ApiError {
    error_code: &'static str,
    message: &'static str,
}

/// `GET /glucose/latest`
///
/// Returns the most recent reading observed across all sources, or
/// `503 Service Unavailable` (`error_code: "API001"`) when the cache has
/// not yet been populated by the poller.
pub async fn latest(State(state): State<AppState>) -> Response {
    match state.cache.latest() {
        Some(reading) => Json(ReadingDto::from(reading)).into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError {
                error_code: "API001",
                message: "no readings available yet",
            }),
        )
            .into_response(),
    }
}

#[derive(Serialize)]
struct ReadingDto {
    patient_id: String,
    source_id: String,
    timestamp: chrono::DateTime<chrono::Utc>,
    glucose_mgdl: f64,
    trend: gluco_hub_core::Trend,
}

impl From<Reading> for ReadingDto {
    fn from(r: Reading) -> Self {
        Self {
            patient_id: r.patient_id.as_str().to_string(),
            source_id: r.source_id.as_str().to_string(),
            timestamp: r.timestamp,
            glucose_mgdl: r.glucose.get(),
            trend: r.trend,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::router_with_state;
    use axum::body::Body;
    use axum::http::Request;
    use chrono::{TimeZone, Utc};
    use gluco_hub_core::{GlucoseMgDl, PatientId, ReadingCache, SourceId, Trend};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn returns_503_when_cache_empty() {
        let handle = crate::metrics::init_recorder().expect("recorder");
        let (tx, rx) =
            tokio::sync::watch::channel(crate::poll_status::PollStatus::default());
        let state = AppState {
            cache: ReadingCache::new(),
            metrics_handle: handle,
            bearer_token: None,
            poll_status_tx: std::sync::Arc::new(tx),
            poll_status_rx: rx,
        };
        let app = router_with_state(state);
        let resp = app
            .oneshot(Request::get("/glucose/latest").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error_code"], "API001");
    }

    #[tokio::test]
    async fn returns_reading_when_cache_populated() {
        let cache = ReadingCache::new();
        cache.update(&[Reading {
            patient_id: PatientId::new("p1").unwrap(),
            source_id: SourceId::new("primary").unwrap(),
            timestamp: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            glucose: GlucoseMgDl::new(123.0).unwrap(),
            trend: Trend::Flat,
        }]);
        let handle = crate::metrics::init_recorder().expect("recorder");
        let (tx, rx) =
            tokio::sync::watch::channel(crate::poll_status::PollStatus::default());
        let app = router_with_state(AppState {
            cache: cache.clone(),
            metrics_handle: handle,
            bearer_token: None,
            poll_status_tx: std::sync::Arc::new(tx),
            poll_status_rx: rx,
        });
        let resp = app
            .oneshot(Request::get("/glucose/latest").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["glucose_mgdl"], 123.0);
        assert_eq!(json["patient_id"], "p1");
        assert_eq!(json["source_id"], "primary");
        assert!(
            json.get("disclaimer").is_none(),
            "disclaimer field should have been removed: {json}"
        );
    }
}
