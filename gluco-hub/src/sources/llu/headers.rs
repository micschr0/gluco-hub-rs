// SPDX-License-Identifier: AGPL-3.0-or-later

use reqwest::header::{
    ACCEPT_ENCODING, AUTHORIZATION, CACHE_CONTROL, CONNECTION, CONTENT_TYPE, HeaderMap, HeaderName,
    HeaderValue, USER_AGENT,
};
use secrecy::ExposeSecret;

use super::auth::LluTokens;

/// LibreLink Up app version sent in the `version` header. LibreView
/// occasionally rejects older values, so this is overridable at runtime.
pub const DEFAULT_LLU_VERSION: &str = "4.17.0";

/// LibreLink Up product identifier. `llu.android` is also valid but the
/// iOS string is the most widely deployed.
pub const LLU_PRODUCT: &str = "llu.ios";

/// User-Agent string. LibreView's WAF inspects this and silently 4xxs
/// unknown agents.
pub const LLU_USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU OS 17_4.1 like Mac OS X) AppleWebKit/536.26 (KHTML, like Gecko) Mobile/14E5239e Safari/9537.53";

const HEADER_VERSION: HeaderName = HeaderName::from_static("version");
const HEADER_PRODUCT: HeaderName = HeaderName::from_static("product");
const HEADER_ACCOUNT_ID: HeaderName = HeaderName::from_static("account-id");

/// Headers required by every LLU request, with `account-id` empty until a
/// successful login produces one. The version is configurable so a
/// deployment can pin against a specific LLU app release.
pub fn base_headers(version: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(LLU_USER_AGENT));
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json;charset=UTF-8"),
    );
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(CONNECTION, HeaderValue::from_static("keep-alive"));
    headers.insert(HEADER_PRODUCT, HeaderValue::from_static(LLU_PRODUCT));
    // `version` is dynamic so we can't use `from_static`. Falling back to a
    // safe default if a caller passes garbage — `HeaderValue::from_str`
    // rejects non-ASCII, which would otherwise panic later inside reqwest.
    let version_value = HeaderValue::from_str(version)
        .unwrap_or_else(|_| HeaderValue::from_static(DEFAULT_LLU_VERSION));
    headers.insert(HEADER_VERSION, version_value);
    headers.insert(HEADER_ACCOUNT_ID, HeaderValue::from_static(""));
    headers
}

/// Headers for an authenticated request: base + Bearer token + account-id.
pub fn authorized_headers(tokens: &LluTokens, version: &str) -> HeaderMap {
    let mut headers = base_headers(version);
    let bearer = format!("Bearer {}", tokens.bearer.expose_secret());
    if let Ok(value) = HeaderValue::from_str(&bearer) {
        headers.insert(AUTHORIZATION, value);
    }
    if let Ok(value) = HeaderValue::from_str(&tokens.account_id_hash) {
        headers.insert(HEADER_ACCOUNT_ID, value);
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_headers_carry_required_fields() {
        let h = base_headers("4.17.0");
        assert_eq!(h.get("product").unwrap(), "llu.ios");
        assert_eq!(h.get("version").unwrap(), "4.17.0");
        assert_eq!(h.get("account-id").unwrap(), "");
        assert!(h.get(USER_AGENT).is_some());
    }

    #[test]
    fn invalid_version_falls_back_to_default() {
        let h = base_headers("\u{0001}bad");
        assert_eq!(h.get("version").unwrap(), DEFAULT_LLU_VERSION);
    }
}
