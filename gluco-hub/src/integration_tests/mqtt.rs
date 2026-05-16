// SPDX-License-Identifier: AGPL-3.0-or-later

//! Phase A: MqttSink against a real Eclipse Mosquitto broker.
//!
//! Coverage beyond the in-process stub broker (`sinks::mqtt::sink::tests`):
//!  * real MQTT v5 server speaking on a real TCP socket;
//!  * retained-message semantics across real subscriber sessions;
//!  * HA discovery payload validated against HA's documented schema.

use std::time::Duration;

use chrono::{TimeZone, Utc};
use gluco_hub_core::Sink;
use serde_json::Value;

use crate::config::{MqttGlucoseUnit, MqttQos, MqttSinkConfig};
use crate::sinks::mqtt::MqttSink;

use super::common::ha_schema::validate_sensor_discovery;
use super::common::mosquitto::{start_mosquitto, subscribe_to, wait_for_topic};
use super::common::{reading, unique_id};

fn cfg(host: &str, port: u16, client_id: &str, topic_prefix: &str) -> MqttSinkConfig {
    MqttSinkConfig {
        broker_host: host.to_string(),
        broker_port: port,
        client_id: client_id.to_string(),
        username: None,
        password: None,
        topic_prefix: topic_prefix.to_string(),
        qos: MqttQos::AtLeastOnce,
        keep_alive_secs: 5,
        session_expiry_secs: 0,
        tls: false,
        include_patient_id: true,
        stats_interval_secs: 60,
        discovery_enabled: false,
        discovery_prefix: "homeassistant".into(),
        device_name: None,
        discovery_unit: MqttGlucoseUnit::default(),
    }
}

#[tokio::test]
async fn glucose_payload_published_with_v1_schema() {
    let broker = start_mosquitto().await.expect("start mosquitto");
    let (host, port) = broker.broker_addr();
    let client_id = unique_id("glcv1");
    let prefix = format!("itest/{client_id}");

    let (mut rx, _cancel) = subscribe_to(&broker, &format!("{prefix}/glucose"), "sub-glcv1")
        .await
        .expect("subscribe");

    let sink = MqttSink::new(&cfg(&host, port, &client_id, &prefix), None).expect("sink");
    sink.push(&[reading(1_700_000_000, 142.0)])
        .await
        .expect("push");

    let pkt = wait_for_topic(
        &mut rx,
        &format!("{prefix}/glucose"),
        Duration::from_secs(5),
    )
    .await
    .expect("glucose publish must arrive");

    assert!(!pkt.retain, "glucose topic must NOT be retained");
    let body: Value = serde_json::from_slice(&pkt.payload).expect("json");
    assert_eq!(body["v"], 1);
    assert_eq!(body["mgdl"], 142.0);
    assert_eq!(body["trend"], "Flat");
    assert_eq!(body["source"], "itest");
}

#[tokio::test]
async fn health_topic_retained_with_online_marker() {
    let broker = start_mosquitto().await.expect("start mosquitto");
    let (host, port) = broker.broker_addr();
    let client_id = unique_id("hlth");
    let prefix = format!("itest/{client_id}");

    let (mut rx, _cancel) = subscribe_to(&broker, &format!("{prefix}/_health"), "sub-hlth")
        .await
        .expect("subscribe");

    let _sink = MqttSink::new(&cfg(&host, port, &client_id, &prefix), None).expect("sink");

    let pkt = wait_for_topic(
        &mut rx,
        &format!("{prefix}/_health"),
        Duration::from_secs(5),
    )
    .await
    .expect("health publish must arrive");

    assert!(pkt.retain, "_health must be retained");
    let body: Value = serde_json::from_slice(&pkt.payload).expect("json");
    assert_eq!(body["online"], true);
    assert_eq!(body["v"], 1);
}

#[tokio::test]
async fn discovery_config_published_when_enabled_and_passes_ha_schema() {
    let broker = start_mosquitto().await.expect("start mosquitto");
    let (host, port) = broker.broker_addr();
    let client_id = unique_id("disc");
    let prefix = format!("itest/{client_id}");

    let mut c = cfg(&host, port, &client_id, &prefix);
    c.discovery_enabled = true;

    let discovery_topic = format!("homeassistant/sensor/gluco_hub_{client_id}_glucose/config");
    let (mut rx, _cancel) = subscribe_to(&broker, &discovery_topic, "sub-disc")
        .await
        .expect("subscribe");

    let _sink = MqttSink::new(&c, None).expect("sink");

    let pkt = wait_for_topic(&mut rx, &discovery_topic, Duration::from_secs(5))
        .await
        .expect("discovery publish must arrive");

    assert!(pkt.retain, "discovery config must be retained");
    let body: Value = serde_json::from_slice(&pkt.payload).expect("json");

    // HA-side schema validation: catches drift in either direction.
    validate_sensor_discovery(&body).expect("HA discovery schema");

    // Tight assertions on fields we control directly.
    assert_eq!(body["state_topic"], format!("{prefix}/glucose"));
    assert_eq!(body["availability_topic"], format!("{prefix}/_health"));
    assert_eq!(body["value_template"], "{{ value_json.mgdl }}");
    assert_eq!(body["device"]["manufacturer"], "gluco-hub-rs");
    assert_eq!(body["unique_id"], format!("gluco_hub_{client_id}_glucose"));
}

#[tokio::test]
async fn trend_discovery_config_published_when_enabled_and_passes_ha_schema() {
    let broker = start_mosquitto().await.expect("start mosquitto");
    let (host, port) = broker.broker_addr();
    let client_id = unique_id("trend");
    let prefix = format!("itest/{client_id}");

    let mut c = cfg(&host, port, &client_id, &prefix);
    c.discovery_enabled = true;

    let trend_topic = format!("homeassistant/sensor/gluco_hub_{client_id}_trend/config");
    let (mut rx, _cancel) = subscribe_to(&broker, &trend_topic, "sub-trend")
        .await
        .expect("subscribe");

    let _sink = MqttSink::new(&c, None).expect("sink");

    let pkt = wait_for_topic(&mut rx, &trend_topic, Duration::from_secs(5))
        .await
        .expect("trend discovery publish must arrive");

    assert!(pkt.retain, "trend discovery config must be retained");
    let body: Value = serde_json::from_slice(&pkt.payload).expect("json");

    // HA-side schema validation — passes because the trend payload
    // intentionally omits `state_class` (categorical, not a numeric
    // measurement) and the schema treats `state_class` as optional.
    validate_sensor_discovery(&body).expect("HA discovery schema");

    // Tight assertions on fields we control directly.
    assert_eq!(body["state_topic"], format!("{prefix}/glucose"));
    assert_eq!(body["availability_topic"], format!("{prefix}/_health"));
    assert_eq!(body["value_template"], "{{ value_json.trend }}");
    assert_eq!(body["unique_id"], format!("gluco_hub_{client_id}_trend"));
    assert_eq!(body["device_class"], "enum");
    assert_eq!(body["icon"], "mdi:trending-up");
    assert_eq!(body["device"]["manufacturer"], "gluco-hub-rs");
    assert_eq!(
        body["device"]["identifiers"][0],
        format!("gluco_hub_{client_id}")
    );

    // Every wire `Trend` variant must be in `options:` — otherwise HA
    // rejects readings whose state isn't in the declared enum.
    let options = body["options"].as_array().expect("options array");
    for expected in [
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
            options.iter().any(|v| v == expected),
            "trend options missing {expected}: {options:?}"
        );
    }
}

#[tokio::test]
async fn discovery_silent_when_disabled() {
    let broker = start_mosquitto().await.expect("start mosquitto");
    let (host, port) = broker.broker_addr();
    let client_id = unique_id("nodisc");
    let prefix = format!("itest/{client_id}");

    let (mut rx, _cancel) = subscribe_to(&broker, "homeassistant/#", "sub-nodisc")
        .await
        .expect("subscribe");

    let _sink = MqttSink::new(&cfg(&host, port, &client_id, &prefix), None).expect("sink");

    let saw_discovery = tokio::time::timeout(Duration::from_millis(800), async {
        while let Some(p) = rx.recv().await {
            if p.topic.starts_with("homeassistant/") {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);

    assert!(
        !saw_discovery,
        "no discovery config must be published when discovery_enabled = false"
    );
}

#[tokio::test]
async fn reading_timestamp_round_trips_unchanged() {
    // Sanity: a fresh-timestamp reading produces a payload whose `ts`
    // field round-trips back to the same millisecond. Catches subtle
    // serde / chrono drift if either gets re-version-bumped.
    let broker = start_mosquitto().await.expect("start mosquitto");
    let (host, port) = broker.broker_addr();
    let client_id = unique_id("rtrip");
    let prefix = format!("itest/{client_id}");

    let (mut rx, _cancel) = subscribe_to(&broker, &format!("{prefix}/glucose"), "sub-rtrip")
        .await
        .expect("subscribe");

    let sink = MqttSink::new(&cfg(&host, port, &client_id, &prefix), None).expect("sink");

    let ts_secs = Utc
        .with_ymd_and_hms(2026, 5, 14, 12, 0, 0)
        .single()
        .unwrap()
        .timestamp();
    sink.push(&[reading(ts_secs, 110.0)]).await.expect("push");

    let pkt = wait_for_topic(
        &mut rx,
        &format!("{prefix}/glucose"),
        Duration::from_secs(5),
    )
    .await
    .expect("glucose publish must arrive");

    let body: Value = serde_json::from_slice(&pkt.payload).expect("json");
    // wire schema serialises `ts` as unix-ms.
    assert_eq!(body["ts"], ts_secs * 1000);
}
