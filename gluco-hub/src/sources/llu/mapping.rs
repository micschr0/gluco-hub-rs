// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pure conversions between LLU wire-format and core domain types.
//!
//! Trend integer mapping: 1=SingleDown, 2=FortyFiveDown, 3=Flat,
//! 4=FortyFiveUp, 5=SingleUp; everything else → `NotComputable`.
//!
//! Timestamp handling — LLU `Timestamp` is **local wall-clock time**
//! of the patient's phone, NOT UTC. Empirical observation against
//! `api-de.libreview.io` for a CEST patient: timestamps come back as
//! `5/9/2026 2:43:50 AM` while wall-clock UTC was `00:43:50Z` — a clean
//! +2h offset matching CEST. The earlier assumption (and the JSON
//! comment claiming UTC) was wrong; LLU clearly returns the value the
//! phone displays.
//!
//! To convert correctly we need the patient's IANA timezone, which the
//! API itself does NOT include. The bridge therefore takes it from
//! configuration (`[source.llu] timezone = "Europe/Berlin"`); default
//! `UTC` preserves prior behaviour for deployments that already lived
//! with the offset or run in a UTC patient.
//!
//! `FactoryTimestamp` (sensor RTC) is also local and would need the same
//! treatment; staying with `Timestamp` keeps the wire surface minimal.

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use gluco_hub_core::{GlucoseMgDl, PatientId, Reading, SourceId, Trend};

use super::error::LluError;
use super::wire::GlucoseMeasurement;

/// LLU `Timestamp` field format: month/day/year with a 12-hour AM/PM clock
/// in the patient's local wall-clock. Accepts both single- and
/// zero-padded month/day/hour.
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

/// Parse an LLU timestamp string into a UTC `DateTime`. `source_tz` is
/// the patient's local IANA timezone (e.g. `Europe/Berlin`); the parsed
/// naive value is interpreted as wall-clock-in-`source_tz` and converted
/// to UTC. Ambiguous local times during a DST fall-back are resolved to
/// the earlier instant; non-existent local times (DST spring-forward gap)
/// are rejected as `BadTimestamp` — both cases are degenerate for a
/// glucose stream and an explicit error is better than silent coercion.
pub fn parse_llu_timestamp(raw: &str, source_tz: Tz) -> Result<DateTime<Utc>, LluError> {
    let naive = NaiveDateTime::parse_from_str(raw, LLU_TIMESTAMP_FORMAT).map_err(|_| {
        LluError::BadTimestamp {
            raw: raw.to_string(),
        }
    })?;
    let local = source_tz
        .from_local_datetime(&naive)
        .earliest()
        .ok_or_else(|| LluError::BadTimestamp {
            raw: raw.to_string(),
        })?;
    Ok(local.with_timezone(&Utc))
}

/// Build a normalised `Reading` from an LLU `GlucoseMeasurement` and the
/// owning patient/source identifiers.
///
/// Out-of-range glucose values (sensor errors / sentinel readings) are
/// rejected as `LluError::Protocol` rather than silently clamped — the
/// poller's error counter then carries the `error_code = "LLU004"` label.
/// Pick the measurement with the newest parseable timestamp from a
/// graph-data slice. Returns `None` when the slice is empty or every
/// timestamp fails to parse. Centralised here so callers (the dryrun
/// probe, future TUI views) don't reach into [`parse_llu_timestamp`]
/// directly.
pub fn newest_measurement(
    graph_data: &[GlucoseMeasurement],
    source_tz: Tz,
) -> Option<(DateTime<Utc>, &GlucoseMeasurement)> {
    graph_data
        .iter()
        .filter_map(|m| {
            parse_llu_timestamp(&m.timestamp, source_tz)
                .ok()
                .map(|t| (t, m))
        })
        .max_by_key(|(t, _)| *t)
}

pub fn reading_from_measurement(
    m: &GlucoseMeasurement,
    patient_id: &PatientId,
    source_id: &SourceId,
    source_tz: Tz,
) -> Result<Reading, LluError> {
    let timestamp = parse_llu_timestamp(&m.timestamp, source_tz)?;
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
    fn timestamp_parses_padded_and_unpadded_in_utc() {
        let parsed = parse_llu_timestamp("03/26/2024 04:38:38 PM", Tz::UTC).unwrap();
        assert_eq!(parsed.to_rfc3339(), "2024-03-26T16:38:38+00:00");
        let parsed = parse_llu_timestamp("3/26/2024 4:38:38 PM", Tz::UTC).unwrap();
        assert_eq!(parsed.to_rfc3339(), "2024-03-26T16:38:38+00:00");
    }

    /// CEST is +02:00 in late March. A wall-clock of `4:38:38 PM` in
    /// Berlin maps to `14:38:38Z` after conversion. Pinning the date to
    /// 2024-03-26 (after the 2024 DST transition on 2024-03-31)... wait,
    /// 03-26 is *before* spring-forward, so we're in CET (+01:00) →
    /// `15:38:38Z`. Tests that — DST transitions are exactly the bug
    /// chrono-tz exists to avoid getting wrong by hand.
    #[test]
    fn timestamp_converts_berlin_local_to_utc_during_cet() {
        let parsed = parse_llu_timestamp("3/26/2024 4:38:38 PM", Tz::Europe__Berlin).unwrap();
        assert_eq!(parsed.to_rfc3339(), "2024-03-26T15:38:38+00:00");
    }

    #[test]
    fn timestamp_converts_berlin_local_to_utc_during_cest() {
        // 2024-04-01 is past the spring-forward; Berlin is CEST = +02:00.
        let parsed = parse_llu_timestamp("4/1/2024 4:38:38 PM", Tz::Europe__Berlin).unwrap();
        assert_eq!(parsed.to_rfc3339(), "2024-04-01T14:38:38+00:00");
    }

    #[test]
    fn timestamp_rejects_malformed() {
        let err = parse_llu_timestamp("yesterday", Tz::UTC).unwrap_err();
        assert!(matches!(err, LluError::BadTimestamp { ref raw } if raw == "yesterday"));
        assert_eq!(err.error_code(), "LLU007");
    }

    #[test]
    fn reading_from_measurement_happy_path() {
        let p = PatientId::new("p1").unwrap();
        let s = SourceId::new("llu").unwrap();
        let m = measurement("3/26/2024 4:38:38 PM", 142.0, Some(3));
        let r = reading_from_measurement(&m, &p, &s, Tz::UTC).unwrap();
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
        let err = reading_from_measurement(&m, &p, &s, Tz::UTC).unwrap_err();
        assert!(matches!(err, LluError::Protocol { .. }));
        assert_eq!(err.error_code(), "LLU004");
    }

    #[test]
    fn reading_from_measurement_propagates_bad_timestamp() {
        let p = PatientId::new("p1").unwrap();
        let s = SourceId::new("llu").unwrap();
        let m = measurement("nope", 142.0, Some(3));
        let err = reading_from_measurement(&m, &p, &s, Tz::UTC).unwrap_err();
        assert!(matches!(err, LluError::BadTimestamp { .. }));
    }

    #[test]
    fn reading_handles_missing_trend_arrow() {
        let p = PatientId::new("p1").unwrap();
        let s = SourceId::new("llu").unwrap();
        let m = measurement("3/26/2024 4:38:38 PM", 142.0, None);
        let r = reading_from_measurement(&m, &p, &s, Tz::UTC).unwrap();
        assert_eq!(r.trend, Trend::NotComputable);
    }
}
