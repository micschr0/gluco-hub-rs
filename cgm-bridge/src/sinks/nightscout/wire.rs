//! Nightscout v3 `entries` wire format.
//!
//! Field set verified against the public Nightscout v3 API and CGM
//! conventions: each entry carries a millisecond Unix timestamp, the
//! glucose value in mg/dL (`sgv`), a textual `direction`, the trend
//! integer using the Nightscout numbering (1=DoubleUp … 7=DoubleDown),
//! and a fixed `type: "sgv"`.

use cgm_bridge_core::{Reading, Trend};
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

/// Nightscout's trend integer scheme (different from LLU's). Encoded
/// alongside the textual `direction` for downstream tooling that prefers
/// numeric input.
fn ns_trend_int(t: Trend) -> u8 {
    match t {
        Trend::DoubleUp => 1,
        Trend::SingleUp => 2,
        Trend::FortyFiveUp => 3,
        Trend::Flat => 4,
        Trend::FortyFiveDown => 5,
        Trend::SingleDown => 6,
        Trend::DoubleDown => 7,
        // NS treats unknown trends as 0 / NOT COMPUTABLE.
        Trend::NotComputable | Trend::RateOutOfRange => 0,
    }
}

/// One Nightscout v3 entry. The `type` field must be a literal `"sgv"`
/// for sensor-glucose-value entries.
#[derive(Debug, Clone, Serialize)]
pub struct NsEntry {
    /// Milliseconds since the Unix epoch.
    pub date: i64,
    /// Sensor glucose value in mg/dL (rounded to integer per NS convention).
    pub sgv: i64,
    pub direction: NsDirection,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub trend: u8,
}

/// Build a Nightscout entry from a normalised `Reading`. Glucose is
/// rounded half-away-from-zero; out-of-range values upstream are already
/// rejected by `GlucoseMgDl::new`.
pub fn entry_from_reading(r: &Reading) -> NsEntry {
    NsEntry {
        date: r.timestamp.timestamp_millis(),
        sgv: r.glucose.get().round() as i64,
        direction: NsDirection::from(r.trend),
        kind: "sgv",
        trend: ns_trend_int(r.trend),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cgm_bridge_core::{GlucoseMgDl, PatientId, Reading, SourceId};
    use chrono::{TimeZone, Utc};

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
    fn ns_trend_int_table() {
        assert_eq!(ns_trend_int(Trend::DoubleUp), 1);
        assert_eq!(ns_trend_int(Trend::SingleUp), 2);
        assert_eq!(ns_trend_int(Trend::FortyFiveUp), 3);
        assert_eq!(ns_trend_int(Trend::Flat), 4);
        assert_eq!(ns_trend_int(Trend::FortyFiveDown), 5);
        assert_eq!(ns_trend_int(Trend::SingleDown), 6);
        assert_eq!(ns_trend_int(Trend::DoubleDown), 7);
        assert_eq!(ns_trend_int(Trend::NotComputable), 0);
        assert_eq!(ns_trend_int(Trend::RateOutOfRange), 0);
    }

    #[test]
    fn entry_rounds_glucose_and_uses_ms_precision() {
        let entry = entry_from_reading(&reading(141.6, Trend::Flat));
        assert_eq!(entry.sgv, 142);
        assert_eq!(entry.date, 1_700_000_000_000);
        assert_eq!(entry.kind, "sgv");
        assert_eq!(entry.direction, NsDirection::Flat);
        assert_eq!(entry.trend, 4);
    }

    #[test]
    fn entry_serializes_with_type_field_as_sgv() {
        let entry = entry_from_reading(&reading(120.0, Trend::SingleUp));
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&entry).unwrap()).unwrap();
        assert_eq!(json["type"], "sgv");
        assert_eq!(json["sgv"], 120);
        assert_eq!(json["direction"], "SingleUp");
        assert_eq!(json["trend"], 2);
    }
}
