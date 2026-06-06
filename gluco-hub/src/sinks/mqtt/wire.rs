// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stable on-the-wire payload shapes for the MQTT sink.
//!
//! Schema is versioned via the `v` field. Bumping `v` is a breaking
//! change for downstream subscribers and must coincide with a
//! documented migration path in `docs/ARCHITECTURE.md`.

use gluco_hub_core::Reading;
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

/// PHI-safe patient record published retained to `<prefix>/_patients`.
///
/// `display_name` is abbreviated to "First L." (first name + first
/// initial of the last name followed by a dot) so no full surname is
/// ever emitted on the broker. The format is stable — consumers should
/// treat it as opaque display text, not as a parseable identifier.
#[derive(Debug, Serialize, PartialEq, Clone)]
pub struct PatientSummary {
    pub id: String,
    pub display_name: String,
    pub is_active: bool,
}

impl PatientSummary {
    /// Build a `PatientSummary` from raw LLU name parts.
    ///
    /// PHI abbreviation rule: `display_name = "First L."` where `L` is
    /// `last_name[..1]`. When either component is absent or empty the
    /// corresponding part is omitted gracefully:
    /// - no first name → last initial only (e.g. `"S."`)
    /// - no last name  → first name only  (e.g. `"Alice"`)
    /// - both absent   → fallback to `id`
    pub fn new(
        id: impl Into<String>,
        first_name: Option<&str>,
        last_name: Option<&str>,
        is_active: bool,
    ) -> Self {
        let id = id.into();
        let display_name = build_display_name(first_name, last_name, &id);
        Self {
            id,
            display_name,
            is_active,
        }
    }
}

/// Abbreviate name parts into a PHI-safe display string.
///
/// Format: `"{first} {L}."` where `{L}` is the first Unicode scalar
/// value (char) of `last_name`.
pub(crate) fn build_display_name(
    first_name: Option<&str>,
    last_name: Option<&str>,
    fallback_id: &str,
) -> String {
    let first = first_name.and_then(|s| {
        let t = s.trim();
        if t.is_empty() { None } else { Some(t) }
    });
    let last_initial: Option<String> = last_name.and_then(|s| {
        let t = s.trim();
        t.chars().next().map(|c| format!("{c}."))
    });
    match (first, last_initial) {
        (Some(f), Some(li)) => format!("{f} {li}"),
        (Some(f), None) => f.to_string(),
        (None, Some(li)) => li,
        (None, None) => fallback_id.to_string(),
    }
}

/// `v: 1` stats payload published retained to `<prefix>/_stats`.
///
/// All counters are monotonic over the lifetime of the sink process.
/// `*_ts_ms` are unix-epoch milliseconds, `None` until the first
/// successful event. Operators chart `publishes_total` for throughput
/// and `connects_total - 1` as the reconnect count.
#[derive(Debug, Serialize, PartialEq)]
pub struct StatsPayload {
    pub v: u8,
    pub uptime_secs: u64,
    pub publishes_total: u64,
    pub publish_errors_total: u64,
    pub connects_total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_publish_ts_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_connect_ts_ms: Option<i64>,
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

/// Build a `v: 1` stats payload from a snapshot of the live counters.
pub fn stats_payload(snapshot: &StatsSnapshot) -> StatsPayload {
    StatsPayload {
        v: 1,
        uptime_secs: snapshot.uptime_secs,
        publishes_total: snapshot.publishes_total,
        publish_errors_total: snapshot.publish_errors_total,
        connects_total: snapshot.connects_total,
        last_publish_ts_ms: snapshot.last_publish_ts_ms,
        last_connect_ts_ms: snapshot.last_connect_ts_ms,
    }
}

/// Plain-data view over [`super::stats::MqttStatsState`]. Decouples
/// the wire layer from the locking concerns of the live state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatsSnapshot {
    pub uptime_secs: u64,
    pub publishes_total: u64,
    pub publish_errors_total: u64,
    pub connects_total: u64,
    pub last_publish_ts_ms: Option<i64>,
    pub last_connect_ts_ms: Option<i64>,
}

/// Stable wire-form of the Trend enum. PascalCase matches the JSON
/// representation already produced by serde on `Reading` itself.
/// `pub(crate)` so `super::discovery` can use it to validate that its
/// `TREND_OPTIONS` enum list covers every wire variant.
pub(crate) fn trend_to_str(t: gluco_hub_core::Trend) -> &'static str {
    use gluco_hub_core::Trend;
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
    use chrono::{TimeZone, Utc};
    use gluco_hub_core::{GlucoseMgDl, PatientId, Reading, SourceId, Trend};

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
    fn stats_payload_v1_includes_all_counters() {
        let snap = StatsSnapshot {
            uptime_secs: 42,
            publishes_total: 10,
            publish_errors_total: 1,
            connects_total: 2,
            last_publish_ts_ms: Some(1_750_000_000_000),
            last_connect_ts_ms: Some(1_749_999_900_000),
        };
        let p = stats_payload(&snap);
        assert_eq!(p.v, 1);
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains(r#""v":1"#));
        assert!(json.contains(r#""uptime_secs":42"#));
        assert!(json.contains(r#""publishes_total":10"#));
        assert!(json.contains(r#""publish_errors_total":1"#));
        assert!(json.contains(r#""connects_total":2"#));
        assert!(json.contains(r#""last_publish_ts_ms":1750000000000"#));
        assert!(json.contains(r#""last_connect_ts_ms":1749999900000"#));
    }

    #[test]
    fn stats_payload_omits_unset_timestamps() {
        let snap = StatsSnapshot {
            uptime_secs: 0,
            publishes_total: 0,
            publish_errors_total: 0,
            connects_total: 0,
            last_publish_ts_ms: None,
            last_connect_ts_ms: None,
        };
        let json = serde_json::to_string(&stats_payload(&snap)).unwrap();
        assert!(
            !json.contains("last_publish_ts_ms"),
            "last_publish_ts_ms must be absent: {json}"
        );
        assert!(
            !json.contains("last_connect_ts_ms"),
            "last_connect_ts_ms must be absent: {json}"
        );
    }

    #[test]
    fn patient_summary_display_name_abbreviates_surname() {
        // Spec example: first="Anna", last="Müller" → "Anna M.".
        let s = PatientSummary::new("uuid-1", Some("Anna"), Some("Müller"), true);
        assert_eq!(s.display_name, "Anna M.");
        assert_eq!(s.id, "uuid-1");
        assert!(s.is_active);
    }

    #[test]
    fn patient_summary_serialises_to_phi_safe_json() {
        let s = PatientSummary::new("uuid-1", Some("Anna"), Some("Müller"), true);
        let json = serde_json::to_string(&s).unwrap();
        // Exact field set: id, display_name, is_active — nothing else.
        assert_eq!(
            json,
            r#"{"id":"uuid-1","display_name":"Anna M.","is_active":true}"#
        );
        // PHI guard: the full surname must never appear on the wire.
        assert!(
            !json.contains("Müller"),
            "full surname must not be serialised: {json}"
        );
    }

    #[test]
    fn patient_summary_array_serialises_as_json_array() {
        let arr = vec![
            PatientSummary::new("a", Some("Anna"), Some("Müller"), true),
            PatientSummary::new("b", Some("Ben"), Some("Smith"), false),
        ];
        let json = serde_json::to_string(&arr).unwrap();
        assert!(json.starts_with('['), "must be a JSON array: {json}");
        assert!(json.contains(r#""display_name":"Anna M.""#));
        assert!(json.contains(r#""display_name":"Ben S.""#));
        assert!(json.contains(r#""is_active":true"#));
        assert!(json.contains(r#""is_active":false"#));
    }

    #[test]
    fn build_display_name_handles_missing_or_empty_parts() {
        // Empty last name must NOT panic (`&last_name[..1]` would) — it
        // gracefully degrades to the first name only.
        assert_eq!(build_display_name(Some("Anna"), Some(""), "id"), "Anna");
        // Whitespace-only is treated as empty.
        assert_eq!(build_display_name(Some("Anna"), Some("   "), "id"), "Anna");
        // No first name → last initial only.
        assert_eq!(build_display_name(None, Some("Smith"), "id"), "S.");
        assert_eq!(build_display_name(Some(""), Some("Smith"), "id"), "S.");
        // No last name → first name only.
        assert_eq!(build_display_name(Some("Anna"), None, "id"), "Anna");
        // Neither present → fall back to the id so the entry is still
        // identifiable.
        assert_eq!(
            build_display_name(None, None, "uuid-fallback"),
            "uuid-fallback"
        );
        assert_eq!(build_display_name(Some(""), Some(""), "uuid-x"), "uuid-x");
    }

    #[test]
    fn build_display_name_uses_first_unicode_scalar_for_initial() {
        // A multibyte surname initial must use the first char, never a
        // byte slice (`[..1]` would split mid-codepoint and panic).
        assert_eq!(
            build_display_name(Some("Émile"), Some("Über"), "id"),
            "Émile Ü."
        );
    }

    #[test]
    fn patient_summary_new_with_empty_last_name_does_not_panic() {
        // Direct guard against the `&last_name[..1]` panic the spec calls out.
        let s = PatientSummary::new("uuid-9", Some("Solo"), Some(""), false);
        assert_eq!(s.display_name, "Solo");
        assert_eq!(s.id, "uuid-9");
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
