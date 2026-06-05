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

use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
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

/// Maximum number of readings retained for the Clock View sparkline.
///
/// Sized for six hours at the default 60-second poll interval. The polished
/// sparkline only renders the most recent three hours (`HISTORY_MAX = 180` on
/// the client), so this leaves comfortable headroom and still bounds memory to
/// a few kilobytes regardless of uptime.
pub const HISTORY_CAP: usize = 360;

/// One point in the bounded readings history that backs `GET /clock/history`.
///
/// Field names are chosen to match the shape the polished sparkline consumes
/// verbatim (`{ ts, mgdl }`) so the handler needs no client-side adaptation.
/// `ts` is Unix epoch milliseconds (same basis as `ClockReadingEvent.timestamp_ms`)
/// and `mgdl` is the raw mg/dL value; the client converts to mmol/L for display.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
pub struct HistoryPoint {
    pub ts: i64,
    pub mgdl: f64,
}

/// Shared, cheaply-cloneable bounded history store for the sparkline.
///
/// The poll loop pushes one point per successful reading via [`push_history`];
/// `GET /clock/history` snapshots it. A `Mutex` is sufficient — writes happen
/// once per poll interval and reads once per page load, so contention is nil.
pub type ClockHistory = Arc<Mutex<VecDeque<HistoryPoint>>>;

/// Construct an empty history store sized to [`HISTORY_CAP`].
pub fn new_history() -> ClockHistory {
    Arc::new(Mutex::new(VecDeque::with_capacity(HISTORY_CAP)))
}

/// The two Clock View write-side dependencies the poll loop feeds after each
/// successful reading: the SSE broadcast sender and the sparkline history
/// buffer. Bundling them keeps `poll_loop`'s argument list within bounds and
/// makes "publish a reading to the Clock View" a single concern.
#[derive(Clone)]
pub struct ClockSink {
    pub tx: Arc<broadcast::Sender<ClockReadingEvent>>,
    pub history: ClockHistory,
}

impl ClockSink {
    /// Publish a reading to Clock View subscribers and append it to the
    /// sparkline history. The broadcast `send` error (no subscribers) is
    /// intentionally ignored — the channel is held open by `AppState`.
    pub fn publish(&self, event: ClockReadingEvent) {
        let ts_ms = event.timestamp_ms;
        let value = event.value;
        let _ = self.tx.send(event);
        push_history(&self.history, ts_ms, value);
    }
}

/// Append a reading to the bounded history, evicting the oldest point once the
/// buffer is full. Called by the poll loop alongside the SSE broadcast so the
/// sparkline series and the live value share a single source of truth.
///
/// A poisoned lock (a panic while another thread held it) is recovered rather
/// than propagated — losing one history point is preferable to taking down the
/// poll loop.
pub fn push_history(history: &ClockHistory, ts: i64, mgdl: f64) {
    let mut buf = match history.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if buf.len() >= HISTORY_CAP {
        buf.pop_front();
    }
    buf.push_back(HistoryPoint { ts, mgdl });
}

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

/// `GET /clock/history` — JSON array of recent `{ ts, mgdl }` points for the
/// Clock View sparkline.
///
/// Returns the bounded ring buffer (oldest first) exactly as the polished
/// sparkline consumes it: each element is `{ "ts": <epoch_ms>, "mgdl": <value> }`.
/// An empty buffer (before the first reading) returns `200 []`, not `503` — the
/// sparkline treats history as optional and renders a placeholder. Sets
/// `Cache-Control: no-store` because the series is PHI.
pub async fn clock_history(State(state): State<AppState>) -> Response {
    let points: Vec<HistoryPoint> = match state.clock_history.lock() {
        Ok(g) => g.iter().copied().collect(),
        Err(poisoned) => poisoned.into_inner().iter().copied().collect(),
    };
    no_store((StatusCode::OK, Json(points)))
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

    /// `GET /clock` content-type + query-reflected config. The existing
    /// `clock_html_injects_config_and_no_store` test covers a different param
    /// set and does not assert `Content-Type`; this locks the HTML media type
    /// and the spec's `?lo=80&hi=200&unit=mmol&eink=1` reflection.
    #[tokio::test]
    async fn clock_html_sets_text_html_content_type_and_reflects_query() {
        use crate::api::router_with_state;
        use axum::body::Body;
        use axum::http::{Request, header};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let app = router_with_state(test_state(None));
        let resp = app
            .oneshot(
                Request::get("/clock?lo=80&hi=200&unit=mmol&eink=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let content_type = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .expect("Content-Type header present");
        assert!(
            content_type.starts_with("text/html"),
            "Content-Type must be text/html, got: {content_type}"
        );
        assert_eq!(
            resp.headers()
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some("no-store"),
        );

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains("window.CLOCK_CONFIG"));
        assert!(body.contains("lo:80"), "lo must reflect query: {body}");
        assert!(body.contains("hi:200"), "hi must reflect query: {body}");
        assert!(body.contains("\"mmol\""), "unit must reflect query");
        assert!(body.contains("eink:true"), "eink must reflect query");
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

    #[test]
    fn push_history_evicts_oldest_when_full() {
        let h = new_history();
        // Fill exactly to capacity with monotonically increasing values.
        for i in 0..HISTORY_CAP {
            push_history(&h, i as i64, i as f64);
        }
        {
            let buf = h.lock().unwrap();
            assert_eq!(buf.len(), HISTORY_CAP);
            assert_eq!(buf.front().copied(), Some(HistoryPoint { ts: 0, mgdl: 0.0 }));
            let last = (HISTORY_CAP - 1) as i64;
            assert_eq!(
                buf.back().copied(),
                Some(HistoryPoint {
                    ts: last,
                    mgdl: last as f64
                })
            );
        }
        // One more push evicts the oldest (ts=0) and appends the newest.
        push_history(&h, 9_999, 9_999.0);
        let buf = h.lock().unwrap();
        assert_eq!(buf.len(), HISTORY_CAP, "cap is enforced, not exceeded");
        assert_eq!(
            buf.front().copied(),
            Some(HistoryPoint {
                ts: 1,
                mgdl: 1.0
            }),
            "oldest point was evicted"
        );
        assert_eq!(
            buf.back().copied(),
            Some(HistoryPoint {
                ts: 9_999,
                mgdl: 9_999.0
            }),
            "newest point is at the back"
        );
    }

    #[tokio::test]
    async fn history_empty_returns_200_empty_array_no_store() {
        use crate::api::router_with_state;
        use axum::body::Body;
        use axum::http::Request;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let app = router_with_state(test_state(None));
        let resp = app
            .oneshot(Request::get("/clock/history").body(Body::empty()).unwrap())
            .await
            .unwrap();
        // Empty buffer is a 200 with [], NOT a 503 — history is optional.
        assert_eq!(resp.status(), StatusCode::OK);
        let cc = resp
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok());
        assert_eq!(cc, Some("no-store"));
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json, serde_json::json!([]));
    }

    #[tokio::test]
    async fn history_returns_points_in_order_with_expected_shape() {
        use crate::api::router_with_state;
        use axum::body::Body;
        use axum::http::Request;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let state = test_state(None);
        // Seed three points out of nothing, oldest first.
        push_history(&state.clock_history, 1_000, 100.0);
        push_history(&state.clock_history, 2_000, 110.0);
        push_history(&state.clock_history, 3_000, 95.0);

        let app = router_with_state(state);
        let resp = app
            .oneshot(Request::get("/clock/history").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // Exact wire shape the sparkline consumes: [{ ts, mgdl }, ...].
        assert_eq!(
            json,
            serde_json::json!([
                { "ts": 1_000, "mgdl": 100.0 },
                { "ts": 2_000, "mgdl": 110.0 },
                { "ts": 3_000, "mgdl": 95.0 },
            ])
        );
    }

    #[tokio::test]
    async fn history_endpoint_caps_at_history_cap() {
        use crate::api::router_with_state;
        use axum::body::Body;
        use axum::http::Request;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let state = test_state(None);
        // Push more than the cap; the endpoint must return exactly HISTORY_CAP
        // points and the oldest must have been evicted.
        let total = HISTORY_CAP + 25;
        for i in 0..total {
            push_history(&state.clock_history, i as i64, (i % 400) as f64);
        }

        let app = router_with_state(state);
        let resp = app
            .oneshot(Request::get("/clock/history").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let arr = json.as_array().expect("array");
        assert_eq!(arr.len(), HISTORY_CAP);
        // First retained point is the (total - HISTORY_CAP)th pushed.
        assert_eq!(arr[0]["ts"], (total - HISTORY_CAP) as i64);
        // Last retained point is the most recent push.
        assert_eq!(arr[HISTORY_CAP - 1]["ts"], (total - 1) as i64);
    }

    /// SSE field-set lock: a `ClockReadingEvent` published directly on
    /// `clock_tx` must arrive as a `reading` frame whose `data:` line is the
    /// full event JSON — every field the Clock View client reads
    /// (`value`, `unit`, `trend`, `trend_label`, `delta`, `timestamp_ms`,
    /// `zone`). Complements `sse_emits_reading_event`, which only spot-checks
    /// value + delta; this asserts the complete contract by parsing the JSON.
    #[tokio::test]
    async fn sse_reading_event_carries_full_json_field_set() {
        use crate::api::router_with_state;
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let state = test_state(None);
        let tx = state.clock_tx.clone();
        let app = router_with_state(state);

        let resp = app
            .oneshot(Request::get("/clock/events").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Publish the event directly (not via build_reading_event) so the
        // asserted field values are exactly what the poll loop would send.
        let _ = tx.send(ClockReadingEvent {
            value: 142.0,
            unit: "mmol".to_string(),
            trend: 6,
            trend_label: "rising".to_string(),
            delta: Some(22.0),
            timestamp_ms: 1_700_000_000_000,
            zone: "in_range".to_string(),
        });

        let mut body = resp.into_body().into_data_stream();
        let chunk = read_first_chunk(&mut body).await;
        assert!(chunk.contains("event: reading"), "got: {chunk}");

        let json = parse_sse_data(&chunk);
        assert_eq!(json["value"], 142.0);
        assert_eq!(json["unit"], "mmol");
        assert_eq!(json["trend"], 6);
        assert_eq!(json["trend_label"], "rising");
        assert_eq!(json["delta"], 22.0);
        assert_eq!(json["timestamp_ms"], 1_700_000_000_000i64);
        assert_eq!(json["zone"], "in_range");
    }

    /// E-ink throttle: with `X-Gluco-Eink: 1`, a reading whose |delta| from
    /// the last emitted value is <= 1.0 (and within the 300 s heartbeat
    /// window) is suppressed, while a |delta| > 1.0 passes through.
    ///
    /// Drives the live `clock_events_sse` handler so the assertion exercises
    /// the real header parsing + stream wiring, not just the combinator:
    ///   1. first reading (120.0)  -> always emitted (no prior value)
    ///   2. +0.5 (120.5)           -> suppressed (small change, fresh)
    ///   3. +5.0 over #1 (125.0)   -> emitted (big change)
    /// Because #2 is suppressed, the SECOND `reading` frame the client sees
    /// must be 125.0, never 120.5.
    #[tokio::test]
    async fn sse_eink_throttle_suppresses_small_delta_passes_large() {
        use crate::api::router_with_state;
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let state = test_state(None);
        let tx = state.clock_tx.clone();
        let app = router_with_state(state);

        let resp = app
            .oneshot(
                Request::get("/clock/events")
                    .header("X-Gluco-Eink", "1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let mk = |v: f64| ClockReadingEvent {
            value: v,
            unit: "mgdl".to_string(),
            trend: 4,
            trend_label: "stable".to_string(),
            delta: None,
            timestamp_ms: 1_700_000_000_000,
            zone: "in_range".to_string(),
        };

        let mut body = resp.into_body().into_data_stream();

        // #1 establishes the baseline — first reading always emits.
        let _ = tx.send(mk(120.0));
        let first = read_first_chunk(&mut body).await;
        assert!(
            parse_sse_data(&first)["value"] == 120.0,
            "first reading must emit: {first}"
        );

        // #2 (+0.5) must be suppressed; #3 (+5.0) must pass. Send both before
        // reading so the next emitted frame is unambiguously #3.
        let _ = tx.send(mk(120.5)); // suppressed: |0.5| <= 1.0, fresh
        let _ = tx.send(mk(125.0)); // emitted: |5.0| > 1.0
        let second = read_first_chunk(&mut body).await;
        let json = parse_sse_data(&second);
        assert_eq!(
            json["value"], 125.0,
            "small-delta reading must be skipped; next emit is the big change: {second}"
        );
    }

    // --- test helpers ---

    /// Extract the JSON object from an SSE frame's `data:` line.
    fn parse_sse_data(chunk: &str) -> serde_json::Value {
        let data_line = chunk
            .lines()
            .find_map(|l| l.strip_prefix("data:"))
            .unwrap_or_else(|| panic!("no data line in SSE chunk: {chunk}"))
            .trim();
        serde_json::from_str(data_line)
            .unwrap_or_else(|e| panic!("data line is not JSON ({e}): {data_line}"))
    }

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
            clock_history: new_history(),
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
