//! Stable on-the-wire payload shapes for the MQTT sink.
//!
//! Schema is versioned via the `v` field. Bumping `v` is a breaking
//! change for downstream subscribers and must coincide with a
//! documented migration path in `docs/ARCHITECTURE.md`.

use cgm_bridge_core::Reading;
use serde::Serialize;

/// `v: 1` glucose payload published to `<prefix>/glucose`.
///
/// `mgdl` is the canonical unit (no rounding). `mmol` is derived and
/// rounded to one decimal — convenient for European subscribers and
/// for Home Assistant `value_template` use in V3.
#[derive(Debug, Serialize, PartialEq)]
pub struct GlucosePayload<'a> {
    pub v: u8,
    pub ts: i64,
    pub mgdl: f64,
    pub mmol: f64,
    pub trend: &'static str,
    pub source: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patient: Option<&'a str>,
}

/// `v: 1` health payload published retained to `<prefix>/_health`.
/// The LWT carries `{ "online": false, "v": 1 }`; on connect we
/// overwrite with `online = true`.
#[derive(Debug, Serialize, PartialEq)]
pub struct HealthPayload {
    pub online: bool,
    pub v: u8,
}

/// Build the glucose payload for one Reading.
///
/// `include_patient` controls whether the patient_id is emitted; when
/// `false`, the field is omitted entirely (privacy-by-default for
/// shared brokers).
pub fn glucose_payload<'a>(reading: &'a Reading, include_patient: bool) -> GlucosePayload<'a> {
    let mgdl = reading.glucose.get();
    let mmol = (mgdl / 18.015_59 * 10.0).round() / 10.0;
    GlucosePayload {
        v: 1,
        ts: reading.timestamp.timestamp_millis(),
        mgdl,
        mmol,
        trend: trend_to_str(reading.trend),
        source: reading.source_id.as_str(),
        patient: if include_patient {
            Some(reading.patient_id.as_str())
        } else {
            None
        },
    }
}

pub fn health_payload(online: bool) -> HealthPayload {
    HealthPayload { online, v: 1 }
}

/// Stable wire-form of the Trend enum. PascalCase matches the JSON
/// representation already produced by serde on `Reading` itself.
fn trend_to_str(t: cgm_bridge_core::Trend) -> &'static str {
    use cgm_bridge_core::Trend;
    match t {
        Trend::DoubleUp => "DoubleUp",
        Trend::SingleUp => "SingleUp",
        Trend::FortyFiveUp => "FortyFiveUp",
        Trend::Flat => "Flat",
        Trend::FortyFiveDown => "FortyFiveDown",
        Trend::SingleDown => "SingleDown",
        Trend::DoubleDown => "DoubleDown",
        Trend::NotComputable => "NotComputable",
        Trend::RateOutOfRange => "RateOutOfRange",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cgm_bridge_core::{GlucoseMgDl, PatientId, Reading, SourceId, Trend};
    use chrono::{TimeZone, Utc};

    fn one_reading(mgdl: f64, trend: Trend) -> Reading {
        Reading {
            patient_id: PatientId::new("p1").unwrap(),
            source_id: SourceId::new("llu").unwrap(),
            timestamp: Utc.timestamp_millis_opt(1_750_000_000_000).unwrap(),
            glucose: GlucoseMgDl::new(mgdl).unwrap(),
            trend,
        }
    }

    #[test]
    fn glucose_payload_schema_v1() {
        let r = one_reading(120.0, Trend::Flat);
        let p = glucose_payload(&r, true);
        assert_eq!(p.v, 1);
        assert_eq!(p.ts, 1_750_000_000_000);
        assert_eq!(p.mgdl, 120.0);
        // 120 / 18.01559 ≈ 6.66 → rounded to 6.7
        assert_eq!(p.mmol, 6.7);
        assert_eq!(p.trend, "Flat");
        assert_eq!(p.source, "llu");
        assert_eq!(p.patient, Some("p1"));
    }

    #[test]
    fn glucose_payload_omits_patient_when_disabled() {
        let r = one_reading(100.0, Trend::SingleDown);
        let p = glucose_payload(&r, false);
        assert!(p.patient.is_none());
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("patient"), "patient must be absent: {json}");
        assert!(json.contains(r#""trend":"SingleDown""#));
    }

    #[test]
    fn health_payload_round_trip() {
        let on = serde_json::to_string(&health_payload(true)).unwrap();
        let off = serde_json::to_string(&health_payload(false)).unwrap();
        assert_eq!(on, r#"{"online":true,"v":1}"#);
        assert_eq!(off, r#"{"online":false,"v":1}"#);
    }

    #[test]
    fn glucose_payload_serialises_to_compact_json() {
        let r = one_reading(180.0, Trend::DoubleUp);
        let p = glucose_payload(&r, true);
        let json = serde_json::to_string(&p).unwrap();
        // Schema-stable assertions — DO NOT rewrite without bumping v.
        assert!(json.starts_with(r#"{"v":1,"ts":"#));
        assert!(json.contains(r#""mgdl":180.0"#));
        assert!(json.contains(r#""trend":"DoubleUp""#));
        assert!(json.contains(r#""source":"llu""#));
        assert!(json.contains(r#""patient":"p1""#));
    }
}
