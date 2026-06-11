// SPDX-License-Identifier: AGPL-3.0-or-later

use thiserror::Error;

/// Errors surfaced by the Nightscout Socket.IO source. Each variant carries
/// a stable `error_code` (the bracketed prefix in `Display`, prefix `NSS`)
/// so logs and metrics labels stay grep-friendly across versions.
///
/// V6 scaffold: the connect/subscribe loop is not yet implemented, hence the
/// [`NsSocketError::NotImplemented`] variant. It is a typed error rather than
/// a `todo!()` / `unimplemented!()` macro so the crate compiles and the
/// poller degrades gracefully (it surfaces the error code) instead of
/// panicking.
#[derive(Debug, Error)]
pub enum NsSocketError {
    /// The Socket.IO connect/subscribe loop is not yet wired up. Emitted by
    /// the scaffold so a misconfigured deployment fails loudly with a stable
    /// code rather than silently returning no readings.
    #[error("[NSS001] NS-Socket source not yet implemented (V6 scaffold)")]
    NotImplemented,

    /// Underlying transport / websocket failure (connect, TLS, read).
    #[error("[NSS002] transport error: {0}")]
    Transport(String),

    /// The Socket.IO `authorize` handshake was rejected by Nightscout.
    #[error("[NSS003] authorization rejected by Nightscout")]
    Unauthorized,

    /// A `dataUpdate` payload could not be parsed into readings.
    #[error("[NSS004] malformed dataUpdate payload: {reason}")]
    Protocol { reason: String },

    /// A timestamp in an `sgv`/entry could not be interpreted.
    #[error("[NSS005] could not parse entry timestamp: {raw}")]
    BadTimestamp { raw: String },
}

impl NsSocketError {
    /// Stable string identifier per error variant. The `Display` impl above
    /// embeds the same code; exposing it as a method lets downstream
    /// classification logic match without parsing the formatted message.
    #[allow(dead_code)]
    pub fn error_code(&self) -> &'static str {
        match self {
            NsSocketError::NotImplemented => "NSS001",
            NsSocketError::Transport(_) => "NSS002",
            NsSocketError::Unauthorized => "NSS003",
            NsSocketError::Protocol { .. } => "NSS004",
            NsSocketError::BadTimestamp { .. } => "NSS005",
        }
    }
}

impl From<NsSocketError> for gluco_hub_core::CoreError {
    fn from(value: NsSocketError) -> Self {
        gluco_hub_core::CoreError::Source {
            message: value.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(NsSocketError::NotImplemented.error_code(), "NSS001");
        assert_eq!(NsSocketError::Transport("x".into()).error_code(), "NSS002");
        assert_eq!(NsSocketError::Unauthorized.error_code(), "NSS003");
        assert_eq!(
            NsSocketError::Protocol { reason: "x".into() }.error_code(),
            "NSS004"
        );
        assert_eq!(
            NsSocketError::BadTimestamp { raw: "x".into() }.error_code(),
            "NSS005"
        );
    }

    #[test]
    fn display_embeds_bracketed_code() {
        assert!(
            NsSocketError::NotImplemented
                .to_string()
                .starts_with("[NSS001]")
        );
    }

    #[test]
    fn maps_into_core_source_error() {
        let core: gluco_hub_core::CoreError = NsSocketError::NotImplemented.into();
        assert_eq!(core.error_code(), "CORE003");
    }
}
