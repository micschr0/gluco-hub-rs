// SPDX-License-Identifier: AGPL-3.0-or-later

//! Wire-format types for LibreLink Up `/llu/connections` and
//! `/llu/connections/:id/graph`.
//!
//! LLU mixes camelCase (envelope/connection) with PascalCase (measurement
//! payload) — every field is renamed explicitly so the Rust field names
//! stay snake_case-idiomatic regardless of upstream casing changes.
//!
//! Fields the bridge does not consume are intentionally absent from these
//! structs; serde drops them.

use serde::Deserialize;

/// Top-level `/llu/connections` response.
#[derive(Debug, Clone, Deserialize)]
pub struct ConnectionsResponse {
    pub status: i64,
    pub data: Vec<Connection>,
}

/// Top-level `/llu/connections/:id/graph` response.
#[derive(Debug, Clone, Deserialize)]
pub struct GraphResponse {
    pub status: i64,
    pub data: GraphData,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphData {
    /// Stream of historical measurements; LLU returns ~24 h of points.
    #[serde(rename = "graphData")]
    pub graph_data: Vec<GlucoseMeasurement>,
}

/// One LibreLink-Up "connection" (a patient-link visible to the account).
/// `glucoseMeasurement` carries the most recent ~1-minute-fresh reading
/// — the same value the LinkUp/LibreLink app surfaces on its main
/// screen. Critically newer than the freshest entry inside the `graph`
/// endpoint's `graphData`, which is rastered at ~5-minute intervals.
/// Both are merged downstream by `LluSource::fetch_latest` so the cache
/// gets the freshest reading the cloud has, not just the historical
/// 5-min raster.
#[derive(Debug, Clone, Deserialize)]
pub struct Connection {
    #[serde(rename = "patientId")]
    pub patient_id: String,

    #[serde(rename = "glucoseMeasurement", default)]
    pub glucose_measurement: Option<GlucoseMeasurement>,
}

/// Glucose sample shape used both for the singular `glucoseMeasurement`
/// and entries inside `graphData`. The LLU JSON uses PascalCase here.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GlucoseMeasurement {
    /// Raw timestamp in LLU's `M/D/YYYY h:mm:ss AM/PM` format, UTC.
    pub timestamp: String,
    /// Glucose value in mg/dL.
    #[serde(rename = "ValueInMgPerDl")]
    pub value_in_mg_per_dl: f64,
    /// Integer 1..=5 (see [`crate::sources::llu::mapping::trend_from_llu`]).
    /// Optional because graph entries (`GlucoseItem`) sometimes omit it.
    pub trend_arrow: Option<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_connections_response_drops_unused_fields() {
        // The bridge only consumes `patientId`. Everything else (id,
        // firstName, lastName, glucoseMeasurement, …) must round-trip
        // through serde without breaking the parse — that's the point
        // of explicit field-by-field renames instead of `deny_unknown_fields`.
        let raw = r#"{
            "status": 0,
            "data": [{
                "id": "abc-123",
                "patientId": "patient-1",
                "firstName": "Ignored",
                "lastName": "Ignored",
                "glucoseMeasurement": {
                    "Timestamp": "3/26/2024 4:38:38 PM",
                    "ValueInMgPerDl": 142.0,
                    "TrendArrow": 3
                },
                "extraField": "ignored"
            }]
        }"#;
        let parsed: ConnectionsResponse = serde_json::from_str(raw).expect("parse");
        assert_eq!(parsed.status, 0);
        assert_eq!(parsed.data.len(), 1);
        assert_eq!(parsed.data[0].patient_id, "patient-1");
    }

    #[test]
    fn parses_graph_response() {
        let raw = r#"{
            "status": 0,
            "data": {
                "connection": {
                    "id": "abc",
                    "patientId": "p1"
                },
                "activeSensors": [],
                "graphData": [
                    {
                        "Timestamp": "3/26/2024 4:33:38 PM",
                        "ValueInMgPerDl": 138.0,
                        "TrendArrow": 3
                    },
                    {
                        "Timestamp": "3/26/2024 4:38:38 PM",
                        "ValueInMgPerDl": 142.0
                    }
                ]
            }
        }"#;
        let parsed: GraphResponse = serde_json::from_str(raw).expect("parse");
        assert_eq!(parsed.data.graph_data.len(), 2);
        assert_eq!(parsed.data.graph_data[0].trend_arrow, Some(3));
        assert!(parsed.data.graph_data[1].trend_arrow.is_none());
    }
}
