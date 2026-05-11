// SPDX-License-Identifier: AGPL-3.0-or-later

//! MQTT-sink error types with stable `[MQTTxxx]` prefixes.
//!
//! The fan-out wrapper in `main.rs` extracts the prefix into the
//! `error_code` label of `cgm_sink_push_errors_total`, so these codes
//! are part of the project's observable contract.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MqttError {
    /// `[MQTT001]` TCP / socket-level transport failure.
    #[error("[MQTT001] transport error: {message}")]
    Transport { message: String },

    /// `[MQTT002]` TLS handshake failed (bad cert, hostname mismatch, …).
    #[error("[MQTT002] TLS handshake failed: {message}")]
    TlsHandshake { message: String },

    /// `[MQTT003]` Broker refused the CONNECT (bad credentials, banned
    /// client-id, server capacity, …).
    #[error("[MQTT003] broker refused connection: {reason}")]
    ConnectRefused { reason: String },

    /// `[MQTT004]` Publish channel closed or full — the EventLoop task
    /// has died or is back-pressured.
    #[error("[MQTT004] publish channel error: {message}")]
    Channel { message: String },

    /// `[MQTT005]` Invalid payload or local serialisation error.
    #[error("[MQTT005] payload error: {message}")]
    Payload { message: String },

    /// `[MQTT006]` Keep-alive / network timeout.
    #[error("[MQTT006] keep-alive timeout: {message}")]
    KeepAliveTimeout { message: String },

    /// `[MQTT007]` MQTT protocol-state error or unexpected packet.
    #[error("[MQTT007] MQTT protocol error: {message}")]
    Protocol { message: String },
}

impl MqttError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Transport { .. } => "MQTT001",
            Self::TlsHandshake { .. } => "MQTT002",
            Self::ConnectRefused { .. } => "MQTT003",
            Self::Channel { .. } => "MQTT004",
            Self::Payload { .. } => "MQTT005",
            Self::KeepAliveTimeout { .. } => "MQTT006",
            Self::Protocol { .. } => "MQTT007",
        }
    }
}

/// Map a `rumqttc::v5::ConnectionError` (yielded by `EventLoop::poll`)
/// to a stable `MqttError` variant. Used by the poll-loop task so
/// reconnect log-lines carry a concrete error code.
pub fn classify_connection_error(e: &rumqttc::v5::ConnectionError) -> MqttError {
    use rumqttc::v5::ConnectionError;
    match e {
        ConnectionError::Io(io) => MqttError::Transport {
            message: io.to_string(),
        },
        ConnectionError::Tls(tls) => MqttError::TlsHandshake {
            message: tls.to_string(),
        },
        ConnectionError::ConnectionRefused(code) => MqttError::ConnectRefused {
            reason: format!("{code:?}"),
        },
        ConnectionError::Timeout(_) => MqttError::KeepAliveTimeout {
            message: e.to_string(),
        },
        other => MqttError::Protocol {
            message: other.to_string(),
        },
    }
}

/// Map a `rumqttc::v5::ClientError` (yielded by `AsyncClient::publish`)
/// to a `MqttError`. Both `Request` and `TryRequest` variants signal
/// that the EventLoop channel is dead or full.
pub fn classify_client_error(e: rumqttc::v5::ClientError) -> MqttError {
    MqttError::Channel {
        message: e.to_string(),
    }
}
