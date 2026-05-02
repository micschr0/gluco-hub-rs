//! MQTT v5 sink (V2). Backed by `rumqttc 0.25` (rustls only — no
//! OpenSSL anywhere in the tree per CLAUDE.md).
//!
//! Topic layout:
//! * `<prefix>/glucose` — one JSON payload per Reading, QoS configurable,
//!   retain = false (stale glucose is dangerous).
//! * `<prefix>/_health` — retained, QoS 1; LWT carries `online: false`,
//!   poll loop publishes `online: true` on every successful ConnAck.
//!
//! Wire schema is `v: 1`. Bumping `v` is a breaking change for
//! subscribers and must be accompanied by a doc update.

pub mod error;
pub mod sink;
pub mod wire;

pub use sink::MqttSink;

#[cfg(test)]
mod tests {
    use super::*;
    use cgm_bridge_core::{GlucoseMgDl, PatientId, Reading, SourceId, Trend};
    use chrono::Utc;

    fn one_reading() -> Reading {
        Reading {
            patient_id: PatientId::new("p1").unwrap(),
            source_id: SourceId::new("llu").unwrap(),
            timestamp: Utc::now(),
            glucose: GlucoseMgDl::new(120.0).unwrap(),
            trend: Trend::Flat,
        }
    }

    /// Smoke test: payload module produces JSON of the agreed shape.
    /// (Broker-level integration is left to operator-run smoke scripts.)
    #[test]
    fn glucose_payload_is_v1_json() {
        let r = one_reading();
        let p = wire::glucose_payload(&r, true);
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains(r#""v":1"#));
        assert!(json.contains(r#""mgdl":120.0"#));
        assert!(json.contains(r#""trend":"Flat""#));
        assert!(json.contains(r#""patient":"p1""#));
    }

    #[test]
    fn error_codes_match_display() {
        use super::error::MqttError;
        let e = MqttError::Transport {
            message: "x".into(),
        };
        assert_eq!(e.code(), "MQTT001");
        assert!(e.to_string().contains("[MQTT001]"));

        let e = MqttError::ConnectRefused {
            reason: "BadUserName".into(),
        };
        assert_eq!(e.code(), "MQTT003");
        assert!(e.to_string().contains("[MQTT003]"));
    }
}
