// SPDX-License-Identifier: AGPL-3.0-or-later

//! `NightscoutSink` — implements `Sink` over `NightscoutClient`.
//!
//! The trait error type is `CoreError`, but we want the inner LLU/NS
//! `error_code` ("NS001"…"NS005") to survive into metrics labels. We
//! embed the code as a `[NSxxx]` prefix in `CoreError::Sink::message`
//! so the poll-loop fan-out can extract it without coupling to NS
//! types.
//!
//! Pre-upload dedup: each push first reads Nightscout's most recent
//! entry timestamp via `GET /api/v1/entries.json?count=1` and drops every
//! reading whose `timestamp` is at or before that high-water mark. NS
//! still dedupes
//! server-side by `date+type`, but doing it here saves bandwidth and
//! keeps the NS access log readable. If the high-water-mark fetch
//! fails for any reason (transport, 5xx, malformed body), the sink
//! falls back to **post-all** and logs a warn — graceful degradation
//! is the safer choice than skipping the upload entirely.

use async_trait::async_trait;
use gluco_hub_core::{CoreError, Reading, Sink};
use tracing::{debug, warn};

use super::client::NightscoutClient;

pub struct NightscoutSink {
    client: NightscoutClient,
}

impl NightscoutSink {
    pub fn new(client: NightscoutClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Sink for NightscoutSink {
    fn name(&self) -> &'static str {
        "nightscout"
    }

    async fn push(&self, readings: &[Reading]) -> Result<(), CoreError> {
        if readings.is_empty() {
            return Ok(());
        }

        let high_water = match self.client.fetch_last_entry_date().await {
            Ok(opt) => opt,
            Err(e) => {
                warn!(
                    error_code = e.error_code(),
                    error = %e,
                    "ns lastEntry fetch failed; falling back to post-all"
                );
                None
            }
        };

        // Filter: keep only readings strictly newer than the high-water
        // mark. `None` (no prior entries / lookup failed) → keep all.
        let filtered: Vec<Reading> = if let Some(hw) = high_water {
            readings
                .iter()
                .filter(|r| r.timestamp.timestamp_millis() > hw)
                .cloned()
                .collect()
        } else {
            readings.to_vec()
        };

        let kept = filtered.len();
        let dropped = readings.len() - kept;
        debug!(kept, dropped, high_water = ?high_water, "ns dedup");
        if dropped > 0 {
            ::metrics::counter!(
                "cgm_sink_dedup_skipped_total",
                "sink" => "nightscout",
            )
            .increment(dropped as u64);
        }

        if filtered.is_empty() {
            // Nothing to upload — NS already has every reading.
            return Ok(());
        }

        self.client
            .post_entries(&filtered)
            .await
            .map_err(|e| CoreError::Sink {
                message: e.to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use gluco_hub_core::{GlucoseMgDl, PatientId, SourceId, Trend};
    use secrecy::SecretString;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn reading_at(secs: i64, value: f64) -> Reading {
        Reading {
            patient_id: PatientId::new("p1").unwrap(),
            source_id: SourceId::new("llu").unwrap(),
            timestamp: Utc.timestamp_opt(secs, 0).unwrap(),
            glucose: GlucoseMgDl::new(value).unwrap(),
            trend: Trend::Flat,
        }
    }

    fn one_reading() -> Reading {
        reading_at(1_700_000_000, 120.0)
    }

    fn build_sink(server: &MockServer) -> NightscoutSink {
        let client =
            NightscoutClient::new(server.uri(), SecretString::from("test-secret")).expect("client");
        NightscoutSink::new(client)
    }

    /// Mount a 404 on `GET /api/v1/entries.json` so `fetch_last_entry_date`
    /// returns `Ok(None)` (registry-empty path) — keeps the POST-side
    /// tests focused without exercising dedup logic in every case.
    async fn mount_empty_last_entry(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/api/v1/entries.json"))
            .respond_with(ResponseTemplate::new(404))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn push_happy_path() {
        let server = MockServer::start().await;
        mount_empty_last_entry(&server).await;
        Mock::given(method("POST"))
            .and(path("/api/v1/entries"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        build_sink(&server)
            .push(&[one_reading()])
            .await
            .expect("push");
    }

    #[tokio::test]
    async fn push_502_carries_ns004_in_message() {
        let server = MockServer::start().await;
        mount_empty_last_entry(&server).await;
        Mock::given(method("POST"))
            .and(path("/api/v1/entries"))
            .respond_with(ResponseTemplate::new(502))
            .mount(&server)
            .await;
        let err = build_sink(&server)
            .push(&[one_reading()])
            .await
            .unwrap_err();
        let CoreError::Sink { message } = err else {
            panic!("expected CoreError::Sink");
        };
        assert!(message.starts_with("[NS004]"), "got: {message}");
    }

    #[tokio::test]
    async fn push_401_carries_ns002() {
        let server = MockServer::start().await;
        mount_empty_last_entry(&server).await;
        Mock::given(method("POST"))
            .and(path("/api/v1/entries"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let err = build_sink(&server)
            .push(&[one_reading()])
            .await
            .unwrap_err();
        let CoreError::Sink { message } = err else {
            panic!("expected CoreError::Sink");
        };
        assert!(message.starts_with("[NS002]"), "got: {message}");
    }

    #[tokio::test]
    async fn push_empty_batch_is_noop() {
        let server = MockServer::start().await;
        // No mocks needed; empty-batch path short-circuits before any
        // HTTP call.
        build_sink(&server).push(&[]).await.expect("noop");
    }

    #[tokio::test]
    async fn dedup_drops_readings_at_or_below_last_entry_date() {
        let server = MockServer::start().await;
        // High-water mark sits between the two readings.
        let mid_ms = Utc
            .timestamp_opt(1_700_000_500, 0)
            .unwrap()
            .timestamp_millis();
        Mock::given(method("GET"))
            .and(path("/api/v1/entries.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": [{ "date": mid_ms }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/entries"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;

        let older = reading_at(1_700_000_000, 100.0); // dropped
        let newer = reading_at(1_700_001_000, 110.0); // kept
        build_sink(&server)
            .push(&[older, newer.clone()])
            .await
            .expect("push");

        // Inspect the POST body — only the newer reading made it through.
        let req = server
            .received_requests()
            .await
            .expect("requests")
            .into_iter()
            .find(|r| r.method.as_str() == "POST" && r.url.path() == "/api/v1/entries")
            .expect("POST happened");
        let body: serde_json::Value = serde_json::from_slice(&req.body).expect("json");
        let arr = body.as_array().expect("array");
        assert_eq!(arr.len(), 1, "exactly one entry survives dedup");
        assert_eq!(arr[0]["sgv"], 110);
    }

    #[tokio::test]
    async fn dedup_skips_post_entirely_when_everything_is_already_uploaded() {
        let server = MockServer::start().await;
        // High-water mark is newer than every reading we hold.
        let future_ms = Utc
            .timestamp_opt(2_000_000_000, 0)
            .unwrap()
            .timestamp_millis();
        Mock::given(method("GET"))
            .and(path("/api/v1/entries.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": [{ "date": future_ms }]
            })))
            .mount(&server)
            .await;
        // Deliberately NO POST mock — if the sink tries to upload, the
        // request hits a 404 and the test fails.

        build_sink(&server)
            .push(&[reading_at(1_700_000_000, 120.0)])
            .await
            .expect("push");
        let posts = server
            .received_requests()
            .await
            .expect("requests")
            .into_iter()
            .filter(|r| r.method.as_str() == "POST" && r.url.path() == "/api/v1/entries")
            .count();
        assert_eq!(posts, 0, "no POST when every reading is already known");
    }

    #[tokio::test]
    async fn last_entry_fetch_500_falls_back_to_post_all() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/entries.json"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/entries"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;

        // Two readings; on dedup-failure both should reach the POST.
        build_sink(&server)
            .push(&[
                reading_at(1_700_000_000, 100.0),
                reading_at(1_700_000_500, 110.0),
            ])
            .await
            .expect("push");

        let req = server
            .received_requests()
            .await
            .expect("requests")
            .into_iter()
            .find(|r| r.method.as_str() == "POST")
            .expect("POST happened");
        let body: serde_json::Value = serde_json::from_slice(&req.body).expect("json");
        assert_eq!(body.as_array().unwrap().len(), 2);
    }
}
