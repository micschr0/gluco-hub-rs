//! Pure conversions between LLU wire-format and core domain types.
//!
//! Trend integer mapping verified against
//! <https://github.com/timoschlueter/nightscout-librelink-up/blob/main/src/helpers/helpers.ts>
//! (`mapTrendArrow`): 1=SingleDown, 2=FortyFiveDown, 3=Flat,
//! 4=FortyFiveUp, 5=SingleUp; everything else → `NotComputable`.
//!
//! Timestamp choice — `Timestamp` vs `FactoryTimestamp`:
//! Each LLU `GlucoseItem` ships both fields. The reference port reads
//! `FactoryTimestamp` (the sensor's local time at the time of the
//! reading) and adjusts via JS `getTimezoneOffset`. We read `Timestamp`
//! (the receiver-side time) and treat it as UTC. In practice both fields
//! carry the same wall-clock value for a patient who scans on a phone in
//! their own timezone; if a deployment surfaces a measurable skew, swap
//! to `FactoryTimestamp` here — it is already deserialised at the wire
//! layer (see `super::wire::GlucoseMeasurement`).

use cgm_bridge_core::{GlucoseMgDl, PatientId, Reading, SourceId, Trend};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};

use super::error::LluError;
use super::wire::GlucoseMeasurement;

/// LLU `Timestamp` field format: month/day/year with a 12-hour AM/PM clock,
/// returned in UTC. Accepts both single- and zero-padded month/day/hour.
const LLU_TIMESTAMP_FORMAT: &str = "%m/%d/%Y %I:%M:%S %p";

/// Map an LLU `TrendArrow` integer to the bridge's `Trend` enum. Unknown
/// values fall back to `Trend::NotComputable` to mirror the reference
/// port's behaviour — LLU silently introduces values from time to time.
pub fn trend_from_llu(value: Option<u8>) -> Trend {
    match value {
        Some(1) => Trend::SingleDown,
        Some(2) => Trend::FortyFiveDown,
        Some(3) => Trend::Flat,
        Some(4) => Trend::FortyFiveUp,
        Some(5) => Trend::SingleUp,
        _ => Trend::NotComputable,
    }
}

/// Parse an LLU timestamp string into a UTC `DateTime`.
pub fn parse_llu_timestamp(raw: &str) -> Result<DateTime<Utc>, LluError> {
    let naive = NaiveDateTime::parse_from_str(raw, LLU_TIMESTAMP_FORMAT).map_err(|_| {
        LluError::BadTimestamp {
            raw: raw.to_string(),
        }
    })?;
    Ok(Utc.from_utc_datetime(&naive))
}

/// Build a normalised `Reading` from an LLU `GlucoseMeasurement` and the
/// owning patient/source identifiers.
///
/// Out-of-range glucose values (sensor errors / sentinel readings) are
/// rejected as `LluError::Protocol` rather than silently clamped — the
/// poller's error counter then carries the `error_code = "LLU004"` label.
pub fn reading_from_measurement(
    m: &GlucoseMeasurement,
    patient_id: &PatientId,
    source_id: &SourceId,
) -> Result<Reading, LluError> {
    let timestamp = parse_llu_timestamp(&m.timestamp)?;
    let glucose = GlucoseMgDl::new(m.value_in_mg_per_dl).map_err(|_| LluError::Protocol {
        reason: format!("glucose out of range: {}", m.value_in_mg_per_dl),
    })?;
    let trend = trend_from_llu(m.trend_arrow);
    Ok(Reading {
        patient_id: patient_id.clone(),
        source_id: source_id.clone(),
        timestamp,
        glucose,
        trend,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measurement(timestamp: &str, value: f64, trend: Option<u8>) -> GlucoseMeasurement {
        GlucoseMeasurement {
            timestamp: timestamp.to_string(),
            value_in_mg_per_dl: value,
            trend_arrow: trend,
        }
    }

    #[test]
    fn trend_known_values() {
        assert_eq!(trend_from_llu(Some(1)), Trend::SingleDown);
        assert_eq!(trend_from_llu(Some(2)), Trend::FortyFiveDown);
        assert_eq!(trend_from_llu(Some(3)), Trend::Flat);
        assert_eq!(trend_from_llu(Some(4)), Trend::FortyFiveUp);
        assert_eq!(trend_from_llu(Some(5)), Trend::SingleUp);
    }

    #[test]
    fn trend_unknown_falls_back() {
        assert_eq!(trend_from_llu(None), Trend::NotComputable);
        assert_eq!(trend_from_llu(Some(0)), Trend::NotComputable);
        assert_eq!(trend_from_llu(Some(7)), Trend::NotComputable);
        assert_eq!(trend_from_llu(Some(99)), Trend::NotComputable);
    }

    #[test]
    fn timestamp_parses_padded_and_unpadded() {
        let parsed = parse_llu_timestamp("03/26/2024 04:38:38 PM").unwrap();
        assert_eq!(parsed.to_rfc3339(), "2024-03-26T16:38:38+00:00");
        let parsed = parse_llu_timestamp("3/26/2024 4:38:38 PM").unwrap();
        assert_eq!(parsed.to_rfc3339(), "2024-03-26T16:38:38+00:00");
    }

    #[test]
    fn timestamp_rejects_malformed() {
        let err = parse_llu_timestamp("yesterday").unwrap_err();
        assert!(matches!(err, LluError::BadTimestamp { ref raw } if raw == "yesterday"));
        assert_eq!(err.error_code(), "LLU007");
    }

    #[test]
    fn reading_from_measurement_happy_path() {
        let p = PatientId::new("p1").unwrap();
        let s = SourceId::new("llu").unwrap();
        let m = measurement("3/26/2024 4:38:38 PM", 142.0, Some(3));
        let r = reading_from_measurement(&m, &p, &s).unwrap();
        assert_eq!(r.glucose.get(), 142.0);
        assert_eq!(r.trend, Trend::Flat);
        assert_eq!(r.patient_id.as_str(), "p1");
        assert_eq!(r.source_id.as_str(), "llu");
        assert_eq!(r.timestamp.to_rfc3339(), "2024-03-26T16:38:38+00:00");
    }

    #[test]
    fn reading_from_measurement_rejects_out_of_range_glucose() {
        let p = PatientId::new("p1").unwrap();
        let s = SourceId::new("llu").unwrap();
        let m = measurement("3/26/2024 4:38:38 PM", 9000.0, Some(3));
        let err = reading_from_measurement(&m, &p, &s).unwrap_err();
        assert!(matches!(err, LluError::Protocol { .. }));
        assert_eq!(err.error_code(), "LLU004");
    }

    #[test]
    fn reading_from_measurement_propagates_bad_timestamp() {
        let p = PatientId::new("p1").unwrap();
        let s = SourceId::new("llu").unwrap();
        let m = measurement("nope", 142.0, Some(3));
        let err = reading_from_measurement(&m, &p, &s).unwrap_err();
        assert!(matches!(err, LluError::BadTimestamp { .. }));
    }

    #[test]
    fn reading_handles_missing_trend_arrow() {
        let p = PatientId::new("p1").unwrap();
        let s = SourceId::new("llu").unwrap();
        let m = measurement("3/26/2024 4:38:38 PM", 142.0, None);
        let r = reading_from_measurement(&m, &p, &s).unwrap();
        assert_eq!(r.trend, Trend::NotComputable);
    }
}
