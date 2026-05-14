// SPDX-License-Identifier: AGPL-3.0-or-later

//! Home Assistant MQTT-discovery schema validation.
//!
//! Replicates the field-level checks HA's `mqtt-discovery` integration
//! performs on a config payload, without booting a full HA container.
//! Sources:
//!  * <https://www.home-assistant.io/integrations/mqtt/#discovery-messages>
//!  * <https://www.home-assistant.io/integrations/sensor.mqtt/>
//!
//! Coverage: the structural and field-presence requirements. We do NOT
//! attempt to validate that HA would render the entity meaningfully —
//! that's a HA-internal concern outside our control.

use serde_json::Value;

/// Outcome of a discovery-schema check. `Ok(())` = passes. `Err(msg)`
/// carries a human-readable reason that the test can `unwrap` to
/// surface in the failure trace.
pub type SchemaResult = Result<(), String>;

/// Validate a `homeassistant/sensor/.../config` payload against HA's
/// documented requirements.
pub fn validate_sensor_discovery(body: &Value) -> SchemaResult {
    require_object(body)?;

    // HA requires *at least one of* `unique_id`, `object_id`.
    if !body.get("unique_id").is_some_and(Value::is_string)
        && !body.get("object_id").is_some_and(Value::is_string)
    {
        return Err("discovery payload missing both `unique_id` and `object_id`".into());
    }

    require_str(body, "state_topic")?;

    if let Some(name) = body.get("name")
        && !(name.is_string() || name.is_null())
    {
        return Err(format!("`name` must be string or null, got {name:?}"));
    }

    let device = body
        .get("device")
        .ok_or("missing `device` block (required for entity grouping)")?;
    require_object(device)?;
    let has_identifier = matches!(device.get("identifiers"), Some(v) if v.is_string() || v.is_array())
        || matches!(device.get("connections"), Some(v) if v.is_array());
    if !has_identifier {
        return Err("`device` must carry at least one of `identifiers` or `connections`".into());
    }

    if let Some(tmpl) = body.get("value_template") {
        let s = tmpl.as_str().ok_or("`value_template` must be a string")?;
        if s.trim().is_empty() {
            return Err("`value_template` must not be empty".into());
        }
    }

    if let Some(sc) = body.get("state_class") {
        let s = sc.as_str().ok_or("`state_class` must be a string")?;
        if !["measurement", "total", "total_increasing"].contains(&s) {
            return Err(format!(
                "`state_class` = {s:?} not in {{measurement, total, total_increasing}}"
            ));
        }
    }

    if body.get("availability_template").is_some() {
        require_str(body, "payload_available")?;
        require_str(body, "payload_not_available")?;
    }

    Ok(())
}

fn require_object(v: &Value) -> SchemaResult {
    if v.is_object() {
        Ok(())
    } else {
        Err(format!("expected JSON object, got {v:?}"))
    }
}

fn require_str(body: &Value, field: &str) -> SchemaResult {
    let v = body
        .get(field)
        .ok_or_else(|| format!("missing required field `{field}`"))?;
    if !v.is_string() {
        return Err(format!("`{field}` must be a string, got {v:?}"));
    }
    if v.as_str().unwrap().is_empty() {
        return Err(format!("`{field}` must not be empty"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn valid_payload_passes() {
        let body = json!({
            "name": "Glucose",
            "unique_id": "gluco_hub_test_glucose",
            "state_topic": "gluco-hub/test/glucose",
            "value_template": "{{ value_json.mgdl }}",
            "state_class": "measurement",
            "availability_template": "{{ 'online' if value_json.online else 'offline' }}",
            "payload_available": "online",
            "payload_not_available": "offline",
            "device": { "identifiers": ["gluco_hub_test"], "name": "Gluco Hub" }
        });
        validate_sensor_discovery(&body).expect("must validate");
    }

    #[test]
    fn missing_unique_and_object_id_rejected() {
        let body = json!({
            "state_topic": "x",
            "device": { "identifiers": ["x"] }
        });
        assert!(validate_sensor_discovery(&body).is_err());
    }

    #[test]
    fn missing_state_topic_rejected() {
        let body = json!({
            "unique_id": "x",
            "device": { "identifiers": ["x"] }
        });
        assert!(validate_sensor_discovery(&body).is_err());
    }

    #[test]
    fn missing_device_block_rejected() {
        let body = json!({ "unique_id": "x", "state_topic": "y" });
        assert!(validate_sensor_discovery(&body).is_err());
    }

    #[test]
    fn invalid_state_class_rejected() {
        let body = json!({
            "unique_id": "x",
            "state_topic": "y",
            "state_class": "wrong",
            "device": { "identifiers": ["x"] }
        });
        assert!(validate_sensor_discovery(&body).is_err());
    }

    #[test]
    fn availability_template_without_payloads_rejected() {
        let body = json!({
            "unique_id": "x",
            "state_topic": "y",
            "availability_template": "{{ value }}",
            "device": { "identifiers": ["x"] }
        });
        assert!(validate_sensor_discovery(&body).is_err());
    }
}
