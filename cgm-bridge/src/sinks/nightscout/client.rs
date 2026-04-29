//! Nightscout v3 HTTP client.
//!
//! Auth path: `api-secret: <sha1_hex(API_SECRET)>` — Nightscout v3
//! continues to accept the legacy SHA-1 header for backward
//! compatibility. The modern path used by the reference port is
//! `Authorization: Bearer <jwt>` obtained from
//! `/api/v2/authorization/request/<accessToken>`.
//!
//! We deliberately ship the SHA-1 path for V1 because:
//!
//! - it works against every NS deployment we tested against (the JWT
//!   path is opt-in on the NS side),
//! - it adds zero round-trips per scrape, and
//! - it is wiremock-testable without a JWT issuer.
//!
//! The JWT path is on the V2 roadmap as `NsAuth::Bearer`.
//!
//! SHA-1 here is a request-shape choice mandated by NS, not a security
//! choice — actual transport security comes from the rustls-protected
//! TLS connection to the NS host.

use cgm_bridge_core::Reading;
use secrecy::{ExposeSecret, SecretString};
use sha1::{Digest, Sha1};
use thiserror::Error;
use tracing::debug;

use super::wire::{NsEntry, entry_from_reading};

/// Errors surfaced by the Nightscout client. Each variant carries a
/// stable `error_code` (the bracketed prefix in `Display`) so logs and
/// metrics labels stay grep-friendly.
#[derive(Debug, Error)]
pub enum NsError {
    #[error("[NS001] HTTP transport error: {0}")]
    Transport(String),

    #[error("[NS002] Nightscout rejected api-secret: 401")]
    Unauthorized,

    #[error("[NS003] Nightscout returned non-success status: {status}")]
    Status { status: u16 },

    #[error("[NS004] Nightscout returned a transient error: {status}")]
    Retryable { status: u16 },

    #[error("[NS005] invalid base URL: {reason}")]
    InvalidBaseUrl { reason: String },
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

#[derive(Debug, Clone)]
pub struct NightscoutClient {
    base_url: String,
    secret: SecretString,
    http: reqwest::Client,
    device: Option<String>,
    app: Option<String>,
}

impl NightscoutClient {
    pub fn new(base_url: impl Into<String>, secret: SecretString) -> Result<Self, NsError> {
        let base_url = base_url.into();
        let trimmed = base_url.trim_end_matches('/').to_string();
        if trimmed.is_empty() {
            return Err(NsError::InvalidBaseUrl {
                reason: "base_url is empty".into(),
            });
        }
        let http = reqwest::Client::builder().build().map_err(NsError::from)?;
        Ok(Self {
            base_url: trimmed,
            secret,
            http,
            device: None,
            app: None,
        })
    }

    /// Identify this service in the Nightscout UI's source column.
    /// Equivalent to the reference port's `NIGHTSCOUT_DEVICE_NAME`.
    pub fn with_device(mut self, device: impl Into<String>) -> Self {
        self.device = Some(device.into());
        self
    }

    /// App name attached to every uploaded entry. Equivalent to the
    /// reference port's `app` config value (default
    /// `nightscout-librelink-up`).
    pub fn with_app(mut self, app: impl Into<String>) -> Self {
        self.app = Some(app.into());
        self
    }

    fn entries_url(&self) -> String {
        format!("{}/api/v3/entries", self.base_url)
    }

    /// `POST /api/v3/entries` with a JSON array of entries derived from
    /// `readings`. An empty `readings` slice is a no-op.
    ///
    /// Status mapping:
    /// - 2xx → `Ok(())`. Empty body on `201 Created` is normal.
    /// - 401 → `NsError::Unauthorized`.
    /// - 429, 5xx → `NsError::Retryable { status }` (caller decides backoff).
    /// - Anything else → `NsError::Status { status }`.
    pub async fn post_entries(&self, readings: &[Reading]) -> Result<(), NsError> {
        if readings.is_empty() {
            debug!("ns post_entries: empty batch, skipping");
            return Ok(());
        }
        let body: Vec<NsEntry> = readings
            .iter()
            .map(|r| entry_from_reading(r, self.device.as_deref(), self.app.as_deref()))
            .collect();

        let resp = self
            .http
            .post(self.entries_url())
            .header("api-secret", api_secret_header(&self.secret))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
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

#[cfg(test)]
mod tests {
    use super::*;
    use cgm_bridge_core::{GlucoseMgDl, PatientId, SourceId, Trend};
    use chrono::{TimeZone, Utc};
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

    fn client(server: &MockServer) -> NightscoutClient {
        NightscoutClient::new(server.uri(), SecretString::from("test-secret")).expect("client")
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
    async fn happy_path_posts_with_correct_header_and_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v3/entries"))
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
            .find(|r| r.url.path() == "/api/v3/entries")
            .expect("entries request");
        let body: serde_json::Value = serde_json::from_slice(&req.body).expect("json");
        let entry = body.get(0).expect("one entry");
        assert_eq!(entry["sgv"], 142);
        assert_eq!(entry["direction"], "Flat");
        assert_eq!(entry["type"], "sgv");
        // No numeric trend field — matches reference.
        assert!(entry.get("trend").is_none());
        assert_eq!(entry["date"], 1_700_000_000_000_i64);
    }

    #[tokio::test]
    async fn maps_401_to_unauthorized() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v3/entries"))
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
    async fn maps_502_to_retryable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v3/entries"))
            .respond_with(ResponseTemplate::new(502))
            .mount(&server)
            .await;
        let err = client(&server)
            .post_entries(&[reading()])
            .await
            .unwrap_err();
        assert!(matches!(err, NsError::Retryable { status: 502 }));
    }

    #[tokio::test]
    async fn maps_429_to_retryable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v3/entries"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;
        let err = client(&server)
            .post_entries(&[reading()])
            .await
            .unwrap_err();
        assert!(matches!(err, NsError::Retryable { status: 429 }));
    }

    #[tokio::test]
    async fn maps_400_to_status() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v3/entries"))
            .respond_with(ResponseTemplate::new(400))
            .mount(&server)
            .await;
        let err = client(&server)
            .post_entries(&[reading()])
            .await
            .unwrap_err();
        assert!(matches!(err, NsError::Status { status: 400 }));
    }
}
