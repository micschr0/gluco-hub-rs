//! MQTT sink — V2 placeholder.
//!
//! Per CLAUDE.md's roadmap, MQTT lands in V2. This module exists so
//! V2 can plug in a `rumqttc`-backed implementation without
//! introducing a new feature flag, restructuring `sinks/mod.rs`, or
//! changing how `build_sinks` discovers concrete sinks. Every push
//! returns `[MQTT001]` until the real client lands.
//!
//! Intentionally NOT wired into `build_sinks` — operators who flip
//! the feature flag and configure `[sink.mqtt]` would otherwise see
//! every poll fail with `[MQTT001]`. The stub is kept reachable
//! through the public API so V2 can swap the body in place.

use async_trait::async_trait;
use cgm_bridge_core::{CoreError, Reading, Sink};

/// V2 placeholder. Construction is free; every push fails with a
/// stable `[MQTT001]` prefix so the upcoming real implementation can
/// be dropped in without changing call sites or test fixtures.
///
/// `dead_code` is allowed because production code never constructs
/// the placeholder — only V2 will, once the real client lands.
#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct MqttSink;

#[allow(dead_code)]
impl MqttSink {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Sink for MqttSink {
    fn name(&self) -> &'static str {
        "mqtt"
    }

    async fn push(&self, _readings: &[Reading]) -> Result<(), CoreError> {
        Err(CoreError::Sink {
            message: "[MQTT001] MQTT sink is a V2 placeholder; not yet implemented".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn placeholder_returns_v2_error() {
        let sink = MqttSink::new();
        assert_eq!(sink.name(), "mqtt");
        let err = sink.push(&[]).await.unwrap_err();
        let CoreError::Sink { message } = err else {
            panic!("expected Sink error, got {err:?}");
        };
        assert!(message.contains("[MQTT001]"), "got: {message}");
    }
}
