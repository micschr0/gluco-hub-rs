// SPDX-License-Identifier: AGPL-3.0-or-later

//! Nightscout v3 `entries` wire format.
//!
//! Field set verified against the reference port
//! (`timoschlueter/nightscout-librelink-up`, `src/nightscout/apiv3.ts`):
//!
//! ```ignore
//! const entryPayloads = entries.map((entry) => ({
//!   type: "sgv",
//!   sgv: entry.sgv,
//!   direction: entry.direction?.toString(),
//!   device: this.device,
//!   date: entry.date.getTime(),
//!   app: this.app,
//! }));
//! ```
//!
//! Notable choices:
//! - We do NOT send a `trend` integer field. The reference doesn't, and
//!   Nightscout dashboards key on `direction` (the textual form), so the
//!   integer would be redundant at best and wrong at worst across NS
//!   versions.
//! - `device` and `app` are caller-provided strings — they identify this
//!   service in the Nightscout UI. They round-trip through serde when set
//!   and are omitted from the JSON when `None`, matching NS's optional
//!   handling.

use gluco_hub_core::{Reading, Trend};
use serde::Serialize;

/// Nightscout direction strings. Most are PascalCase and identical to
/// the variant name; the two sentinel values use spaced upper-case
/// strings, which serde must produce verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum NsDirection {
    DoubleUp,
    SingleUp,
    FortyFiveUp,
    Flat,
    FortyFiveDown,
    SingleDown,
    DoubleDown,
    #[serde(rename = "NOT COMPUTABLE")]
    NotComputable,
    #[serde(rename = "RATE OUT OF RANGE")]
    RateOutOfRange,
}

impl From<Trend> for NsDirection {
    fn from(t: Trend) -> Self {
        match t {
            Trend::DoubleUp => NsDirection::DoubleUp,
            Trend::SingleUp => NsDirection::SingleUp,
            Trend::FortyFiveUp => NsDirection::FortyFiveUp,
            Trend::Flat => NsDirection::Flat,
            Trend::FortyFiveDown => NsDirection::FortyFiveDown,
            Trend::SingleDown => NsDirection::SingleDown,
            Trend::DoubleDown => NsDirection::DoubleDown,
            Trend::NotComputable => NsDirection::NotComputable,
            Trend::RateOutOfRange => NsDirection::RateOutOfRange,
        }
    }
}

/// Nightscout's trend integer scheme — kept as a private mapping for
/// future use, but NOT serialised. The reference port omits it; relying
/// on `direction` alone keeps us aligned with what NS dashboards expect.
#[allow(dead_code)]
fn ns_trend_int(t: Trend) -> u8 {
    match t {
        Trend::DoubleUp => 1,
        Trend::SingleUp => 2,
        Trend::FortyFiveUp => 3,
        Trend::Flat => 4,
        Trend::FortyFiveDown => 5,
        Trend::SingleDown => 6,
        Trend::DoubleDown => 7,
        Trend::NotComputable | Trend::RateOutOfRange => 0,
    }
}

/// One Nightscout v3 entry. The `type` field must be a literal `"sgv"`
/// for sensor-glucose-value entries.
///
/// `device` and `app` are operator-provided identifiers ("gluco-hub",
/// "gluco-hub-1", etc.) that show up in the NS UI's source column.
/// Both omit from the JSON when `None` — matches the reference, which
/// always sets them but only because its config has defaults.
#[derive(Debug, Clone, Serialize)]
pub struct NsEntry {
    /// Milliseconds since the Unix epoch.
    pub date: i64,
    /// Sensor glucose value in mg/dL (rounded to integer per NS convention).
    pub sgv: i64,
    pub direction: NsDirection,
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

/// Build a Nightscout entry from a normalised `Reading`. Glucose is
/// rounded half-away-from-zero; out-of-range values upstream are already
/// rejected by `GlucoseMgDl::new`.
pub fn entry_from_reading(r: &Reading, device: Option<&str>, app: Option<&str>) -> NsEntry {
    NsEntry {
        date: r.timestamp.timestamp_millis(),
        sgv: r.glucose.get().round() as i64,
        direction: NsDirection::from(r.trend),
        kind: "sgv",
        device: device.map(str::to_string),
        app: app.map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use gluco_hub_core::{GlucoseMgDl, PatientId, Reading, SourceId};

    fn reading(value: f64, trend: Trend) -> Reading {
        Reading {
            patient_id: PatientId::new("p1").unwrap(),
            source_id: SourceId::new("llu").unwrap(),
            timestamp: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            glucose: GlucoseMgDl::new(value).unwrap(),
            trend,
        }
    }

    #[test]
    fn directions_serialize_as_ns_strings() {
        let cases = [
            (NsDirection::DoubleUp, "\"DoubleUp\""),
            (NsDirection::SingleUp, "\"SingleUp\""),
            (NsDirection::FortyFiveUp, "\"FortyFiveUp\""),
            (NsDirection::Flat, "\"Flat\""),
            (NsDirection::FortyFiveDown, "\"FortyFiveDown\""),
            (NsDirection::SingleDown, "\"SingleDown\""),
            (NsDirection::DoubleDown, "\"DoubleDown\""),
            (NsDirection::NotComputable, "\"NOT COMPUTABLE\""),
            (NsDirection::RateOutOfRange, "\"RATE OUT OF RANGE\""),
        ];
        for (variant, expected) in cases {
            let got = serde_json::to_string(&variant).unwrap();
            assert_eq!(got, expected, "{variant:?}");
        }
    }

    #[test]
    fn trend_to_direction_table() {
        assert_eq!(NsDirection::from(Trend::DoubleUp), NsDirection::DoubleUp);
        assert_eq!(NsDirection::from(Trend::Flat), NsDirection::Flat);
        assert_eq!(
            NsDirection::from(Trend::NotComputable),
            NsDirection::NotComputable
        );
        assert_eq!(
            NsDirection::from(Trend::RateOutOfRange),
            NsDirection::RateOutOfRange
        );
    }

    #[test]
    fn entry_rounds_glucose_and_uses_ms_precision() {
        let entry = entry_from_reading(&reading(141.6, Trend::Flat), None, None);
        assert_eq!(entry.sgv, 142);
        assert_eq!(entry.date, 1_700_000_000_000);
        assert_eq!(entry.kind, "sgv");
        assert_eq!(entry.direction, NsDirection::Flat);
        assert!(entry.device.is_none());
        assert!(entry.app.is_none());
    }

    #[test]
    fn entry_serializes_with_type_field_as_sgv_and_no_trend_int() {
        let entry = entry_from_reading(
            &reading(120.0, Trend::SingleUp),
            Some("cgm-bridge"),
            Some("cgm-bridge"),
        );
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&entry).unwrap()).unwrap();
        assert_eq!(json["type"], "sgv");
        assert_eq!(json["sgv"], 120);
        assert_eq!(json["direction"], "SingleUp");
        assert_eq!(json["device"], "cgm-bridge");
        assert_eq!(json["app"], "cgm-bridge");
        // The reference port does not send a numeric `trend` — neither do we.
        assert!(json.get("trend").is_none());
    }

    #[test]
    fn entry_omits_device_and_app_when_absent() {
        let entry = entry_from_reading(&reading(120.0, Trend::Flat), None, None);
        let s = serde_json::to_string(&entry).unwrap();
        assert!(!s.contains("device"), "got: {s}");
        assert!(!s.contains("app"), "got: {s}");
    }
}
