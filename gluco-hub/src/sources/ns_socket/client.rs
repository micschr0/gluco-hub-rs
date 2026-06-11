// SPDX-License-Identifier: AGPL-3.0-or-later

//! Socket.IO client for the Nightscout real-time feed.
//!
//! # Status: V6 scaffold
//!
//! This module currently holds only the connection parameters and a stub
//! [`NsSocketClient::connect`] that returns [`NsSocketError::NotImplemented`]
//! (`[NSS001]`). The real Socket.IO connect / authorize / subscribe loop is
//! intentionally left out so the crate compiles and lints clean while the
//! feature is incubated. See the module-level docs in [`super`] for the
//! verified wire contract this stub will eventually implement.
//!
//! # Dependency note (no new runtime deps yet)
//!
//! Nightscout speaks **Socket.IO v4** over an Engine.IO websocket transport.
//! Rust has no first-party Socket.IO client; the leading candidate is
//! [`rust-socketio`](https://crates.io/crates/rust-socketio). When the loop
//! lands it MUST be added with a **rustls** TLS backend (NO OpenSSL — see
//! `CLAUDE.md` Don'ts) and only after `cargo deny check` still passes. Until
//! then this scaffold pulls in **zero** new crates.

use std::time::Duration;

use secrecy::SecretString;

use super::error::NsSocketError;

/// How the client authenticates to Nightscout's Socket.IO endpoint during
/// the `authorize` handshake. Fixed set — never a magic string.
///
/// Verified against cgm-remote-monitor `lib/server/websocket.js`: the
/// `authorize` payload carries a `secret` (an API-secret SHA-1 hash) and/or
/// a `token` (an access token like `myreader-0123456789abcdef`). Modern
/// Nightscout deployments prefer token auth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NsAuthMode {
    /// Present the access token in the `authorize` payload's `token` field.
    Token,
    /// Present the SHA-1 of the API secret in the `secret` field.
    ApiSecret,
}

/// Immutable connection parameters for the Nightscout Socket.IO feed.
///
/// Constructed from `[source.ns_socket]` config. The secret is held as a
/// [`SecretString`] and is NEVER logged (see `CLAUDE.md` secrets rules).
pub struct NsSocketClient {
    /// Base URL of the Nightscout site, e.g. `https://ns.example.com`. The
    /// Socket.IO transport connects to the default namespace (`/`) at the
    /// Engine.IO path `/socket.io/` under this origin.
    base_url: String,
    /// Which credential to send in the `authorize` handshake.
    auth_mode: NsAuthMode,
    /// The credential itself (access token or raw API secret). Wrapped so it
    /// cannot accidentally land in a log line.
    secret: SecretString,
    /// History window, in hours, requested in the `authorize` payload's
    /// `history` field. Nightscout defaults to 48; we let the operator cap
    /// it to bound the initial replay.
    history_hours: u32,
    /// Connect/handshake timeout.
    connect_timeout: Duration,
}

impl NsSocketClient {
    /// Default `authorize` `history` window in hours when unset, matching
    /// Nightscout's own server-side default.
    pub const DEFAULT_HISTORY_HOURS: u32 = 48;

    /// Default connect/handshake timeout.
    pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

    pub fn new(
        base_url: String,
        auth_mode: NsAuthMode,
        secret: SecretString,
        history_hours: u32,
    ) -> Self {
        Self {
            base_url,
            auth_mode,
            secret,
            history_hours,
            connect_timeout: Self::DEFAULT_CONNECT_TIMEOUT,
        }
    }

    /// Base URL accessor (origin only — no secret material).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Selected auth mode.
    pub fn auth_mode(&self) -> NsAuthMode {
        self.auth_mode
    }

    /// Requested history window in hours.
    pub fn history_hours(&self) -> u32 {
        self.history_hours
    }

    /// Connect to the Nightscout Socket.IO endpoint and perform the
    /// `authorize` handshake.
    ///
    /// # V6 scaffold
    ///
    /// Not yet implemented — returns [`NsSocketError::NotImplemented`]
    /// (`[NSS001]`). The real implementation will:
    ///
    /// 1. Open an Engine.IO websocket to `<base_url>/socket.io/` (default
    ///    namespace `/`, **wss** for `https` origins).
    /// 2. Emit `authorize` with `{ client: "gluco-hub", token | secret,
    ///    history: <history_hours> }` and await the server's `connected`
    ///    event plus the ack `{ read, write, write_treatment }`.
    /// 3. Subscribe to the `dataUpdate` event (delta payloads carrying an
    ///    `sgvs` array) and map each entry to a [`gluco_hub_core::Reading`].
    pub async fn connect(&self) -> Result<(), NsSocketError> {
        // Touch the secret-bearing field through a non-logging path so the
        // scaffold does not trip dead-code lints, without exposing it.
        let _ = (&self.secret, self.connect_timeout);
        Err(NsSocketError::NotImplemented)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> NsSocketClient {
        NsSocketClient::new(
            "https://ns.example.com".to_string(),
            NsAuthMode::Token,
            SecretString::from("test-token"),
            NsSocketClient::DEFAULT_HISTORY_HOURS,
        )
    }

    #[test]
    fn accessors_expose_non_secret_fields() {
        let c = sample();
        assert_eq!(c.base_url(), "https://ns.example.com");
        assert_eq!(c.auth_mode(), NsAuthMode::Token);
        assert_eq!(c.history_hours(), 48);
    }

    #[tokio::test]
    async fn connect_is_not_yet_implemented() {
        let err = sample().connect().await.expect_err("scaffold stub");
        assert_eq!(err.error_code(), "NSS001");
    }
}
