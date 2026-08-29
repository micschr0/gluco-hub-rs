// SPDX-License-Identifier: AGPL-3.0-or-later

//! `NightscoutSink` — implements `Sink` over `NightscoutClient`.
//!
//! The trait error type is `CoreError`, but we want the inner LLU/NS
//! `error_code` ("NS001"…"NS005") to survive into metrics labels. We
//! embed the code as a `[NSxxx]` prefix in `CoreError::Sink::message`
//! so the poll-loop fan-out can extract it without coupling to NS
//! types.
//!
//! No client-side dedup here: `SinkRouter` (the caller) already filters
//! each batch down to readings strictly newer than that *source's own*
//! watermark before calling `push`. A sink-local pre-upload check via
//! `GET /api/v1/entries.json?count=1` (dropped 2026-08) used the wrong
//! high-water mark in multi-source deployments — that endpoint returns
//! the NS-instance-global newest entry across every patient/source
//! sharing this NS instance, so one source's advancing timestamp could
//! silently filter out another source's genuinely-new-to-NS readings
//! before they were ever POSTed. NS still dedupes server-side by
//! `date+type`, so a redundant re-POST (e.g. after a router watermark
//! reset on restart) is a harmless no-op there.

use async_trait::async_trait;
use gluco_hub_core::{CoreError, Reading, Sink};

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
        self.client
            .post_entries(readings)
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

    #[tokio::test]
    async fn push_happy_path() {
        let server = MockServer::start().await;
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
    async fn push_sends_every_reading_no_client_side_filtering() {
        // No pre-upload dedup here — SinkRouter already filtered the
        // batch by the time it reaches the sink; NS dedupes server-side.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/entries"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;

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
        assert_eq!(body.as_array().unwrap().len(), 2, "no readings filtered");
    }
}
