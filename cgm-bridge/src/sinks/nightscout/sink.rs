//! `NightscoutSink` — implements `Sink` over `NightscoutClient`.
//!
//! The trait error type is `CoreError`, but we want the inner LLU/NS
//! `error_code` ("NS001"…"NS005") to survive into metrics labels. We
//! embed the code as a `[NSxxx]` prefix in `CoreError::Sink::message`
//! so the poll-loop fan-out can extract it without coupling to NS
//! types.

use async_trait::async_trait;
use cgm_bridge_core::{CoreError, Reading, Sink};

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
        self.client
            .post_entries(readings)
            .await
            .map_err(|e| CoreError::Sink {
                message: format!("[{}] {}", e.error_code(), e),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cgm_bridge_core::{GlucoseMgDl, PatientId, SourceId, Trend};
    use chrono::{TimeZone, Utc};
    use secrecy::SecretString;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn one_reading() -> Reading {
        Reading {
            patient_id: PatientId::new("p1").unwrap(),
            source_id: SourceId::new("llu").unwrap(),
            timestamp: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            glucose: GlucoseMgDl::new(120.0).unwrap(),
            trend: Trend::Flat,
        }
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
            .and(path("/api/v3/entries"))
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
            .and(path("/api/v3/entries"))
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
            .and(path("/api/v3/entries"))
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
        // No mock configured: an unexpected HTTP call would fail the
        // wiremock server's strict mode (we just don't mount any).
        let server = MockServer::start().await;
        build_sink(&server).push(&[]).await.expect("noop");
    }
}
