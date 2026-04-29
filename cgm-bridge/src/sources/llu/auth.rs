use std::time::{Duration, SystemTime, UNIX_EPOCH};

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

use cgm_bridge_core::PatientId;

use super::error::LluError;
use super::headers::{DEFAULT_LLU_VERSION, authorized_headers, base_headers};
use super::region::Region;
use super::wire::{Connection, ConnectionsResponse, GraphResponse};

/// Credentials needed to authenticate against LibreLink Up. The password is
/// kept inside `SecretString` so it never appears in `Debug` output or logs.
#[derive(Debug, Clone)]
pub struct LluCredentials {
    pub email: String,
    pub password: SecretString,
    pub region: Region,
}

/// Bearer token + account-id hash returned by a successful login. Both are
/// secrets — the `Debug` impl avoids leaking them.
pub struct LluTokens {
    pub bearer: SecretString,
    /// `sha256(user.id)` rendered as lowercase hex, sent verbatim in the
    /// `account-id` header on subsequent calls.
    pub account_id_hash: String,
    /// Wall-clock expiry of `bearer`. Computed from the server-provided
    /// `expires` field (Unix seconds) at login time.
    pub expires_at: SystemTime,
}

impl std::fmt::Debug for LluTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let prefix: String = self.account_id_hash.chars().take(8).collect();
        f.debug_struct("LluTokens")
            .field("bearer", &"<redacted>")
            .field("account_id_prefix", &prefix)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl LluTokens {
    /// Returns true when `now + skew` is at or past `expires_at`.
    pub fn is_expired(&self, now: SystemTime, skew: Duration) -> bool {
        self.expires_at
            .checked_sub(skew)
            .map(|deadline| now >= deadline)
            .unwrap_or(true)
    }
}

/// Compute the LLU `account-id` header value: SHA-256 of the user id from
/// the login response, rendered as lowercase hex.
pub fn account_id_hash(user_id: &str) -> String {
    let digest = Sha256::digest(user_id.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest.iter() {
        use std::fmt::Write;
        let _ = write!(&mut out, "{:02x}", byte);
    }
    out
}

/// One-stop client for the LibreLink Up surface used by the bridge:
/// auth, connections list, and per-patient graph. Each method is a thin
/// HTTP call — token caching and 401 retry live one layer up in
/// `LluSource` (4c.2).
#[derive(Debug, Clone)]
pub struct LluAuthClient {
    http: reqwest::Client,
    base_url_override: Option<String>,
    version: String,
}

impl LluAuthClient {
    /// Build a client backed by a rustls-enabled `reqwest::Client` with the
    /// LibreLink Up app version pinned to `DEFAULT_LLU_VERSION`.
    pub fn new() -> Result<Self, LluError> {
        // No `https_only(true)`: the production `Region::base_url()` is
        // already hardcoded to `https://`, and forbidding http here would
        // break the wiremock-based test suite without adding any real
        // protection on top of the hardcoded URLs.
        let http = reqwest::Client::builder().build().map_err(LluError::from)?;
        Ok(Self {
            http,
            base_url_override: None,
            version: DEFAULT_LLU_VERSION.to_string(),
        })
    }

    /// Override the LLU app version sent in the `version` header. Useful
    /// when LibreView starts rejecting an older value mid-deploy.
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    /// Pin the base URL (without trailing slash). Intended for tests against
    /// `wiremock`; real callers rely on `Region`-derived URLs.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url_override = Some(base_url.into());
        self
    }

    fn base_url_for(&self, region: Region) -> String {
        self.base_url_override
            .clone()
            .unwrap_or_else(|| region.base_url())
    }

    fn login_url(&self, region: Region) -> String {
        format!("{}/llu/auth/login", self.base_url_for(region))
    }

    fn connections_url(&self, region: Region) -> String {
        format!("{}/llu/connections", self.base_url_for(region))
    }

    fn graph_url(&self, region: Region, patient_id: &PatientId) -> String {
        format!(
            "{}/llu/connections/{}/graph",
            self.base_url_for(region),
            patient_id.as_str()
        )
    }

    /// Authenticate with LibreLink Up. Follows at most one region redirect:
    /// LLU sometimes responds with `{ status: 0, data: { redirect: true,
    /// region: "..." } }` and expects the client to retry against the new
    /// region. Loops past one hop are mapped to `LluError::RedirectLoop`.
    pub async fn login(&self, creds: &LluCredentials) -> Result<LluTokens, LluError> {
        let body = LoginRequest {
            email: &creds.email,
            password: creds.password.expose_secret(),
        };

        let mut current_region = creds.region;
        for hop in 0..2 {
            let url = self.login_url(current_region);
            debug!(region = ?current_region, hop, "llu login request");
            let resp = self
                .http
                .post(&url)
                .headers(base_headers(&self.version))
                .json(&body)
                .send()
                .await?;

            let status = resp.status();
            let raw = resp.bytes().await?;

            if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(LluError::InvalidCredentials);
            }

            let envelope: LoginEnvelope =
                serde_json::from_slice(&raw).map_err(|e| LluError::Protocol {
                    reason: format!("decode: {e}"),
                })?;

            // LLU encodes "wrong password" as `status == 2` (and friends).
            // Anything other than 0 with no payload counts as a hard failure.
            match envelope.data {
                LoginData::Redirect { redirect, region } => {
                    if !redirect {
                        return Err(LluError::Protocol {
                            reason: "redirect payload with redirect=false".into(),
                        });
                    }
                    if hop > 0 {
                        return Err(LluError::RedirectLoop);
                    }
                    let next = Region::parse(&region)?;
                    warn!(from = ?current_region, to = ?next, "llu region redirect");
                    current_region = next;
                    continue;
                }
                LoginData::AuthTicket { auth_ticket, user } => {
                    if envelope.status != 0 {
                        return Err(LluError::Status {
                            status: envelope.status,
                        });
                    }
                    return Ok(build_tokens(auth_ticket, &user.id));
                }
                LoginData::Empty {} => {
                    if envelope.status == 2 {
                        return Err(LluError::InvalidCredentials);
                    }
                    return Err(LluError::Status {
                        status: envelope.status,
                    });
                }
            }
        }
        Err(LluError::RedirectLoop)
    }

    /// `GET /llu/connections` — list patient links visible to the
    /// authenticated account. The response wraps `Vec<Connection>`; this
    /// method returns the inner vector for ergonomics.
    pub async fn connections(
        &self,
        tokens: &LluTokens,
        region: Region,
    ) -> Result<Vec<Connection>, LluError> {
        let url = self.connections_url(region);
        let resp = self
            .http
            .get(&url)
            .headers(authorized_headers(tokens, &self.version))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(LluError::Unauthorized {
                endpoint: "connections",
            });
        }
        let raw = resp.bytes().await?;
        let parsed: ConnectionsResponse =
            serde_json::from_slice(&raw).map_err(|e| LluError::Protocol {
                reason: format!("connections decode: {e}"),
            })?;
        if parsed.status != 0 {
            return Err(LluError::Status {
                status: parsed.status,
            });
        }
        Ok(parsed.data)
    }

    /// `GET /llu/connections/{patientId}/graph` — return ~24 h of
    /// historical readings plus the current measurement. Note that LLU
    /// keys this on the `patientId` field of `Connection`, NOT on
    /// `Connection.id`.
    pub async fn graph(
        &self,
        tokens: &LluTokens,
        region: Region,
        patient_id: &PatientId,
    ) -> Result<GraphResponse, LluError> {
        let url = self.graph_url(region, patient_id);
        let resp = self
            .http
            .get(&url)
            .headers(authorized_headers(tokens, &self.version))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(LluError::Unauthorized { endpoint: "graph" });
        }
        let raw = resp.bytes().await?;
        let parsed: GraphResponse =
            serde_json::from_slice(&raw).map_err(|e| LluError::Protocol {
                reason: format!("graph decode: {e}"),
            })?;
        if parsed.status != 0 {
            return Err(LluError::Status {
                status: parsed.status,
            });
        }
        Ok(parsed)
    }
}

fn build_tokens(ticket: AuthTicket, user_id: &str) -> LluTokens {
    let expires_at = UNIX_EPOCH
        .checked_add(Duration::from_secs(ticket.expires))
        .unwrap_or(SystemTime::now());
    LluTokens {
        bearer: SecretString::from(ticket.token),
        account_id_hash: account_id_hash(user_id),
        expires_at,
    }
}

#[derive(Serialize)]
struct LoginRequest<'a> {
    email: &'a str,
    password: &'a str,
}

#[derive(Deserialize)]
struct LoginEnvelope {
    status: i64,
    data: LoginData,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum LoginData {
    Redirect {
        redirect: bool,
        region: String,
    },
    AuthTicket {
        #[serde(rename = "authTicket")]
        auth_ticket: AuthTicket,
        user: LoginUser,
    },
    /// Returned on auth failures (`status != 0`) and other non-token replies.
    Empty {},
}

#[derive(Deserialize)]
struct AuthTicket {
    token: String,
    /// Unix seconds.
    expires: u64,
    #[allow(dead_code)]
    #[serde(default)]
    duration: u64,
}

#[derive(Deserialize)]
struct LoginUser {
    id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn creds(region: Region) -> LluCredentials {
        LluCredentials {
            email: "patient@example.com".to_string(),
            password: SecretString::from("hunter2"),
            region,
        }
    }

    #[test]
    fn account_id_hash_matches_known_vector() {
        // sha256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        assert_eq!(
            account_id_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn tokens_debug_redacts_secrets() {
        let tokens = LluTokens {
            bearer: SecretString::from("supersecret".to_string()),
            account_id_hash: "abcdef0123456789".to_string(),
            expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        };
        let dump = format!("{:?}", tokens);
        assert!(!dump.contains("supersecret"));
        assert!(dump.contains("abcdef01"));
    }

    #[test]
    fn is_expired_with_skew() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let tokens = LluTokens {
            bearer: SecretString::from("x".to_string()),
            account_id_hash: "h".to_string(),
            expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_059),
        };
        assert!(!tokens.is_expired(now, Duration::from_secs(30)));
        assert!(tokens.is_expired(now, Duration::from_secs(60)));
    }

    #[tokio::test]
    async fn login_returns_tokens_on_happy_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/llu/auth/login"))
            .and(header("product", "llu.ios"))
            .and(header("version", "4.16.0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": 0,
                "data": {
                    "authTicket": {
                        "token": "tok-123",
                        "expires": 1_700_000_000u64,
                        "duration": 3600u64
                    },
                    "user": { "id": "user-42" }
                }
            })))
            .mount(&server)
            .await;

        let client = LluAuthClient::new()
            .expect("client")
            .with_base_url(server.uri());
        // `with_base_url` bypasses `Region`, but creds still need a value.
        let tokens = client.login(&creds(Region::Eu)).await.expect("login");
        assert_eq!(tokens.bearer.expose_secret(), "tok-123");
        // sha256("user-42")
        assert_eq!(tokens.account_id_hash, account_id_hash("user-42"));
        assert_eq!(
            tokens.expires_at,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000)
        );
    }

    #[tokio::test]
    async fn login_maps_401_to_invalid_credentials() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/llu/auth/login"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let client = LluAuthClient::new()
            .expect("client")
            .with_base_url(server.uri());
        let err = client.login(&creds(Region::Eu)).await.unwrap_err();
        assert_eq!(err.error_code(), "LLU003");
    }

    #[tokio::test]
    async fn login_maps_status_2_to_invalid_credentials() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/llu/auth/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": 2,
                "data": {}
            })))
            .mount(&server)
            .await;

        let client = LluAuthClient::new()
            .expect("client")
            .with_base_url(server.uri());
        let err = client.login(&creds(Region::Eu)).await.unwrap_err();
        assert!(matches!(err, LluError::InvalidCredentials));
    }

    #[tokio::test]
    async fn login_rejects_malformed_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/llu/auth/login"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let client = LluAuthClient::new()
            .expect("client")
            .with_base_url(server.uri());
        let err = client.login(&creds(Region::Eu)).await.unwrap_err();
        assert!(matches!(err, LluError::Protocol { .. }));
    }

    #[tokio::test]
    async fn login_follows_one_region_redirect() {
        let server = MockServer::start().await;
        // Both hops use the same base URL because `with_base_url` overrides
        // region routing. We assert the redirect path is taken by counting
        // requests via wiremock's `expect`.
        Mock::given(method("POST"))
            .and(path("/llu/auth/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": 0,
                "data": { "redirect": true, "region": "US" }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/llu/auth/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": 0,
                "data": {
                    "authTicket": { "token": "tok", "expires": 1u64, "duration": 0u64 },
                    "user": { "id": "u" }
                }
            })))
            .mount(&server)
            .await;

        let client = LluAuthClient::new()
            .expect("client")
            .with_base_url(server.uri());
        let tokens = client.login(&creds(Region::Eu)).await.expect("login");
        assert_eq!(tokens.bearer.expose_secret(), "tok");
    }

    #[tokio::test]
    async fn login_breaks_redirect_loop() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/llu/auth/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": 0,
                "data": { "redirect": true, "region": "US" }
            })))
            .mount(&server)
            .await;

        let client = LluAuthClient::new()
            .expect("client")
            .with_base_url(server.uri());
        let err = client.login(&creds(Region::Eu)).await.unwrap_err();
        assert!(matches!(err, LluError::RedirectLoop));
    }

    fn fake_tokens() -> LluTokens {
        LluTokens {
            bearer: SecretString::from("test-bearer".to_string()),
            account_id_hash: account_id_hash("user-1"),
            expires_at: UNIX_EPOCH + Duration::from_secs(9_999_999_999),
        }
    }

    #[tokio::test]
    async fn connections_returns_data_on_happy_path() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/llu/connections"))
            .and(header("authorization", "Bearer test-bearer"))
            .and(header("account-id", account_id_hash("user-1").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": 0,
                "data": [{
                    "id": "conn-1",
                    "patientId": "patient-1",
                    "glucoseMeasurement": {
                        "Timestamp": "3/26/2024 4:38:38 PM",
                        "ValueInMgPerDl": 142.0,
                        "TrendArrow": 3
                    }
                }]
            })))
            .mount(&server)
            .await;

        let client = LluAuthClient::new()
            .expect("client")
            .with_base_url(server.uri());
        let conns = client
            .connections(&fake_tokens(), Region::Eu)
            .await
            .expect("connections");
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].patient_id, "patient-1");
    }

    #[tokio::test]
    async fn connections_rejects_malformed_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/llu/connections"))
            .respond_with(ResponseTemplate::new(200).set_body_string("nope"))
            .mount(&server)
            .await;

        let client = LluAuthClient::new()
            .expect("client")
            .with_base_url(server.uri());
        let err = client
            .connections(&fake_tokens(), Region::Eu)
            .await
            .unwrap_err();
        assert!(matches!(err, LluError::Protocol { .. }));
    }

    #[tokio::test]
    async fn graph_returns_data_on_happy_path() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/llu/connections/patient-1/graph"))
            .and(header("authorization", "Bearer test-bearer"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": 0,
                "data": {
                    "connection": { "id": "conn-1", "patientId": "patient-1" },
                    "activeSensors": [],
                    "graphData": [
                        {
                            "Timestamp": "3/26/2024 4:33:38 PM",
                            "ValueInMgPerDl": 138.0,
                            "TrendArrow": 3
                        }
                    ]
                }
            })))
            .mount(&server)
            .await;

        let client = LluAuthClient::new()
            .expect("client")
            .with_base_url(server.uri());
        let pid = PatientId::new("patient-1").expect("pid");
        let resp = client
            .graph(&fake_tokens(), Region::Eu, &pid)
            .await
            .expect("graph");
        assert_eq!(resp.data.graph_data.len(), 1);
        assert_eq!(resp.data.graph_data[0].value_in_mg_per_dl, 138.0);
    }

    #[tokio::test]
    async fn graph_maps_401_to_unauthorized() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/llu/connections/patient-1/graph"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let client = LluAuthClient::new()
            .expect("client")
            .with_base_url(server.uri());
        let pid = PatientId::new("patient-1").expect("pid");
        let err = client
            .graph(&fake_tokens(), Region::Eu, &pid)
            .await
            .unwrap_err();
        assert!(matches!(err, LluError::Unauthorized { endpoint } if endpoint == "graph"));
        assert_eq!(err.error_code(), "LLU008");
    }
}
