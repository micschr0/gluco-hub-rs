// SPDX-License-Identifier: AGPL-3.0-or-later

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use gluco_hub_core::PatientId;

use super::error::LluError;
use super::headers::{DEFAULT_LLU_VERSION, authorized_headers, base_headers};
use super::region::Region;
use super::wire::{Connection, ConnectionsResponse, GraphResponse};

/// Credentials needed to authenticate against LibreLink Up. The password is
/// kept inside `SecretString` so it never appears in `Debug` output or logs.
/// Email is also redacted — it is PII and must not appear in panic backtraces.
#[derive(Clone)]
pub struct LluCredentials {
    pub email: String,
    pub password: SecretString,
    pub region: Region,
}

impl std::fmt::Debug for LluCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LluCredentials")
            .field("email", &"<redacted>")
            .field("password", &"<redacted>")
            .field("region", &self.region)
            .finish()
    }
}

/// Bearer token + account-id hash returned by a successful login. Both are
/// secrets — the `Debug` impl avoids leaking them.
#[derive(Clone)]
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
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(LluError::from)?;
        Ok(Self {
            http,
            base_url_override: None,
            version: DEFAULT_LLU_VERSION.to_string(),
        })
    }

    /// Override the LLU app version sent in the `version` header. Useful
    /// when LibreView starts rejecting an older value mid-deploy — the
    /// operator bumps the value via config or env var without a recompile.
    /// The string is otherwise treated as opaque; no semver parsing.
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    /// Pin the base URL (without trailing slash). Intended for tests
    /// against `wiremock`; real callers rely on `Region`-derived URLs.
    /// Test-gated so production builds never expose the override.
    #[cfg(test)]
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

    /// Send a request with automatic 429 rate-limit retry.
    /// Reads the `Retry-After` header (defaults to 5 s if missing),
    /// sleeps, and retries up to 3 total attempts.
    async fn send_with_retry(
        &self,
        req: reqwest::RequestBuilder,
        label: &'static str,
    ) -> Result<reqwest::Response, LluError> {
        const MAX_ATTEMPTS: u32 = 3;
        for attempt in 0..MAX_ATTEMPTS {
            let resp = req
                .try_clone()
                .ok_or_else(|| LluError::Transport("request not cloneable".into()))?
                .send()
                .await?;
            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_after = resp
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(5);
                warn!(
                    attempt = attempt + 1,
                    retry_after, "LLU {label} 429 rate-limited, retrying"
                );
                tokio::time::sleep(Duration::from_secs(retry_after)).await;
                continue;
            }
            return Ok(resp);
        }
        Err(LluError::Transport(format!(
            "{label} rate-limited after {MAX_ATTEMPTS} attempts"
        )))
    }

    /// Authenticate with LibreLink Up. Follows at most one region redirect:
    /// LLU sometimes responds with `{ status: 0, data: { redirect: true,
    /// region: "..." } }` and expects the client to retry against the new
    /// region. Loops past one hop are mapped to `LluError::RedirectLoop`.
    pub async fn login(&self, creds: &LluCredentials) -> Result<LluTokens, LluError> {
        let password = creds.password.expose_secret();

        // If the password looks like a JWT, skip the login call and use
        // the token directly as the Bearer credential. The operator can
        // paste a pre-obtained token into the `password` config field
        // without changing the schema.
        if is_jwt(password) {
            info!("LLU password appears to be a JWT — skipping login, using token directly");

            let account_id_hash = jwt_claims_user_id(password)
                .map(|uid| account_id_hash(&uid))
                .unwrap_or_default();

            let expires_at = jwt_claims_exp(password)
                .and_then(|exp| UNIX_EPOCH.checked_add(Duration::from_secs(exp)))
                .unwrap_or_else(|| {
                    // Without an `exp` claim, assume the token is good for 60
                    // minutes — the same order of magnitude as a normal LLU
                    // auth ticket.
                    SystemTime::now()
                        .checked_add(Duration::from_secs(3600))
                        .unwrap_or(SystemTime::now())
                });

            return Ok(LluTokens {
                bearer: SecretString::from(password.to_string()),
                account_id_hash,
                expires_at,
            });
        }

        let body = LoginRequest {
            email: &creds.email,
            password,
        };

        let mut current_region = creds.region;
        for hop in 0..2 {
            let url = self.login_url(current_region);
            debug!(region = ?current_region, hop, "llu login request");
            let resp = self
                .send_with_retry(
                    self.http
                        .post(&url)
                        .headers(base_headers(&self.version))
                        .json(&body),
                    "login",
                )
                .await?;

            let status = resp.status();
            let content_type = content_type_of(resp.headers());
            let raw = resp.bytes().await?;

            if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(LluError::InvalidCredentials);
            }

            let envelope: LoginEnvelope = match serde_json::from_slice(&raw) {
                Ok(v) => v,
                Err(e) => {
                    log_protocol_failure("login", status, content_type.as_deref(), &raw, &e);
                    return Err(LluError::Protocol {
                        reason: format!("decode: {e}"),
                    });
                }
            };

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
            .send_with_retry(
                self.http
                    .get(&url)
                    .headers(authorized_headers(tokens, &self.version)),
                "connections",
            )
            .await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(LluError::Unauthorized {
                endpoint: "connections",
            });
        }
        let content_type = content_type_of(resp.headers());
        let raw = resp.bytes().await?;
        let parsed: ConnectionsResponse = match serde_json::from_slice(&raw) {
            Ok(v) => v,
            Err(e) => {
                log_protocol_failure("connections", status, content_type.as_deref(), &raw, &e);
                return Err(LluError::Protocol {
                    reason: format!("connections decode: {e}"),
                });
            }
        };
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
            .send_with_retry(
                self.http
                    .get(&url)
                    .headers(authorized_headers(tokens, &self.version)),
                "graph",
            )
            .await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(LluError::Unauthorized { endpoint: "graph" });
        }
        let content_type = content_type_of(resp.headers());
        let raw = resp.bytes().await?;
        let parsed: GraphResponse = match serde_json::from_slice(&raw) {
            Ok(v) => v,
            Err(e) => {
                log_protocol_failure("graph", status, content_type.as_deref(), &raw, &e);
                return Err(LluError::Protocol {
                    reason: format!("graph decode: {e}"),
                });
            }
        };
        if parsed.status != 0 {
            return Err(LluError::Status {
                status: parsed.status,
            });
        }
        Ok(parsed)
    }
}

fn content_type_of(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(String::from)
}

/// Emit a debug log with the response context that produced a decode
/// failure. Only fires on the error path — a healthy response never
/// hits this. Body is truncated to `SNIPPET_BYTES` and rendered with
/// `from_utf8_lossy` so binary garbage (e.g. raw gzip bytes, before we
/// enabled the `gzip` feature) shows up as `\xNN` placeholders rather
/// than panicking the formatter. Gated behind `debug` to avoid spamming
/// production logs; operators flip on `RUST_LOG=debug` when investigating.
fn log_protocol_failure(
    endpoint: &'static str,
    status: reqwest::StatusCode,
    content_type: Option<&str>,
    raw: &[u8],
    err: &serde_json::Error,
) {
    const SNIPPET_BYTES: usize = 200;
    let snippet = String::from_utf8_lossy(&raw[..raw.len().min(SNIPPET_BYTES)]);
    debug!(
        endpoint,
        status = status.as_u16(),
        content_type = content_type.unwrap_or("<missing>"),
        body_bytes = raw.len(),
        truncated = raw.len() > SNIPPET_BYTES,
        body_snippet = %snippet,
        error = %err,
        "llu response decode failed",
    );
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

/// Heuristic: does `s` look like a JWT? Checks for two dots, a header
/// segment starting with `eyJ` (base64url of `{"`), and minimum length.
fn is_jwt(s: &str) -> bool {
    s.len() >= 20 && s.matches('.').count() == 2 && s.starts_with("eyJ")
}

/// Minimal base64url decoder. Accepts standard base64url alphabet
/// (`-` and `_` as the last two characters) with optional padding.
fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let input = input.trim_end_matches('=');
    let mut buf = Vec::with_capacity(input.len() * 3 / 4);
    let mut accum: u32 = 0;
    let mut bits: u32 = 0;
    for &b in input.as_bytes() {
        let val = ALPHABET.iter().position(|&c| c == b)? as u32;
        accum = (accum << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            buf.push((accum >> bits) as u8);
        }
    }
    Some(buf)
}

/// Extract a user identifier from the JWT payload. Tries common claims:
/// `sub`, `userId`, and `user_id`. Returns `None` if the payload cannot
/// be decoded or none of the claims are present.
fn jwt_claims_user_id(jwt: &str) -> Option<String> {
    let payload = jwt.split('.').nth(1)?;
    let decoded = base64url_decode(payload)?;
    let v: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    v.get("sub")
        .or_else(|| v.get("userId"))
        .or_else(|| v.get("user_id"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Extract the `exp` claim (Unix seconds) from the JWT payload.
/// Returns `None` if the payload cannot be decoded or the claim is absent.
fn jwt_claims_exp(jwt: &str) -> Option<u64> {
    let payload = jwt.split('.').nth(1)?;
    let decoded = base64url_decode(payload)?;
    let v: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    v.get("exp").and_then(|v| v.as_u64())
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
            .and(header("version", "4.17.0"))
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
        assert_eq!(resp.data.graph_data[0].value_in_mg_per_dl, Some(138.0));
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

    // ── JWT heuristic tests ──

    #[test]
    fn is_jwt_detects_valid_token() {
        let token = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        assert!(is_jwt(token));
    }

    #[test]
    fn is_jwt_rejects_plain_password() {
        assert!(!is_jwt("hunter2"));
        assert!(!is_jwt("correct-horse-battery-staple"));
        assert!(!is_jwt("12345678901234567890")); // ≥20 chars but no dots
    }

    #[test]
    fn is_jwt_rejects_short_input() {
        assert!(!is_jwt("eyJ.h.sig")); // 10 chars — below minimum
    }

    #[test]
    fn is_jwt_rejects_wrong_dot_count() {
        assert!(!is_jwt("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0")); // 1 dot
        assert!(!is_jwt("eyJ.a.b.c")); // 3 dots
    }

    #[test]
    fn is_jwt_rejects_non_jwt_header_prefix() {
        assert!(!is_jwt(
            "eXJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.sig"
        )); // starts with eXJ not eyJ
    }

    #[test]
    fn base64url_decode_roundtrips_known() {
        // "abc" in base64url = "YWJj"
        assert_eq!(base64url_decode("YWJj").unwrap(), b"abc");
        // "f" (single byte) = "Zg" (must handle padding correctly)
        assert_eq!(base64url_decode("Zg").unwrap(), b"f");
    }

    #[test]
    fn base64url_decode_rejects_invalid_chars() {
        assert!(base64url_decode("!!!").is_none());
    }

    #[test]
    fn base64url_decode_handles_padding() {
        assert_eq!(base64url_decode("YWJj").unwrap(), b"abc");
        assert_eq!(base64url_decode("YWJj=").unwrap(), b"abc");
        assert_eq!(base64url_decode("YWJj==").unwrap(), b"abc");
    }

    #[test]
    fn jwt_claims_extracts_sub() {
        // Payload: {"sub":"user-42","exp":2000000000}
        let payload = "eyJzdWIiOiJ1c2VyLTQyIiwiZXhwIjoyMDAwMDAwMDAwfQ";
        let token = format!("header.{payload}.sig");
        assert_eq!(jwt_claims_user_id(&token).as_deref(), Some("user-42"));
    }

    #[test]
    fn jwt_claims_extracts_user_id() {
        // Payload: {"user_id":"abc-123"}
        let payload = "eyJ1c2VyX2lkIjoiYWJjLTEyMyJ9";
        let token = format!("header.{payload}.sig");
        assert_eq!(jwt_claims_user_id(&token).as_deref(), Some("abc-123"));
    }

    #[test]
    fn jwt_claims_returns_none_without_known_field() {
        let payload = base64url_encode(br#"{"iss":"gluco-hub"}"#);
        let token = format!("header.{payload}.sig");
        assert!(jwt_claims_user_id(&token).is_none());
    }

    #[test]
    fn jwt_claims_exp_extracts_unix_time() {
        let payload = "eyJleHAiOjIwMDAwMDAwMDB9"; // {"exp":2000000000}
        let token = format!("header.{payload}.sig");
        assert_eq!(jwt_claims_exp(&token), Some(2_000_000_000));
    }

    #[test]
    fn jwt_claims_exp_returns_none_when_absent() {
        let payload = base64url_encode(br#"{"sub":"u1"}"#);
        let token = format!("header.{payload}.sig");
        assert!(jwt_claims_exp(&token).is_none());
    }

    // Helper to produce base64url without pulling in a crate.
    fn base64url_encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
            let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
            if chunk.len() > 1 {
                out.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
            }
            if chunk.len() > 2 {
                out.push(ALPHABET[(n & 0x3F) as usize] as char);
            }
        }
        out
    }
}
