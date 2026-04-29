//! `LluSource` — composes `LluAuthClient` calls into the `Source` trait.
//!
//! Token-cache + 401-retry policy:
//! - Tokens live in `Arc<tokio::sync::Mutex<Option<LluTokens>>>`.
//! - The mutex is held for the duration of an (optional) login so
//!   concurrent fetchers share a single re-login round-trip; it is
//!   released before any data HTTP call so subsequent fetchers can
//!   proceed in parallel.
//! - Each data call (connections, graph) is retried at most once on a
//!   401: cached tokens are dropped, a fresh login is performed, and
//!   the data call is reissued. A second 401 propagates as
//!   `CoreError::Source`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use cgm_bridge_core::{CoreError, PatientId, Reading, Source, SourceId};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::auth::{LluAuthClient, LluCredentials, LluTokens};
use super::error::LluError;
use super::mapping::reading_from_measurement;
use super::wire::Connection;

/// How the source picks a connection from the `/llu/connections` list.
/// `First` matches single-patient accounts; `ByPatientId` is used when an
/// account has multiple patients linked.
#[derive(Debug, Clone)]
pub enum ConnectionSelection {
    First,
    ByPatientId(PatientId),
}

impl ConnectionSelection {
    fn describe(&self) -> String {
        match self {
            ConnectionSelection::First => "first".to_string(),
            ConnectionSelection::ByPatientId(id) => format!("patient_id={}", id.as_str()),
        }
    }
}

/// Default tolerance for treating an `LluTokens` as expired before its
/// stated `expires_at`. Trades one extra login per ~hour against the risk
/// of a wasted poll on the boundary.
pub const DEFAULT_EXPIRY_SKEW_SECS: u64 = 60;

pub struct LluSource {
    id: SourceId,
    client: LluAuthClient,
    creds: LluCredentials,
    selection: ConnectionSelection,
    tokens: Arc<Mutex<Option<LluTokens>>>,
    expiry_skew: Duration,
}

impl LluSource {
    pub fn new(
        id: SourceId,
        client: LluAuthClient,
        creds: LluCredentials,
        selection: ConnectionSelection,
    ) -> Self {
        Self {
            id,
            client,
            creds,
            selection,
            tokens: Arc::new(Mutex::new(None)),
            expiry_skew: Duration::from_secs(DEFAULT_EXPIRY_SKEW_SECS),
        }
    }

    pub fn with_expiry_skew(mut self, skew: Duration) -> Self {
        self.expiry_skew = skew;
        self
    }

    /// Returns valid tokens, logging in if none are cached or the cached
    /// pair is past its skew-adjusted expiry. The mutex is held for the
    /// duration of the login so concurrent callers share the round-trip.
    async fn ensure_tokens(&self) -> Result<LluTokens, LluError> {
        let mut guard = self.tokens.lock().await;
        let now = SystemTime::now();
        if let Some(t) = guard.as_ref() {
            if !t.is_expired(now, self.expiry_skew) {
                return Ok(t.clone());
            }
            debug!("llu cached tokens expired, re-logging in");
        }
        let fresh = self.client.login(&self.creds).await?;
        info!(
            account_id_prefix = &fresh.account_id_hash[..8.min(fresh.account_id_hash.len())],
            "llu login succeeded"
        );
        let cloned = fresh.clone();
        *guard = Some(fresh);
        Ok(cloned)
    }

    /// Drop the cached tokens so the next `ensure_tokens` re-logs in.
    async fn invalidate_tokens(&self) {
        let mut guard = self.tokens.lock().await;
        *guard = None;
    }

    fn select_connection<'a>(
        &self,
        connections: &'a [Connection],
    ) -> Result<&'a Connection, LluError> {
        let chosen = match &self.selection {
            ConnectionSelection::First => connections.first(),
            ConnectionSelection::ByPatientId(id) => {
                connections.iter().find(|c| c.patient_id == id.as_str())
            }
        };
        chosen.ok_or_else(|| LluError::NoConnection {
            selection: self.selection.describe(),
        })
    }

    async fn fetch_connections(&self) -> Result<Vec<Connection>, LluError> {
        let tokens = self.ensure_tokens().await?;
        match self.client.connections(&tokens, self.creds.region).await {
            Ok(v) => Ok(v),
            Err(LluError::Unauthorized { .. }) => {
                warn!("llu connections rejected token, re-logging in");
                self.invalidate_tokens().await;
                let tokens = self.ensure_tokens().await?;
                self.client.connections(&tokens, self.creds.region).await
            }
            Err(e) => Err(e),
        }
    }

    async fn fetch_graph(
        &self,
        patient_id: &PatientId,
    ) -> Result<crate::sources::llu::wire::GraphResponse, LluError> {
        let tokens = self.ensure_tokens().await?;
        match self
            .client
            .graph(&tokens, self.creds.region, patient_id)
            .await
        {
            Ok(v) => Ok(v),
            Err(LluError::Unauthorized { .. }) => {
                warn!("llu graph rejected token, re-logging in");
                self.invalidate_tokens().await;
                let tokens = self.ensure_tokens().await?;
                self.client
                    .graph(&tokens, self.creds.region, patient_id)
                    .await
            }
            Err(e) => Err(e),
        }
    }
}

#[async_trait]
impl Source for LluSource {
    fn id(&self) -> &SourceId {
        &self.id
    }

    async fn fetch_latest(&self) -> Result<Vec<Reading>, CoreError> {
        let connections = self.fetch_connections().await.map_err(into_core)?;
        let connection = self.select_connection(&connections).map_err(into_core)?;
        let patient_id =
            PatientId::new(connection.patient_id.clone()).map_err(|e| CoreError::Source {
                message: format!("[LLU004] invalid patient id: {e}"),
            })?;

        let graph = self.fetch_graph(&patient_id).await.map_err(into_core)?;

        let mut readings: Vec<Reading> = graph
            .data
            .graph_data
            .iter()
            .map(|m| reading_from_measurement(m, &patient_id, &self.id))
            .collect::<Result<_, _>>()
            .map_err(into_core)?;
        // graph_data is typically chronological already, but LLU is
        // unofficial — sort defensively so downstream sinks can rely on
        // monotonic timestamps.
        readings.sort_by_key(|r| r.timestamp);
        Ok(readings)
    }
}

fn into_core(e: LluError) -> CoreError {
    CoreError::Source {
        message: format!("[{}] {}", e.error_code(), e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::llu::Region;
    use secrecy::SecretString;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn creds() -> LluCredentials {
        LluCredentials {
            email: "patient@example.com".to_string(),
            password: SecretString::from("hunter2"),
            region: Region::Eu,
        }
    }

    fn login_body() -> serde_json::Value {
        // expires far in the future so tokens stay valid across test sub-calls.
        serde_json::json!({
            "status": 0,
            "data": {
                "authTicket": {
                    "token": "tok",
                    "expires": 9_999_999_999u64,
                    "duration": 3600u64
                },
                "user": { "id": "user-1" }
            }
        })
    }

    fn connections_body() -> serde_json::Value {
        serde_json::json!({
            "status": 0,
            "data": [
                {
                    "id": "conn-1",
                    "patientId": "patient-1",
                    "glucoseMeasurement": {
                        "Timestamp": "3/26/2024 4:38:38 PM",
                        "ValueInMgPerDl": 142.0,
                        "TrendArrow": 3
                    }
                }
            ]
        })
    }

    fn graph_body() -> serde_json::Value {
        // Two readings, returned out of order to exercise the defensive
        // sort.
        serde_json::json!({
            "status": 0,
            "data": {
                "connection": { "id": "conn-1", "patientId": "patient-1" },
                "activeSensors": [],
                "graphData": [
                    {
                        "Timestamp": "3/26/2024 4:38:38 PM",
                        "ValueInMgPerDl": 142.0,
                        "TrendArrow": 3
                    },
                    {
                        "Timestamp": "3/26/2024 4:33:38 PM",
                        "ValueInMgPerDl": 138.0,
                        "TrendArrow": 3
                    }
                ]
            }
        })
    }

    async fn mount_happy_path(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/llu/auth/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(login_body()))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/llu/connections"))
            .respond_with(ResponseTemplate::new(200).set_body_json(connections_body()))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/llu/connections/patient-1/graph"))
            .respond_with(ResponseTemplate::new(200).set_body_json(graph_body()))
            .mount(server)
            .await;
    }

    fn build_source(server: &MockServer) -> LluSource {
        let client = LluAuthClient::new()
            .expect("client")
            .with_base_url(server.uri());
        LluSource::new(
            SourceId::new("llu").unwrap(),
            client,
            creds(),
            ConnectionSelection::First,
        )
    }

    #[tokio::test]
    async fn fetch_latest_returns_readings_oldest_first() {
        let server = MockServer::start().await;
        mount_happy_path(&server).await;

        let source = build_source(&server);
        let readings = source.fetch_latest().await.expect("fetch");
        assert_eq!(readings.len(), 2);
        assert!(readings[0].timestamp < readings[1].timestamp);
        assert_eq!(readings[1].glucose.get(), 142.0);
        assert_eq!(readings[1].patient_id.as_str(), "patient-1");
    }

    #[tokio::test]
    async fn cached_tokens_reused_across_calls() {
        let server = MockServer::start().await;
        mount_happy_path(&server).await;

        let source = build_source(&server);
        source.fetch_latest().await.expect("first fetch");
        source.fetch_latest().await.expect("second fetch");

        // Exactly one /auth/login hit means the cache held.
        let logins = server
            .received_requests()
            .await
            .expect("requests")
            .into_iter()
            .filter(|r| r.url.path() == "/llu/auth/login")
            .count();
        assert_eq!(logins, 1, "expected exactly one login");
    }

    #[tokio::test]
    async fn re_logs_in_after_401_on_connections() {
        let server = MockServer::start().await;
        // /auth/login is hit twice (initial + post-401).
        Mock::given(method("POST"))
            .and(path("/llu/auth/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(login_body()))
            .mount(&server)
            .await;
        // First connections call → 401, then 200.
        Mock::given(method("GET"))
            .and(path("/llu/connections"))
            .respond_with(ResponseTemplate::new(401))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/llu/connections"))
            .respond_with(ResponseTemplate::new(200).set_body_json(connections_body()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/llu/connections/patient-1/graph"))
            .respond_with(ResponseTemplate::new(200).set_body_json(graph_body()))
            .mount(&server)
            .await;

        let source = build_source(&server);
        let readings = source.fetch_latest().await.expect("fetch with retry");
        assert_eq!(readings.len(), 2);

        let logins = server
            .received_requests()
            .await
            .expect("requests")
            .into_iter()
            .filter(|r| r.url.path() == "/llu/auth/login")
            .count();
        assert_eq!(logins, 2, "expected re-login after 401");
    }

    #[tokio::test]
    async fn empty_connections_yields_no_connection_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/llu/auth/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(login_body()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/llu/connections"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": 0,
                "data": []
            })))
            .mount(&server)
            .await;

        let source = build_source(&server);
        let err = source.fetch_latest().await.unwrap_err();
        let CoreError::Source { message } = err else {
            panic!("expected Source error");
        };
        assert!(message.contains("LLU009"), "got: {message}");
        assert!(message.contains("first"));
    }

    #[tokio::test]
    async fn by_patient_id_picks_correct_connection() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/llu/auth/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(login_body()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/llu/connections"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": 0,
                "data": [
                    { "id": "c-a", "patientId": "patient-a" },
                    { "id": "c-b", "patientId": "patient-b" }
                ]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/llu/connections/patient-b/graph"))
            .respond_with(ResponseTemplate::new(200).set_body_json(graph_body()))
            .mount(&server)
            .await;

        let client = LluAuthClient::new()
            .expect("client")
            .with_base_url(server.uri());
        let source = LluSource::new(
            SourceId::new("llu").unwrap(),
            client,
            creds(),
            ConnectionSelection::ByPatientId(PatientId::new("patient-b").unwrap()),
        );
        let readings = source.fetch_latest().await.expect("fetch");
        assert_eq!(readings.len(), 2);
    }
}
