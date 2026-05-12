// SPDX-License-Identifier: AGPL-3.0-or-later

//! Home Assistant MQTT auto-discovery (V3).
//!
//! When enabled, the MQTT sink publishes a retained config message on
//! `<discovery_prefix>/sensor/<unique_id>/config` after each ConnAck.
//! HA picks the entity up automatically and reads state from the
//! existing `<topic_prefix>/glucose` topic.
//!
//! Schema follows the HA MQTT discovery format
//! (<https://www.home-assistant.io/integrations/mqtt/#mqtt-discovery>):
//! one `sensor` entity per gluco-hub instance, with `mgdl` as the
//! state value and the full JSON body exposed as entity attributes.
//! Availability tracks the `<topic_prefix>/_health` retained topic
//! via the boolean `online` field.

use serde::Serialize;

use crate::config::MqttSinkConfig;

/// Discovery message published once per ConnAck (retained, QoS 1).
/// Field names match HA's [MQTT sensor discovery schema][1] verbatim;
/// extending requires a doc update in `docs/ARCHITECTURE.md`.
///
/// [1]: https://www.home-assistant.io/integrations/sensor.mqtt/
#[derive(Debug, Serialize, PartialEq)]
pub struct DiscoveryPayload<'a> {
    pub name: &'static str,
    pub unique_id: String,
    pub state_topic: String,
    pub value_template: &'static str,
    pub unit_of_measurement: &'static str,
    pub state_class: &'static str,
    pub icon: &'static str,
    pub availability_topic: String,
    pub availability_template: &'static str,
    pub payload_available: &'static str,
    pub payload_not_available: &'static str,
    pub json_attributes_topic: String,
    pub device: Device<'a>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Device<'a> {
    pub identifiers: [String; 1],
    pub name: String,
    pub manufacturer: &'static str,
    pub model: &'static str,
    pub sw_version: &'a str,
}

/// Topic that HA listens to for the discovery message. Conventional
/// shape is `<discovery_prefix>/sensor/<unique_id>/config`.
pub fn discovery_topic(cfg: &MqttSinkConfig) -> String {
    format!("{}/sensor/{}/config", cfg.discovery_prefix, unique_id(cfg),)
}

/// Build the JSON config payload. `sw_version` is the binary's
/// `CARGO_PKG_VERSION` at compile time so HA shows which release
/// emitted the entity.
pub fn build_discovery_payload(cfg: &MqttSinkConfig) -> DiscoveryPayload<'_> {
    let device_identifier = format!("gluco_hub_{}", cfg.client_id);
    DiscoveryPayload {
        name: "Glucose",
        unique_id: format!("{device_identifier}_glucose"),
        state_topic: format!("{}/glucose", cfg.topic_prefix),
        value_template: "{{ value_json.mgdl }}",
        unit_of_measurement: "mg/dL",
        state_class: "measurement",
        icon: "mdi:water-percent",
        availability_topic: format!("{}/_health", cfg.topic_prefix),
        availability_template: "{{ 'online' if value_json.online else 'offline' }}",
        payload_available: "online",
        payload_not_available: "offline",
        json_attributes_topic: format!("{}/glucose", cfg.topic_prefix),
        device: Device {
            identifiers: [device_identifier.clone()],
            name: cfg
                .device_name
                .clone()
                .unwrap_or_else(|| format!("Gluco Hub ({})", cfg.client_id)),
            manufacturer: "gluco-hub-rs",
            model: "gluco-hub-rs",
            sw_version: env!("CARGO_PKG_VERSION"),
        },
    }
}

fn unique_id(cfg: &MqttSinkConfig) -> String {
    format!("gluco_hub_{}_glucose", cfg.client_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{MqttQos, MqttSinkConfig};

    fn cfg(client_id: &str) -> MqttSinkConfig {
        MqttSinkConfig {
            broker_host: "mqtt.example.com".into(),
            broker_port: 8883,
            client_id: client_id.into(),
            username: None,
            password: None,
            topic_prefix: format!("gluco-hub/{client_id}"),
            qos: MqttQos::default(),
            keep_alive_secs: 30,
            session_expiry_secs: 0,
            tls: true,
            include_patient_id: true,
            stats_interval_secs: 60,
            discovery_enabled: true,
            discovery_prefix: "homeassistant".into(),
            device_name: None,
        }
    }

    #[test]
    fn topic_uses_unique_id_under_discovery_prefix() {
        let c = cfg("kitchen-1");
        assert_eq!(
            discovery_topic(&c),
            "homeassistant/sensor/gluco_hub_kitchen-1_glucose/config"
        );
    }

    #[test]
    fn topic_respects_custom_discovery_prefix() {
        let mut c = cfg("dev-1");
        c.discovery_prefix = "ha-staging".into();
        assert!(discovery_topic(&c).starts_with("ha-staging/sensor/"));
    }

    #[test]
    fn payload_has_state_topic_under_topic_prefix() {
        let c = cfg("rpi-1");
        let p = build_discovery_payload(&c);
        assert_eq!(p.state_topic, "gluco-hub/rpi-1/glucose");
        assert_eq!(p.availability_topic, "gluco-hub/rpi-1/_health");
        assert_eq!(p.json_attributes_topic, "gluco-hub/rpi-1/glucose");
    }

    #[test]
    fn payload_unique_id_derives_from_client_id() {
        let c = cfg("phone-2");
        let p = build_discovery_payload(&c);
        assert_eq!(p.unique_id, "gluco_hub_phone-2_glucose");
        assert_eq!(p.device.identifiers, ["gluco_hub_phone-2".to_string()]);
    }

    #[test]
    fn default_device_name_falls_back_to_client_id_hint() {
        let c = cfg("hub-a");
        let p = build_discovery_payload(&c);
        assert_eq!(p.device.name, "Gluco Hub (hub-a)");
    }

    #[test]
    fn explicit_device_name_overrides_default() {
        let mut c = cfg("anything");
        c.device_name = Some("Kitchen CGM".into());
        let p = build_discovery_payload(&c);
        assert_eq!(p.device.name, "Kitchen CGM");
    }

    #[test]
    fn sw_version_matches_crate_version() {
        let c = cfg("v-1");
        let p = build_discovery_payload(&c);
        assert_eq!(p.device.sw_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn payload_serialises_with_ha_field_names() {
        let c = cfg("ser-1");
        let p = build_discovery_payload(&c);
        let v = serde_json::to_value(&p).expect("serialise");
        // HA-specific field names must survive serialisation verbatim.
        assert_eq!(v["unit_of_measurement"], "mg/dL");
        assert_eq!(v["value_template"], "{{ value_json.mgdl }}");
        assert_eq!(v["state_class"], "measurement");
        assert_eq!(v["device"]["manufacturer"], "gluco-hub-rs");
    }
}
