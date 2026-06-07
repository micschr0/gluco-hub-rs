// SPDX-License-Identifier: AGPL-3.0-or-later

//! Home Assistant MQTT auto-discovery (V3).
//!
//! When enabled, the MQTT sink publishes two retained config messages
//! on `<discovery_prefix>/sensor/<unique_id>/config` after each ConnAck:
//!  * a **glucose** sensor whose state is the current reading;
//!  * a **trend** sensor whose state is the current trend variant
//!    (`Flat`, `SingleUp`, …), declared as `device_class: "enum"` so HA
//!    classifies it as a finite-state text sensor.
//!
//! Both entities share the same `device` block, so HA groups them under
//! one device. They also share the same `state_topic`
//! (`<topic_prefix>/glucose`) and `availability_topic`
//! (`<topic_prefix>/_health`) — a single MQTT publish updates both
//! entities and they go unavailable together.
//!
//! Schema follows the HA MQTT discovery format
//! (<https://www.home-assistant.io/integrations/mqtt/#mqtt-discovery>).
//! Dynamic, per-state arrow icons are intentionally **not** part of the
//! discovery payload — HA's MQTT discovery does not support templated
//! `icon` fields, and rendering the arrow is a dashboard concern (a
//! `template`/`mushroom` card with state→icon mapping).

use serde::Serialize;

use crate::config::{MqttGlucoseUnit, MqttSinkConfig};

/// All `Trend` variants emitted in the wire payload, in HA `options:`
/// order. Mirrors `Trend::*` in `gluco-hub-core` and the PascalCase
/// strings produced by `super::wire::trend_to_str`. Kept in sync by
/// the `trend_options_cover_all_wire_variants` test below — if a
/// variant is added or renamed upstream, that test will fail.
pub(crate) const TREND_OPTIONS: &[&str] = &[
    "DoubleUp",
    "SingleUp",
    "FortyFiveUp",
    "Flat",
    "FortyFiveDown",
    "SingleDown",
    "DoubleDown",
    "NotComputable",
    "RateOutOfRange",
];

impl MqttGlucoseUnit {
    /// HA `unit_of_measurement` string for the discovery payload.
    pub(crate) fn unit_of_measurement(self) -> &'static str {
        match self {
            Self::MgDl => "mg/dL",
            Self::Mmol => "mmol/L",
        }
    }

    /// HA `value_template` (Jinja2) that reads the matching field from
    /// the JSON wire payload — both fields are always present.
    pub(crate) fn value_template(self) -> &'static str {
        match self {
            Self::MgDl => "{{ value_json.mgdl }}",
            Self::Mmol => "{{ value_json.mmol }}",
        }
    }
}

/// Glucose discovery payload, published once per ConnAck (retained, QoS 1).
/// Field names match HA's [MQTT sensor discovery schema][1] verbatim;
/// extending requires a doc update in `docs/ARCHITECTURE.md`.
///
/// [1]: https://www.home-assistant.io/integrations/sensor.mqtt/
#[derive(Debug, Serialize, PartialEq)]
pub struct DiscoveryPayload<'a> {
    pub name: &'static str,
    pub has_entity_name: bool,
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
    pub origin: Origin<'a>,
}

/// Trend discovery payload — a sibling sensor entity declared as
/// `device_class: "enum"` with the full list of `Trend` variants in
/// `options`. Has no `unit_of_measurement` and no `state_class`: the
/// state is a categorical string, not a numeric measurement. Shares
/// the glucose entity's `state_topic`, `availability_*`, and `device`.
#[derive(Debug, Serialize, PartialEq)]
pub struct TrendDiscoveryPayload<'a> {
    pub name: &'static str,
    pub has_entity_name: bool,
    pub unique_id: String,
    pub state_topic: String,
    pub value_template: &'static str,
    pub device_class: &'static str,
    pub options: &'static [&'static str],
    pub icon: &'static str,
    pub availability_topic: String,
    pub availability_template: &'static str,
    pub payload_available: &'static str,
    pub payload_not_available: &'static str,
    pub device: Device<'a>,
    pub origin: Origin<'a>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Device<'a> {
    pub identifiers: [String; 1],
    pub name: String,
    pub manufacturer: &'static str,
    pub model: &'static str,
    pub sw_version: &'a str,
}

/// `origin` block introduced in HA 2024.6 — surfaces the upstream
/// integration name, version, and support URL in HA's device picker so
/// users can see which tool published the entity.
#[derive(Debug, Serialize, PartialEq)]
pub struct Origin<'a> {
    pub name: &'static str,
    pub sw_version: &'a str,
    pub support_url: &'static str,
}

/// Manufacturer / module identity strings, shared by the `Device` and
/// `Origin` blocks of both entities so they group under one device.
const MANUFACTURER: &str = "gluco-hub-rs";
const MODEL: &str = "gluco-hub-rs";
const SUPPORT_URL: &str = "https://github.com/micschr0/gluco-hub-rs";

/// Topic that HA listens to for the glucose discovery message.
/// Conventional shape: `<discovery_prefix>/sensor/<unique_id>/config`.
pub fn discovery_topic(cfg: &MqttSinkConfig) -> String {
    format!(
        "{}/sensor/{}/config",
        cfg.discovery_prefix,
        unique_id(cfg, "glucose"),
    )
}

/// Topic that HA listens to for the trend discovery message. Same
/// shape as [`discovery_topic`], suffixed with `_trend`.
pub fn discovery_topic_trend(cfg: &MqttSinkConfig) -> String {
    format!(
        "{}/sensor/{}/config",
        cfg.discovery_prefix,
        unique_id(cfg, "trend"),
    )
}

/// Build the glucose JSON config payload. `sw_version` is the binary's
/// `CARGO_PKG_VERSION` at compile time so HA shows which release
/// emitted the entity.
pub fn build_discovery_payload(cfg: &MqttSinkConfig) -> DiscoveryPayload<'_> {
    DiscoveryPayload {
        name: "Glucose",
        has_entity_name: true,
        unique_id: unique_id(cfg, "glucose"),
        state_topic: format!("{}/glucose", cfg.topic_prefix),
        value_template: cfg.discovery_unit.value_template(),
        unit_of_measurement: cfg.discovery_unit.unit_of_measurement(),
        state_class: "measurement",
        icon: "mdi:water-percent",
        availability_topic: format!("{}/_health", cfg.topic_prefix),
        availability_template: "{{ 'online' if value_json.online else 'offline' }}",
        payload_available: "online",
        payload_not_available: "offline",
        json_attributes_topic: format!("{}/glucose", cfg.topic_prefix),
        device: build_device(cfg),
        origin: build_origin(),
    }
}

/// Build the trend JSON config payload. Sibling of the glucose entity
/// — same device, same state and availability topics, but the state is
/// the categorical `Trend` variant string.
pub fn build_trend_discovery_payload(cfg: &MqttSinkConfig) -> TrendDiscoveryPayload<'_> {
    TrendDiscoveryPayload {
        name: "Trend",
        has_entity_name: true,
        unique_id: unique_id(cfg, "trend"),
        state_topic: format!("{}/glucose", cfg.topic_prefix),
        value_template: "{{ value_json.trend }}",
        device_class: "enum",
        options: TREND_OPTIONS,
        icon: "mdi:trending-up",
        availability_topic: format!("{}/_health", cfg.topic_prefix),
        availability_template: "{{ 'online' if value_json.online else 'offline' }}",
        payload_available: "online",
        payload_not_available: "offline",
        device: build_device(cfg),
        origin: build_origin(),
    }
}

fn build_device(cfg: &MqttSinkConfig) -> Device<'_> {
    let device_identifier = format!("gluco_hub_{}", cfg.client_id);
    Device {
        identifiers: [device_identifier],
        name: cfg
            .device_name
            .clone()
            .unwrap_or_else(|| format!("Gluco Hub ({})", cfg.client_id)),
        manufacturer: MANUFACTURER,
        model: MODEL,
        sw_version: env!("CARGO_PKG_VERSION"),
    }
}

fn build_origin() -> Origin<'static> {
    Origin {
        name: MANUFACTURER,
        sw_version: env!("CARGO_PKG_VERSION"),
        support_url: SUPPORT_URL,
    }
}

fn unique_id(cfg: &MqttSinkConfig, kind: &str) -> String {
    format!("gluco_hub_{}_{}", cfg.client_id, kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{MqttGlucoseUnit, MqttQos, MqttSinkConfig};

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
            discovery_unit: MqttGlucoseUnit::default(),
            client_cert_file: None,
            client_key_file: None,
            tailscale_hostname: None,
            per_source: false,
        }
    }

    // --- Glucose entity ----------------------------------------------------

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
        assert_eq!(p.origin.sw_version, env!("CARGO_PKG_VERSION"));
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
        assert_eq!(v["has_entity_name"], true);
        assert_eq!(v["origin"]["name"], "gluco-hub-rs");
        assert_eq!(v["origin"]["support_url"], SUPPORT_URL);
    }

    #[test]
    fn glucose_payload_does_not_carry_device_class() {
        // Glucose is a plain numeric measurement; setting device_class
        // here would clash with state_class = "measurement".
        let c = cfg("nodc");
        let p = build_discovery_payload(&c);
        let v = serde_json::to_value(&p).expect("serialise");
        assert!(
            v.get("device_class").is_none(),
            "glucose must not declare a device_class"
        );
    }

    #[test]
    fn default_discovery_unit_is_mgdl() {
        // Preserves V2 / V3 behaviour — operators upgrading do not see
        // a unit change unless they explicitly opt in.
        assert_eq!(MqttGlucoseUnit::default(), MqttGlucoseUnit::MgDl);
    }

    #[test]
    fn mmol_discovery_unit_switches_unit_and_template() {
        let mut c = cfg("eu-1");
        c.discovery_unit = MqttGlucoseUnit::Mmol;
        let p = build_discovery_payload(&c);
        assert_eq!(p.unit_of_measurement, "mmol/L");
        assert_eq!(p.value_template, "{{ value_json.mmol }}");
    }

    #[test]
    fn mgdl_discovery_unit_keeps_legacy_strings() {
        let mut c = cfg("us-1");
        c.discovery_unit = MqttGlucoseUnit::MgDl;
        let p = build_discovery_payload(&c);
        assert_eq!(p.unit_of_measurement, "mg/dL");
        assert_eq!(p.value_template, "{{ value_json.mgdl }}");
    }

    #[test]
    fn mqtt_glucose_unit_deserialises_from_lowercase_strings() {
        // ENV-based config sets the field as a plain lowercase string;
        // serde must accept both forms.
        let mgdl: MqttGlucoseUnit = serde_json::from_str(r#""mgdl""#).expect("mgdl");
        let mmol: MqttGlucoseUnit = serde_json::from_str(r#""mmol""#).expect("mmol");
        assert_eq!(mgdl, MqttGlucoseUnit::MgDl);
        assert_eq!(mmol, MqttGlucoseUnit::Mmol);
    }

    // --- Trend entity ------------------------------------------------------

    #[test]
    fn trend_topic_uses_trend_suffix_under_discovery_prefix() {
        let c = cfg("kitchen-1");
        assert_eq!(
            discovery_topic_trend(&c),
            "homeassistant/sensor/gluco_hub_kitchen-1_trend/config"
        );
    }

    #[test]
    fn trend_payload_unique_id_derives_from_client_id() {
        let c = cfg("phone-2");
        let p = build_trend_discovery_payload(&c);
        assert_eq!(p.unique_id, "gluco_hub_phone-2_trend");
    }

    #[test]
    fn trend_payload_reads_trend_field_via_value_template() {
        let c = cfg("vt-1");
        let p = build_trend_discovery_payload(&c);
        assert_eq!(p.value_template, "{{ value_json.trend }}");
    }

    #[test]
    fn trend_payload_shares_state_and_availability_topics_with_glucose() {
        // Single MQTT publish must update both entities → state_topic
        // and availability_topic are identical strings on both payloads.
        let c = cfg("share-1");
        let g = build_discovery_payload(&c);
        let t = build_trend_discovery_payload(&c);
        assert_eq!(g.state_topic, t.state_topic);
        assert_eq!(g.availability_topic, t.availability_topic);
        assert_eq!(g.availability_template, t.availability_template);
    }

    #[test]
    fn trend_payload_shares_device_identifier_with_glucose() {
        // Same device.identifiers → HA groups both entities under one
        // device card.
        let c = cfg("dev-1");
        let g = build_discovery_payload(&c);
        let t = build_trend_discovery_payload(&c);
        assert_eq!(g.device.identifiers, t.device.identifiers);
        assert_eq!(g.device.name, t.device.name);
    }

    #[test]
    fn trend_payload_declares_device_class_enum_with_full_options() {
        let c = cfg("opt-1");
        let p = build_trend_discovery_payload(&c);
        assert_eq!(p.device_class, "enum");
        // Order-independent membership check — HA does not care about
        // option order, only that every emitted state is listed.
        for v in [
            "DoubleUp",
            "SingleUp",
            "FortyFiveUp",
            "Flat",
            "FortyFiveDown",
            "SingleDown",
            "DoubleDown",
            "NotComputable",
            "RateOutOfRange",
        ] {
            assert!(
                p.options.contains(&v),
                "trend options missing {v}: {:?}",
                p.options
            );
        }
    }

    #[test]
    fn trend_payload_has_no_state_class_or_unit() {
        // Categorical sensor — declaring either would be wrong and HA's
        // schema validator would reject `state_class != measurement|total|*`.
        let c = cfg("ns-1");
        let p = build_trend_discovery_payload(&c);
        let v = serde_json::to_value(&p).expect("serialise");
        assert!(
            v.get("state_class").is_none(),
            "trend must not declare a state_class"
        );
        assert!(
            v.get("unit_of_measurement").is_none(),
            "trend must not declare a unit_of_measurement"
        );
    }

    #[test]
    fn trend_payload_serialises_with_ha_field_names() {
        let c = cfg("ser-2");
        let p = build_trend_discovery_payload(&c);
        let v = serde_json::to_value(&p).expect("serialise");
        assert_eq!(v["name"], "Trend");
        assert_eq!(v["has_entity_name"], true);
        assert_eq!(v["device_class"], "enum");
        assert_eq!(v["value_template"], "{{ value_json.trend }}");
        assert_eq!(v["icon"], "mdi:trending-up");
        assert_eq!(v["origin"]["support_url"], SUPPORT_URL);
        assert!(v["options"].is_array(), "options must be a JSON array");
    }

    #[test]
    fn trend_options_cover_all_wire_variants() {
        // Guardrail: if a new Trend variant is added upstream in
        // gluco-hub-core or wire.rs grows a new branch, this list must
        // grow too — otherwise HA would receive a state outside the
        // declared enum and refuse it.
        use gluco_hub_core::Trend;
        for t in [
            Trend::DoubleUp,
            Trend::SingleUp,
            Trend::FortyFiveUp,
            Trend::Flat,
            Trend::FortyFiveDown,
            Trend::SingleDown,
            Trend::DoubleDown,
            Trend::NotComputable,
            Trend::RateOutOfRange,
        ] {
            let wire = super::super::wire::trend_to_str(t);
            assert!(
                TREND_OPTIONS.contains(&wire),
                "TREND_OPTIONS missing wire value for {t:?}: {wire}",
            );
        }
    }
}
