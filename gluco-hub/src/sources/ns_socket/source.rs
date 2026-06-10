// SPDX-License-Identifier: AGPL-3.0-or-later

//! `NsSocketSource` — adapts the Nightscout Socket.IO feed to the `Source`
//! trait.
//!
//! # Status: V6 scaffold
//!
//! The struct and trait wiring are complete, but [`NsSocketClient::connect`]
//! is a stub. [`NsSocketSource::fetch_latest`] therefore surfaces
//! `[NSS001]` (mapped to `CoreError::Source`) until the Socket.IO loop is
//! implemented. The poller treats this like any other source error, so a
//! deployment that enables this source before it is finished fails loudly
//! (visible error code) rather than silently producing no data.
//!
//! Once the loop lands, the design is **push, adapted to pull**: a background
//! task owns the Socket.IO connection and keeps the latest `sgvs` deltas in
//! shared state; `fetch_latest` returns a snapshot of that state. This keeps
//! the existing interval poller unchanged (see `docs/EXTENDING.md`).

use async_trait::async_trait;
use gluco_hub_core::{CoreError, Reading, Source, SourceId};
use tracing::warn;

use super::client::NsSocketClient;

/// Nightscout-as-a-source. Holds the configured Socket.IO client and the
/// stable [`SourceId`] used in logs and metrics.
pub struct NsSocketSource {
    id: SourceId,
    client: NsSocketClient,
}

impl NsSocketSource {
    pub fn new(id: SourceId, client: NsSocketClient) -> Self {
        Self { id, client }
    }
}

#[async_trait]
impl Source for NsSocketSource {
    fn id(&self) -> &SourceId {
        &self.id
    }

    async fn fetch_latest(&self) -> Result<Vec<Reading>, CoreError> {
        // V6 scaffold: the Socket.IO connect/subscribe loop is not yet
        // implemented. `connect` returns `[NSS001]`, which maps to
        // `CoreError::Source` via the `From` impl in `error.rs`.
        self.client.connect().await.map_err(|e| {
            warn!(
                source_id = %self.id.as_str(),
                error_code = e.error_code(),
                "ns_socket source not yet implemented"
            );
            CoreError::from(e)
        })?;
        // Unreachable until `connect` is implemented; kept so the trait
        // signature and return type are exercised by the type checker.
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::super::client::NsAuthMode;
    use super::*;
    use secrecy::SecretString;

    fn source() -> NsSocketSource {
        let client = NsSocketClient::new(
            "https://ns.example.com".to_string(),
            NsAuthMode::Token,
            SecretString::from("test-token"),
            NsSocketClient::DEFAULT_HISTORY_HOURS,
        );
        NsSocketSource::new(SourceId::new("ns_socket").expect("valid id"), client)
    }

    #[test]
    fn exposes_source_id() {
        assert_eq!(source().id().as_str(), "ns_socket");
    }

    #[tokio::test]
    async fn fetch_latest_surfaces_not_implemented() {
        let err = source().fetch_latest().await.expect_err("scaffold stub");
        // Mapped through CoreError::Source — the NSS001 text is preserved in
        // the message so operators can grep for it.
        assert_eq!(err.error_code(), "CORE003");
        assert!(err.to_string().contains("NSS001"), "got: {err}");
    }
}
