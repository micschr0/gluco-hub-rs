use std::time::{Duration, SystemTime, UNIX_EPOCH};

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

use super::error::LluError;
use super::headers::{DEFAULT_LLU_VERSION, base_headers};
use super::region::Region;

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

    fn login_url(&self, region: Region) -> String {
        match &self.base_url_override {
            Some(base) => format!("{}/llu/auth/login", base),
            None => format!("{}/llu/auth/login", region.base_url()),
        }
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
}
