// SPDX-License-Identifier: AGPL-3.0-or-later

//! Clock View — a single-file responsive glucose display served on the same
//! port as the rest of the HTTP API.
//!
//! Three routes:
//! - `GET /clock`        — the HTML page (config injected server-side).
//! - `GET /clock/state`  — JSON snapshot of the latest reading.
//! - `GET /clock/events` — Server-Sent Events stream of new readings.
//!
//! All three set `Cache-Control: no-store` because they carry PHI. The SSE
//! response additionally sets `X-Accel-Buffering: no` and a keep-alive
//! heartbeat so the Home Assistant Ingress (nginx) proxy does not buffer the
//! stream.

use std::convert::Infallible;
use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::stream::Stream;
use gluco_hub_core::Trend;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use super::AppState;

/// Embedded HTML page. `include_str!` resolves at compile time so there is no
/// runtime file I/O and the binary stays self-contained.
const CLOCK_HTML: &str = include_str!("clock.html");

/// Default low threshold in mg/dL (below this is "low").
const DEFAULT_LO: f64 = 70.0;
/// Default high threshold in mg/dL (above this is "high").
const DEFAULT_HI: f64 = 180.0;
/// SSE keep-alive heartbeat interval. Comfortably under the Ingress/nginx
/// idle timeout so the connection is never reaped between readings.
const KEEPALIVE_SECS: u64 = 15;

/// JSON snapshot returned by `GET /clock/state`.
#[derive(Serialize, Clone)]
pub struct ClockStateDto {
    pub value: f64,
    pub unit: String,
    pub trend: u8,
    pub trend_label: String,
    pub delta: Option<f64>,
    pub timestamp_ms: i64,
    pub poll_interval_ms: u64,
    pub zone: String,
    pub patient_label: Option<String>,
    pub lo: f64,
    pub hi: f64,
}

/// Payload pushed over the SSE stream after each successful reading.
///
/// Constructed by the poll loop and broadcast to all connected clients.
#[derive(Serialize, Clone, Debug)]
pub struct ClockReadingEvent {
    pub value: f64,
    pub unit: String,
    pub trend: u8,
    pub trend_label: String,
    pub delta: Option<f64>,
    pub timestamp_ms: i64,
    pub zone: String,
}

/// Five-zone classification of a glucose value in mg/dL.
///
/// Boundaries are inclusive on the in-range side: a value exactly equal to
/// `lo` or `hi` is "in_range". 54 mg/dL is the clinical urgent-low cutoff and
/// 250 mg/dL the urgent-high cutoff; both are fixed regardless of `lo`/`hi`.
fn glucose_zone(value_mgdl: f64, lo: f64, hi: f64) -> &'static str {
    if value_mgdl < 54.0 {
        "hypo"
    } else if value_mgdl < lo {
        "low"
    } else if value_mgdl <= hi {
        "in_range"
    } else if value_mgdl <= 250.0 {
        "high"
    } else {
        "very_high"
    }
}

/// Map the core `Trend` enum onto a LibreLink-style numeric trend (1-7) and a
/// coarse five-label set. The numeric value runs from 1 (falling fast) to 7
/// (rising fast) so a client can render an arrow angle without a lookup table;
/// non-directional variants collapse to 4 / "stable" as a safe neutral.
fn trend_to_num_label(trend: Trend) -> (u8, &'static str) {
    match trend {
        Trend::DoubleUp => (7, "rising_fast"),
        Trend::SingleUp => (6, "rising"),
        Trend::FortyFiveUp => (5, "rising"),
        Trend::Flat => (4, "stable"),
        Trend::FortyFiveDown => (3, "falling"),
        Trend::SingleDown => (2, "falling"),
        Trend::DoubleDown => (1, "falling_fast"),
        Trend::NotComputable | Trend::RateOutOfRange => (4, "stable"),
    }
}

/// Query parameters accepted by `GET /clock`. All optional; missing values
/// fall back to the documented defaults (`lo=70`, `hi=180`, unit `mgdl`).
#[derive(Debug, Deserialize, Default)]
pub struct ClockQuery {
    eink: Option<String>,
    lo: Option<f64>,
    hi: Option<f64>,
    #[allow(dead_code)]
    kiosk: Option<String>,
    #[allow(dead_code)]
    pin: Option<String>,
    unit: Option<String>,
    dark: Option<String>,
}

/// Returns true when a query flag is set to a truthy token (`1`, `true`, `yes`,
/// or simply present without a value).
fn flag_truthy(v: &Option<String>) -> bool {
    match v {
        Some(s) => {
            let s = s.trim().to_ascii_lowercase();
            s.is_empty() || s == "1" || s == "true" || s == "yes" || s == "on"
        }
        None => false,
    }
}

/// Normalise the `unit` query param to either `mmol` or `mgdl` (the default).
fn normalise_unit(unit: &Option<String>) -> &'static str {
    match unit.as_deref().map(str::trim).map(str::to_ascii_lowercase) {
        Some(ref s) if s == "mmol" || s == "mmoll" || s == "mmol/l" => "mmol",
        _ => "mgdl",
    }
}

/// Attach `Cache-Control: no-store` to a response (PHI must not be retained by
/// intermediate proxies).
fn no_store<R: IntoResponse>(inner: R) -> Response {
    let mut resp = inner.into_response();
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    resp
}

/// `GET /clock` — serve the HTML page with `window.CLOCK_CONFIG` injected just
/// before `</head>`. The config is derived from the query params plus the
/// current poll interval from the status watch channel.
pub async fn clock_html(State(state): State<AppState>, Query(q): Query<ClockQuery>) -> Response {
    let lo = q.lo.unwrap_or(DEFAULT_LO);
    let hi = q.hi.unwrap_or(DEFAULT_HI);
    let unit = normalise_unit(&q.unit);
    let eink = flag_truthy(&q.eink);

    // `dark`: null = auto, 0 = force-light, 1 = force-dark.
    let dark_js = match q.dark.as_deref().map(str::trim) {
        Some("0") => "0",
        Some("1") => "1",
        _ => "null",
    };

    let poll = state.poll_status_rx.borrow().clone();
    let poll_ms = poll.poll_interval_secs.saturating_mul(1000).max(1000);

    // No patient label source exists yet; emit `null` rather than fabricating
    // a name. When a label becomes available it must be the abbreviated form
    // ("Anna M.") — never a full name (PHI).
    let config = format!(
        "<script>window.CLOCK_CONFIG = {{\
unit:{unit},lo:{lo},hi:{hi},patientLabel:null,pollMs:{poll_ms},eink:{eink},dark:{dark}\
}};</script>",
        unit = serde_json::to_string(unit).unwrap_or_else(|_| "\"mgdl\"".into()),
        lo = lo,
        hi = hi,
        poll_ms = poll_ms,
        eink = eink,
        dark = dark_js,
    );

    let html = match CLOCK_HTML.split_once("</head>") {
        Some((head, tail)) => format!("{head}{config}</head>{tail}"),
        // Fallback: prepend the config if the marker is somehow missing.
        None => format!("{config}{CLOCK_HTML}"),
    };

    let mut resp = axum::response::Html(html).into_response();
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    resp
}

/// `GET /clock/state` — JSON snapshot of the latest reading.
///
/// Returns `503 {"error":"no_reading_yet"}` before the first reading. `delta`
/// is `None` because the cache holds a single reading with no history; the SSE
/// stream carries deltas computed by the poll loop instead.
pub async fn clock_state(State(state): State<AppState>, Query(q): Query<ClockQuery>) -> Response {
    let Some(reading) = state.cache.latest() else {
        return no_store((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "no_reading_yet" })),
        ));
    };

    let lo = q.lo.unwrap_or(DEFAULT_LO);
    let hi = q.hi.unwrap_or(DEFAULT_HI);
    let unit = normalise_unit(&q.unit).to_string();

    let poll = state.poll_status_rx.borrow().clone();
    let poll_interval_ms = poll.poll_interval_secs.saturating_mul(1000).max(1000);

    let value = reading.glucose.get();
    let (trend, trend_label) = trend_to_num_label(reading.trend);

    let dto = ClockStateDto {
        value,
        unit,
        trend,
        trend_label: trend_label.to_string(),
        // The single-slot cache has no previous reading, so a delta cannot be
        // derived here without fabricating data. The SSE event carries it.
        delta: None,
        timestamp_ms: reading.timestamp.timestamp_millis(),
        poll_interval_ms,
        zone: glucose_zone(value, lo, hi).to_string(),
        patient_label: None,
        lo,
        hi,
    };

    no_store((StatusCode::OK, Json(dto)))
}

/// `GET /clock/events` — SSE stream of `reading` events.
///
/// Subscribes to the broadcast channel fed by the poll loop. Sets
/// `X-Accel-Buffering: no` plus a keep-alive heartbeat so the Ingress proxy
/// does not buffer or reap the stream. When the `X-Gluco-Eink` request header
/// is `1`, the server throttles events to changes greater than 1 mg/dL or a
/// 5-minute heartbeat interval (e-ink panels should not refresh on noise).
pub async fn clock_events_sse(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let eink = headers
        .get("X-Gluco-Eink")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim() == "1")
        .unwrap_or(false);

    let rx = state.clock_tx.subscribe();
    let stream = reading_event_stream(rx, eink);

    let sse = Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(KEEPALIVE_SECS))
            .text(""),
    );

    let mut resp = sse.into_response();
    let h = resp.headers_mut();
    h.insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    // Disable nginx response buffering in the Ingress proxy.
    h.insert(
        axum::http::HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    resp
}

/// Build the SSE event stream from a broadcast receiver, applying the e-ink
/// throttle when requested. Lagged receivers (slow clients) skip dropped
/// messages and continue rather than tearing down the stream.
fn reading_event_stream(
    rx: broadcast::Receiver<ClockReadingEvent>,
    eink: bool,
) -> impl Stream<Item = Result<Event, Infallible>> {
    // State threaded through the unfold: the receiver, plus the last emitted
    // value and the instant it was emitted (for the e-ink throttle).
    let init = (rx, None::<f64>, std::time::Instant::now());

    futures::stream::unfold(
        init,
        move |(mut rx, mut last_value, mut last_emit)| async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        if eink {
                            let big_change = match last_value {
                                Some(prev) => (ev.value - prev).abs() > 1.0,
                                None => true,
                            };
                            let stale = last_emit.elapsed() > Duration::from_secs(300);
                            if !(big_change || stale) {
                                // Suppress noise; wait for the next reading.
                                continue;
                            }
                            last_value = Some(ev.value);
                            last_emit = std::time::Instant::now();
                        }
                        let data = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".to_string());
                        let event = Event::default().event("reading").data(data);
                        return Some((Ok(event), (rx, last_value, last_emit)));
                    }
                    // Slow consumer: messages were dropped. Skip them and keep the
                    // stream alive — the next reading will resync the display.
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    // Sender gone (server shutting down): end the stream.
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        },
    )
}

/// Build a `ClockReadingEvent` from a reading and the previous value. Called by
/// the poll loop, which is the only place a previous value is available (the
/// cache holds a single slot with no history).
pub fn build_reading_event(
    value_mgdl: f64,
    trend: Trend,
    timestamp_ms: i64,
    prev_value_mgdl: Option<f64>,
    unit: &str,
    lo: f64,
    hi: f64,
) -> ClockReadingEvent {
    let (trend_num, trend_label) = trend_to_num_label(trend);
    ClockReadingEvent {
        value: value_mgdl,
        unit: unit.to_string(),
        trend: trend_num,
        trend_label: trend_label.to_string(),
        delta: prev_value_mgdl.map(|prev| value_mgdl - prev),
        timestamp_ms,
        zone: glucose_zone(value_mgdl, lo, hi).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_thresholds() {
        assert_eq!(glucose_zone(53.9, 70.0, 180.0), "hypo");
        assert_eq!(glucose_zone(69.9, 70.0, 180.0), "low");
        assert_eq!(glucose_zone(70.0, 70.0, 180.0), "in_range");
        assert_eq!(glucose_zone(180.0, 70.0, 180.0), "in_range");
        assert_eq!(glucose_zone(180.1, 70.0, 180.0), "high");
        assert_eq!(glucose_zone(250.1, 70.0, 180.0), "very_high");
    }

    #[test]
    fn zone_boundaries_at_clinical_cutoffs() {
        // 54 is the hypo cutoff (inclusive low end of "low").
        assert_eq!(glucose_zone(54.0, 70.0, 180.0), "low");
        // 250 is the very_high cutoff (inclusive high end of "high").
        assert_eq!(glucose_zone(250.0, 70.0, 180.0), "high");
    }

    #[test]
    fn trend_mapping_covers_all_variants() {
        assert_eq!(trend_to_num_label(Trend::DoubleUp), (7, "rising_fast"));
        assert_eq!(trend_to_num_label(Trend::SingleUp), (6, "rising"));
        assert_eq!(trend_to_num_label(Trend::FortyFiveUp), (5, "rising"));
        assert_eq!(trend_to_num_label(Trend::Flat), (4, "stable"));
        assert_eq!(trend_to_num_label(Trend::FortyFiveDown), (3, "falling"));
        assert_eq!(trend_to_num_label(Trend::SingleDown), (2, "falling"));
        assert_eq!(trend_to_num_label(Trend::DoubleDown), (1, "falling_fast"));
        assert_eq!(trend_to_num_label(Trend::NotComputable), (4, "stable"));
        assert_eq!(trend_to_num_label(Trend::RateOutOfRange), (4, "stable"));
    }

    #[test]
    fn delta_computed_when_previous_available() {
        let ev = build_reading_event(120.0, Trend::Flat, 0, Some(100.0), "mgdl", 70.0, 180.0);
        assert_eq!(ev.delta, Some(20.0));
        assert_eq!(ev.zone, "in_range");
        assert_eq!(ev.trend, 4);
        assert_eq!(ev.trend_label, "stable");
    }

    #[test]
    fn delta_none_when_no_previous() {
        let ev = build_reading_event(120.0, Trend::Flat, 0, None, "mgdl", 70.0, 180.0);
        assert_eq!(ev.delta, None);
    }

    #[test]
    fn delta_negative_on_drop() {
        let ev = build_reading_event(60.0, Trend::SingleDown, 0, Some(95.0), "mgdl", 70.0, 180.0);
        assert_eq!(ev.delta, Some(-35.0));
        assert_eq!(ev.zone, "low");
        assert_eq!(ev.trend_label, "falling");
    }

    #[test]
    fn flag_truthy_accepts_common_tokens() {
        assert!(flag_truthy(&Some("1".into())));
        assert!(flag_truthy(&Some("true".into())));
        assert!(flag_truthy(&Some("".into())));
        assert!(!flag_truthy(&Some("0".into())));
        assert!(!flag_truthy(&None));
    }

    #[test]
    fn normalise_unit_maps_mmol_and_defaults_mgdl() {
        assert_eq!(normalise_unit(&Some("mmol".into())), "mmol");
        assert_eq!(normalise_unit(&Some("MMOL/L".into())), "mmol");
        assert_eq!(normalise_unit(&Some("mgdl".into())), "mgdl");
        assert_eq!(normalise_unit(&None), "mgdl");
    }

    #[tokio::test]
    async fn state_returns_503_before_first_reading() {
        use crate::api::router_with_state;
        use axum::body::Body;
        use axum::http::Request;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let app = router_with_state(test_state(None));
        let resp = app
            .oneshot(Request::get("/clock/state").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let cc = resp
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok());
        assert_eq!(cc, Some("no-store"));
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"], "no_reading_yet");
    }

    #[tokio::test]
    async fn state_returns_reading_with_zone_and_no_store() {
        use crate::api::router_with_state;
        use axum::body::Body;
        use axum::http::Request;
        use chrono::{TimeZone, Utc};
        use gluco_hub_core::{GlucoseMgDl, PatientId, Reading, ReadingCache, SourceId};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let cache = ReadingCache::new();
        cache.update(&[Reading {
            patient_id: PatientId::new("p1").unwrap(),
            source_id: SourceId::new("primary").unwrap(),
            timestamp: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            glucose: GlucoseMgDl::new(123.0).unwrap(),
            trend: Trend::Flat,
        }]);
        let app = router_with_state(test_state(Some(cache)));
        let resp = app
            .oneshot(Request::get("/clock/state").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let cc = resp
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok());
        assert_eq!(cc, Some("no-store"));
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["value"], 123.0);
        assert_eq!(json["zone"], "in_range");
        assert_eq!(json["unit"], "mgdl");
        assert_eq!(json["trend_label"], "stable");
        assert_eq!(json["lo"], 70.0);
        assert_eq!(json["hi"], 180.0);
        assert!(json["delta"].is_null());
        assert!(json["patient_label"].is_null());
    }

    #[tokio::test]
    async fn clock_html_injects_config_and_no_store() {
        use crate::api::router_with_state;
        use axum::body::Body;
        use axum::http::Request;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let app = router_with_state(test_state(None));
        let resp = app
            .oneshot(
                Request::get("/clock?lo=80&hi=160&eink=1&unit=mmol&dark=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let cc = resp
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok());
        assert_eq!(cc, Some("no-store"));
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains("window.CLOCK_CONFIG"));
        assert!(body.contains("lo:80"));
        assert!(body.contains("hi:160"));
        assert!(body.contains("eink:true"));
        assert!(body.contains("\"mmol\""));
        assert!(body.contains("dark:1"));
        // Config must be injected inside <head>.
        let cfg_pos = body.find("window.CLOCK_CONFIG").unwrap();
        let head_close = body.find("</head>").unwrap();
        assert!(cfg_pos < head_close, "config must precede </head>");
    }

    #[tokio::test]
    async fn sse_emits_reading_event() {
        use crate::api::router_with_state;
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let state = test_state(None);
        let tx = state.clock_tx.clone();
        let app = router_with_state(state);

        // Connect, then publish one reading; assert the framed SSE chunk shows
        // the `reading` event with our value.
        let resp = app
            .oneshot(Request::get("/clock/events").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let cc = resp
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok());
        assert_eq!(cc, Some("no-store"));
        assert_eq!(
            resp.headers()
                .get("x-accel-buffering")
                .and_then(|v| v.to_str().ok()),
            Some("no"),
        );

        let _ = tx.send(build_reading_event(
            142.0,
            Trend::SingleUp,
            1_700_000_000_000,
            Some(120.0),
            "mgdl",
            70.0,
            180.0,
        ));

        let mut body = resp.into_body().into_data_stream();
        let chunk = read_first_chunk(&mut body).await;
        // axum frames SSE as `event: <name>\ndata: <json>\n\n`.
        assert!(chunk.contains("event: reading"), "got: {chunk}");
        assert!(chunk.contains("142"), "got: {chunk}");
        assert!(chunk.contains("\"delta\":22"), "got: {chunk}");
    }

    // --- test helpers ---

    fn test_state(cache: Option<gluco_hub_core::ReadingCache>) -> AppState {
        use crate::poll_status::PollStatus;
        let handle = crate::metrics::init_recorder().expect("recorder");
        let (tx, rx) = tokio::sync::watch::channel(PollStatus {
            poll_interval_secs: 60,
            ..Default::default()
        });
        let (clock_tx, _clock_rx) = broadcast::channel(16);
        AppState {
            cache: cache.unwrap_or_default(),
            metrics_handle: handle,
            bearer_token: None,
            poll_status_tx: std::sync::Arc::new(tx),
            poll_status_rx: rx,
            clock_tx: std::sync::Arc::new(clock_tx),
        }
    }

    /// Read framed SSE bytes until a non-heartbeat data chunk arrives.
    async fn read_first_chunk(body: &mut axum::body::BodyDataStream) -> String {
        use futures::StreamExt;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let next = tokio::time::timeout_at(deadline, body.next()).await;
            let frame = next.expect("timed out waiting for SSE chunk");
            let bytes = frame.expect("stream ended").expect("chunk error");
            let s = String::from_utf8_lossy(&bytes).to_string();
            if s.contains("event:") {
                return s;
            }
            // Heartbeat / keep-alive comment — keep reading.
        }
    }
}
