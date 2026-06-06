// SPDX-License-Identifier: AGPL-3.0-or-later

//! Nightscout HTTP client with two operator-selectable auth modes.
//!
//! Modern Nightscout (cgm-remote-monitor ≥ 14.x) rejects the legacy
//! `api-secret: <sha1_hex(secret)>` header on the **v3** API with `401`,
//! even though the v1 API still honours it. Earlier versions of this
//! client posted to `/api/v3/entries` with the SHA-1 header and therefore
//! never authenticated against a real deployment (see issue #24). The two
//! modes below each target the API version that actually accepts their
//! credential:
//!
//! - [`Auth::ApiSecret`] → `POST /api/v1/entries` with
//!   `api-secret: <sha1_hex(secret)>`. The v1 API still accepts the
//!   legacy header, so this is the zero-round-trip path for deployments
//!   that only expose an API secret.
//! - [`Auth::Token`] → `POST /api/v3/entries` with
//!   `Authorization: Bearer <jwt>`. The JWT is minted on demand from an
//!   access token via `GET /api/v2/authorization/request/<token>` and
//!   cached until a request returns `401`, at which point it is refreshed
//!   once and the request retried.
//!
//! SHA-1 here is a request-shape choice mandated by NS, not a security
//! choice — transport security comes from the rustls-protected TLS
//! connection to the NS host. The access token and minted JWT are both
//! wrapped in [`SecretString`] so neither leaks through `Debug` or logs.

use std::sync::Arc;

use gluco_hub_core::Reading;
use secrecy::{ExposeSecret, SecretString};
use sha1::{Digest, Sha1};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::debug;

use super::wire::{NsEntry, entry_from_reading};

/// Errors surfaced by the Nightscout client. Each variant carries a
/// stable `error_code` (the bracketed prefix in `Display`) so logs and
/// metrics labels stay grep-friendly.
#[derive(Debug, Error)]
pub enum NsError {
    #[error("[NS001] HTTP transport error: {0}")]
    Transport(String),

    #[error("[NS002] Nightscout rejected credentials: 401")]
    Unauthorized,

    #[error("[NS003] Nightscout returned non-success status: {status}")]
    Status { status: u16 },

    #[error("[NS004] Nightscout returned a transient error: {status}")]
    Retryable { status: u16 },

    #[error("[NS005] invalid base URL: {reason}")]
    InvalidBaseUrl { reason: String },

    #[error("[NS006] could not obtain Nightscout JWT: {reason}")]
    Authorization { reason: String },
}

impl NsError {
    /// Stable string identifier per error variant. Used by the
    /// `Display` impl above (which is what callers actually read);
    /// kept as a separate accessor so future typed retry policies
    /// can match without parsing the formatted message.
    #[allow(dead_code)]
    pub fn error_code(&self) -> &'static str {
        match self {
            NsError::Transport(_) => "NS001",
            NsError::Unauthorized => "NS002",
            NsError::Status { .. } => "NS003",
            NsError::Retryable { .. } => "NS004",
            NsError::InvalidBaseUrl { .. } => "NS005",
            NsError::Authorization { .. } => "NS006",
        }
    }
}

impl From<reqwest::Error> for NsError {
    fn from(value: reqwest::Error) -> Self {
        NsError::Transport(value.to_string())
    }
}

/// Compute the `api-secret` header value from the raw secret per the
/// Nightscout `lib/api3/security.js` convention: lowercase hex of the
/// SHA-1 digest of the secret bytes.
pub fn api_secret_header(secret: &SecretString) -> String {
    let digest = Sha1::digest(secret.expose_secret().as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest.iter() {
        use std::fmt::Write;
        let _ = write!(&mut out, "{:02x}", byte);
    }
    out
}

/// How the client authenticates, and—because the credential dictates the
/// API version that accepts it—which entries endpoint it targets.
#[derive(Debug, Clone)]
enum Auth {
    /// `api-secret: <sha1_hex>` against the v1 API.
    ApiSecret { header: String },
    /// `Authorization: Bearer <jwt>` against the v3 API. `jwt` is the
    /// lazily-minted, refresh-on-401 cache shared across clones.
    Token {
        access_token: SecretString,
        jwt: Arc<RwLock<Option<SecretString>>>,
    },
}

#[derive(Debug, Clone)]
pub struct NightscoutClient {
    base_url: String,
    http: reqwest::Client,
    device: Option<String>,
    app: Option<String>,
    auth: Auth,
}

impl NightscoutClient {
    /// Build a client that authenticates with the legacy `api-secret`
    /// SHA-1 header against the **v1** entries API.
    pub fn new(base_url: impl Into<String>, secret: SecretString) -> Result<Self, NsError> {
        Self::build(
            base_url,
            Auth::ApiSecret {
                header: api_secret_header(&secret),
            },
        )
    }

    /// Build a client that mints a JWT from `access_token` and
    /// authenticates with `Authorization: Bearer` against the **v3**
    /// entries API.
    pub fn with_access_token(
        base_url: impl Into<String>,
        access_token: SecretString,
    ) -> Result<Self, NsError> {
        Self::build(
            base_url,
            Auth::Token {
                access_token,
                jwt: Arc::new(RwLock::new(None)),
            },
        )
    }

    fn build(base_url: impl Into<String>, auth: Auth) -> Result<Self, NsError> {
        let trimmed = base_url.into().trim_end_matches('/').to_string();
        if trimmed.is_empty() {
            return Err(NsError::InvalidBaseUrl {
                reason: "base_url is empty".into(),
            });
        }
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(NsError::from)?;
        Ok(Self {
            base_url: trimmed,
            http,
            device: None,
            app: None,
            auth,
        })
    }

    /// Identify this service in the Nightscout UI's source column.
    pub fn with_device(mut self, device: impl Into<String>) -> Self {
        self.device = Some(device.into());
        self
    }

    /// App name attached to every uploaded entry.
    pub fn with_app(mut self, app: impl Into<String>) -> Self {
        self.app = Some(app.into());
        self
    }

    /// The entries collection URL for the auth mode's API version. Both
    /// the dedup `GET` and the upload `POST` hit this same path; the
    /// v1 API serves JSON for it when `Accept: application/json` is set.
    fn entries_url(&self) -> String {
        match self.auth {
            Auth::ApiSecret { .. } => format!("{}/api/v1/entries", self.base_url),
            Auth::Token { .. } => format!("{}/api/v3/entries", self.base_url),
        }
    }

    /// Send a request with the mode's credential attached. In token mode
    /// a `401` triggers exactly one JWT refresh + retry (the cached JWT
    /// has likely expired or been rotated); a second `401` propagates to
    /// the caller, which maps it to [`NsError::Unauthorized`]. In
    /// api-secret mode the response is returned verbatim.
    ///
    /// `build_req` must be callable twice (it rebuilds the full request,
    /// body included, for the retry).
    async fn send_authed<F>(&self, build_req: F) -> Result<reqwest::Response, NsError>
    where
        F: Fn() -> reqwest::RequestBuilder,
    {
        match &self.auth {
            Auth::ApiSecret { header } => {
                Ok(build_req().header("api-secret", header).send().await?)
            }
            Auth::Token { jwt, .. } => {
                // Clone out of the guard and drop it on the same line:
                // `refresh_jwt` takes the write lock, so holding the read
                // guard across that call would self-deadlock.
                let cached = jwt.read().await.clone();
                let token = match cached {
                    Some(t) => t,
                    None => self.refresh_jwt().await?,
                };
                let resp = build_req()
                    .bearer_auth(token.expose_secret())
                    .send()
                    .await?;
                if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
                    return Ok(resp);
                }
                let fresh = self.refresh_jwt().await?;
                Ok(build_req()
                    .bearer_auth(fresh.expose_secret())
                    .send()
                    .await?)
            }
        }
    }

    /// Exchange the access token for a fresh JWT via
    /// `GET /api/v2/authorization/request/<token>` and cache it. The
    /// access token sits in the URL path (NS's own convention), so this
    /// URL is never logged.
    async fn refresh_jwt(&self) -> Result<SecretString, NsError> {
        let Auth::Token { access_token, jwt } = &self.auth else {
            return Err(NsError::Authorization {
                reason: "JWT refresh requested in api-secret mode".into(),
            });
        };
        let url = format!(
            "{}/api/v2/authorization/request/{}",
            self.base_url,
            access_token.expose_secret()
        );
        let resp = self
            .http
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(NsError::Unauthorized);
        }
        if !status.is_success() {
            let code = status.as_u16();
            if code == 429 || (500..=599).contains(&code) {
                return Err(NsError::Retryable { status: code });
            }
            return Err(NsError::Authorization {
                reason: format!("authorization request returned {code}"),
            });
        }
        #[derive(serde::Deserialize)]
        struct AuthResp {
            token: Option<String>,
        }
        let parsed: AuthResp = resp.json().await.map_err(|e| NsError::Authorization {
            reason: format!("authorization response decode failed: {e}"),
        })?;
        let token =
            parsed
                .token
                .filter(|t| !t.is_empty())
                .ok_or_else(|| NsError::Authorization {
                    reason: "authorization response missing token".into(),
                })?;
        let secret = SecretString::from(token);
        *jwt.write().await = Some(secret.clone());
        debug!("ns: obtained fresh JWT from access token");
        Ok(secret)
    }

    /// `GET <entries>?count=1` — return the millisecond `date` of the
    /// newest entry already known to Nightscout, or `None` when the
    /// server has no entries yet.
    ///
    /// The result list arrives either wrapped as `{"result": [...]}` (v3)
    /// or as a bare top-level array (v1); both shapes are accepted. A
    /// `404 Not Found` (some self-hosted NS instances expose no `entries`
    /// collection until the first write) is treated as "empty registry"
    /// and returns `Ok(None)` rather than an error. A non-JSON body (e.g.
    /// the v1 API falling back to its tab-separated format) also degrades
    /// to `Ok(None)`, so dedup is skipped rather than the upload aborted.
    pub async fn fetch_last_entry_date(&self) -> Result<Option<i64>, NsError> {
        let url = format!("{}?count=1", self.entries_url());
        let resp = self
            .send_authed(|| {
                self.http
                    .get(&url)
                    .header(reqwest::header::ACCEPT, "application/json")
            })
            .await?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(NsError::Unauthorized);
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            let code = status.as_u16();
            if code == 429 || (500..=599).contains(&code) {
                return Err(NsError::Retryable { status: code });
            }
            return Err(NsError::Status { status: code });
        }
        let raw = resp.bytes().await?;
        // Empty body — same meaning as an empty registry.
        if raw.is_empty() {
            return Ok(None);
        }
        // Try the wrapped shape first (newer NS); fall back to a bare
        // array (older NS / v1). When neither parses, treat as "no hint"
        // and let the sink post everything — better than erroring and
        // skipping the upload entirely.
        #[derive(serde::Deserialize)]
        struct Wrapped {
            result: Vec<RawEntry>,
        }
        #[derive(serde::Deserialize)]
        struct RawEntry {
            date: Option<i64>,
        }
        let entries: Vec<RawEntry> = if let Ok(w) = serde_json::from_slice::<Wrapped>(&raw) {
            w.result
        } else if let Ok(arr) = serde_json::from_slice::<Vec<RawEntry>>(&raw) {
            arr
        } else {
            tracing::warn!("ns lastEntry: decode failed; falling back to post-all");
            return Ok(None);
        };
        Ok(entries.into_iter().filter_map(|e| e.date).max())
    }

    /// `POST <entries>` with a JSON array of entries derived from
    /// `readings`. An empty `readings` slice is a no-op.
    ///
    /// Status mapping (per attempt):
    /// - 2xx → `Ok(())`. Empty body on `201 Created` is normal.
    /// - 401 → `NsError::Unauthorized` (terminal; in token mode only
    ///   after a JWT refresh + retry has also failed).
    /// - 429, 5xx → `NsError::Retryable { status }` — automatically
    ///   retried up to [`MAX_POST_RETRIES`] times with exponential
    ///   backoff (200 ms, 400 ms). The final failure surfaces with
    ///   the same status as the last attempt.
    /// - Anything else → `NsError::Status { status }` (terminal).
    ///
    /// Each retry increments
    /// `cgm_sink_post_retry_total{sink="nightscout", attempt=N}`. No
    /// `Retry-After` header parsing in V1 — the bounded backoff is
    /// chosen to ride out the 1–3 s blips typical of NS instances
    /// behind a CDN without amplifying real outages.
    pub async fn post_entries(&self, readings: &[Reading]) -> Result<(), NsError> {
        if readings.is_empty() {
            debug!("ns post_entries: empty batch, skipping");
            return Ok(());
        }
        let body: Vec<NsEntry> = readings
            .iter()
            .map(|r| entry_from_reading(r, self.device.as_deref(), self.app.as_deref()))
            .collect();

        let mut attempt: u32 = 0;
        loop {
            match self.try_post_entries(&body).await {
                Ok(()) => return Ok(()),
                Err(NsError::Retryable { status }) if attempt < MAX_POST_RETRIES => {
                    attempt += 1;
                    let delay = retry_backoff(attempt);
                    ::metrics::counter!(
                        "cgm_sink_post_retry_total",
                        "sink" => "nightscout",
                        "attempt" => attempt.to_string(),
                    )
                    .increment(1);
                    tracing::warn!(
                        attempt,
                        status,
                        delay_ms = delay.as_millis() as u64,
                        "ns retryable; backing off"
                    );
                    tokio::time::sleep(delay).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn try_post_entries(&self, body: &[NsEntry]) -> Result<(), NsError> {
        let url = self.entries_url();
        let resp = self
            .send_authed(|| {
                self.http
                    .post(&url)
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .json(body)
            })
            .await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(NsError::Unauthorized);
        }
        let code = status.as_u16();
        if code == 429 || (500..=599).contains(&code) {
            return Err(NsError::Retryable { status: code });
        }
        Err(NsError::Status { status: code })
    }
}

/// Bounded retry budget per `post_entries` call. With base 200 ms,
/// the worst-case wall time is `200 + 400 = 600 ms` of sleep before
/// returning the final error — well under the smallest valid
/// `[poller] interval_secs = 30 s`, so retries never bleed into the
/// next poll tick.
const MAX_POST_RETRIES: u32 = 2;

fn retry_backoff(attempt: u32) -> std::time::Duration {
    // attempt=1 → 200ms, attempt=2 → 400ms.
    std::time::Duration::from_millis(200_u64 << (attempt.saturating_sub(1)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use gluco_hub_core::{GlucoseMgDl, PatientId, SourceId, Trend};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn reading() -> Reading {
        Reading {
            patient_id: PatientId::new("p1").unwrap(),
            source_id: SourceId::new("llu").unwrap(),
            timestamp: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            glucose: GlucoseMgDl::new(142.0).unwrap(),
            trend: Trend::Flat,
        }
    }

    /// api-secret mode client (posts to the v1 entries API).
    fn client(server: &MockServer) -> NightscoutClient {
        NightscoutClient::new(server.uri(), SecretString::from("test-secret")).expect("client")
    }

    /// token mode client (mints a JWT, posts to the v3 entries API).
    /// The access token's last path segment is what the auth-endpoint
    /// mock matches on.
    fn token_client(server: &MockServer) -> NightscoutClient {
        NightscoutClient::with_access_token(server.uri(), SecretString::from("itest-ad3b1f9d"))
            .expect("client")
    }

    /// Mount the JWT mint endpoint returning `jwt_value`.
    async fn mount_jwt(server: &MockServer, jwt_value: &str) {
        Mock::given(method("GET"))
            .and(path("/api/v2/authorization/request/itest-ad3b1f9d"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "token": jwt_value })),
            )
            .mount(server)
            .await;
    }

    #[test]
    fn api_secret_header_matches_known_vector() {
        // sha1("test-secret") = fe1bae27cb7c1fb823f496f286e78f1d2ae87734
        assert_eq!(
            api_secret_header(&SecretString::from("test-secret")),
            "fe1bae27cb7c1fb823f496f286e78f1d2ae87734"
        );
    }

    #[test]
    fn rejects_empty_base_url() {
        let err = NightscoutClient::new("", SecretString::from("x")).unwrap_err();
        assert_eq!(err.error_code(), "NS005");
    }

    #[test]
    fn empty_batch_is_a_noop() {
        // No mock configured: if the client made an HTTP call, the test
        // would hang or fail. Instead it must short-circuit.
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let c = NightscoutClient::new("http://127.0.0.1:1", SecretString::from("x"))
                .expect("client");
            c.post_entries(&[]).await.expect("noop");
        });
    }

    #[tokio::test]
    async fn api_secret_posts_to_v1_with_correct_header_and_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/entries"))
            .and(header(
                "api-secret",
                "fe1bae27cb7c1fb823f496f286e78f1d2ae87734",
            ))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;

        client(&server)
            .post_entries(&[reading()])
            .await
            .expect("post");

        let req = server
            .received_requests()
            .await
            .expect("requests")
            .into_iter()
            .find(|r| r.url.path() == "/api/v1/entries")
            .expect("entries request");
        let body: serde_json::Value = serde_json::from_slice(&req.body).expect("json");
        let entry = body.get(0).expect("one entry");
        assert_eq!(entry["sgv"], 142);
        assert_eq!(entry["direction"], "Flat");
        assert_eq!(entry["type"], "sgv");
        assert!(entry.get("trend").is_none());
        assert_eq!(entry["date"], 1_700_000_000_000_i64);
    }

    #[tokio::test]
    async fn token_mode_mints_jwt_then_posts_with_bearer() {
        let server = MockServer::start().await;
        mount_jwt(&server, "jwt-abc").await;
        Mock::given(method("POST"))
            .and(path("/api/v3/entries"))
            .and(header("authorization", "Bearer jwt-abc"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;

        token_client(&server)
            .post_entries(&[reading()])
            .await
            .expect("post");

        // Exactly one auth round-trip, then the v3 POST.
        let reqs = server.received_requests().await.expect("requests");
        let auth_hits = reqs
            .iter()
            .filter(|r| r.url.path() == "/api/v2/authorization/request/itest-ad3b1f9d")
            .count();
        assert_eq!(auth_hits, 1, "one JWT mint");
        assert!(
            reqs.iter()
                .any(|r| r.method.as_str() == "POST" && r.url.path() == "/api/v3/entries"),
            "v3 POST happened"
        );
    }

    #[tokio::test]
    async fn token_mode_refreshes_jwt_on_401_then_succeeds() {
        let server = MockServer::start().await;
        // First mint → stale token; second mint → fresh token.
        Mock::given(method("GET"))
            .and(path("/api/v2/authorization/request/itest-ad3b1f9d"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "token": "stale" })),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/authorization/request/itest-ad3b1f9d"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "token": "fresh" })),
            )
            .mount(&server)
            .await;
        // Stale bearer → 401; fresh bearer → 201.
        Mock::given(method("POST"))
            .and(path("/api/v3/entries"))
            .and(header("authorization", "Bearer stale"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v3/entries"))
            .and(header("authorization", "Bearer fresh"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;

        token_client(&server)
            .post_entries(&[reading()])
            .await
            .expect("post after refresh");
    }

    #[tokio::test]
    async fn token_mode_auth_endpoint_401_maps_unauthorized() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/authorization/request/itest-ad3b1f9d"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let err = token_client(&server)
            .post_entries(&[reading()])
            .await
            .unwrap_err();
        assert!(matches!(err, NsError::Unauthorized));
    }

    #[tokio::test]
    async fn token_mode_auth_response_without_token_maps_ns006() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/authorization/request/itest-ad3b1f9d"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        let err = token_client(&server)
            .post_entries(&[reading()])
            .await
            .unwrap_err();
        assert_eq!(err.error_code(), "NS006");
    }

    #[tokio::test]
    async fn maps_401_to_unauthorized() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/entries"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let err = client(&server)
            .post_entries(&[reading()])
            .await
            .unwrap_err();
        assert!(matches!(err, NsError::Unauthorized));
    }

    #[tokio::test]
    async fn maps_502_to_retryable_after_exhausting_retries() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/entries"))
            .respond_with(ResponseTemplate::new(502))
            .mount(&server)
            .await;
        let err = client(&server)
            .post_entries(&[reading()])
            .await
            .unwrap_err();
        assert!(matches!(err, NsError::Retryable { status: 502 }));
        // 1 initial attempt + MAX_POST_RETRIES retries (2) = 3 POSTs.
        let posts = server
            .received_requests()
            .await
            .expect("requests")
            .into_iter()
            .filter(|r| r.method.as_str() == "POST")
            .count();
        assert_eq!(posts, 3, "expected initial + 2 retries");
    }

    #[tokio::test]
    async fn retries_succeed_after_two_502s_then_201() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/entries"))
            .respond_with(ResponseTemplate::new(502))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/entries"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        client(&server)
            .post_entries(&[reading()])
            .await
            .expect("ok after retries");
        let posts = server
            .received_requests()
            .await
            .expect("requests")
            .into_iter()
            .filter(|r| r.method.as_str() == "POST")
            .count();
        assert_eq!(posts, 3, "two 502s + one 201");
    }

    #[tokio::test]
    async fn non_retryable_400_is_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/entries"))
            .respond_with(ResponseTemplate::new(400))
            .mount(&server)
            .await;
        let err = client(&server)
            .post_entries(&[reading()])
            .await
            .unwrap_err();
        assert!(matches!(err, NsError::Status { status: 400 }));
        let posts = server
            .received_requests()
            .await
            .expect("requests")
            .into_iter()
            .filter(|r| r.method.as_str() == "POST")
            .count();
        assert_eq!(posts, 1, "400 must be terminal — no retry");
    }

    #[tokio::test]
    async fn maps_429_to_retryable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/entries"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;
        let err = client(&server)
            .post_entries(&[reading()])
            .await
            .unwrap_err();
        assert!(matches!(err, NsError::Retryable { status: 429 }));
    }
}
