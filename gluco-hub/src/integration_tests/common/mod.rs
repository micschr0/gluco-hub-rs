// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared helpers for integration tests.

pub mod ha_schema;

#[cfg(feature = "sink-mqtt")]
pub mod mosquitto;

#[cfg(feature = "sink-nightscout")]
pub mod nightscout_container;

use chrono::{DateTime, TimeZone, Utc};
use gluco_hub_core::{GlucoseMgDl, PatientId, Reading, SourceId, Trend};

/// Build a `Reading` with deterministic patient/source ids and a
/// caller-supplied timestamp/value. Tests use unique timestamps to
/// avoid colliding on shared backing services (Nightscout dedups by
/// `(deviceId, date)`; the Mosquitto retained-config topic is keyed
/// by per-test `client_id`).
pub fn reading(ts_secs: i64, mgdl: f64) -> Reading {
    Reading {
        patient_id: PatientId::new("itest-patient").expect("patient id"),
        source_id: SourceId::new("itest").expect("source id"),
        timestamp: Utc.timestamp_opt(ts_secs, 0).single().expect("ts"),
        glucose: GlucoseMgDl::new(mgdl).expect("glucose"),
        trend: Trend::Flat,
    }
}

/// Compute a timestamp `secs_ago` seconds before the current UTC time.
pub fn recent_ts(secs_ago: i64) -> DateTime<Utc> {
    Utc::now() - chrono::Duration::seconds(secs_ago)
}

/// Generate a fresh unique id for a test run (uuid v4). Used as MQTT
/// `client_id` and as the Nightscout `deviceId` so concurrent or
/// retried test runs do not collide on the shared backing service.
pub fn unique_id(prefix: &str) -> String {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    // MQTT client_id is constrained to 23 chars (validated upstream in
    // config.rs). Truncate the uuid hex to keep the total ≤ 23.
    let slug: String = raw
        .chars()
        .take(23usize.saturating_sub(prefix.len() + 1))
        .collect();
    format!("{prefix}-{slug}")
}
