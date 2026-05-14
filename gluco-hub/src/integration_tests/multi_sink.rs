// SPDX-License-Identifier: AGPL-3.0-or-later

//! Phase C: end-to-end fan-out through the full
//! `SinkRouter → DlqSink → real sink` layering against both backends
//! simultaneously. Proves the V3 wiring (HA discovery, watermark
//! backfill, DLQ) works against real services rather than just unit
//! mocks.
//!
//! Each test boots a Mosquitto broker AND a Nightscout/Mongo stack —
//! ~30 s combined boot time. Most of the work in this file is wire-up;
//! the actual assertions are tight.

use std::sync::Arc;
use std::time::Duration;

use gluco_hub_core::Sink;
use secrecy::SecretString;
use serde_json::Value;

use crate::config::{MqttQos, MqttSinkConfig};
use crate::dlq::DlqSink;
use crate::sink_router::SinkRouter;
use crate::sinks::mqtt::MqttSink;
use crate::sinks::nightscout::{NightscoutClient, NightscoutSink};

use super::common::mosquitto::{start_mosquitto, subscribe_to, wait_for_topic};
use super::common::nightscout_container::{
    API_SECRET, api_secret_sha1, fetch_entries_v3, start_nightscout_stack,
};
use super::common::{reading, unique_id};

#[tokio::test]
async fn one_reading_fans_out_to_both_ns_and_mqtt() {
    // -- Backends.
    let mqtt = start_mosquitto().await.expect("mosquitto");
    let ns = start_nightscout_stack().await.expect("nightscout stack");
    let device_id = unique_id("e2e-fanout");
    let topic_prefix = format!("itest/{device_id}");

    // -- MQTT subscriber to catch the glucose publish.
    let (mut rx, _cancel) = subscribe_to(&mqtt, &format!("{topic_prefix}/glucose"), "sub-e2e")
        .await
        .expect("subscribe");

    // -- Build the full sink layering: SinkRouter → DlqSink → real sink.
    let dlq_dir = tempfile::tempdir().expect("dlq dir");

    let ns_client = NightscoutClient::new(ns.ns_url(), SecretString::from(API_SECRET))
        .expect("ns client")
        .with_device(&device_id)
        .with_app("gluco-hub-itest");
    let ns_sink: Arc<dyn Sink> = Arc::new(NightscoutSink::new(ns_client));
    let ns_dlq = Arc::new(DlqSink::open(ns_sink, dlq_dir.path(), 1000).expect("dlq ns"));
    let ns_router = Arc::new(SinkRouter::new(ns_dlq));

    let (host, port) = mqtt.broker_addr();
    let mqtt_cfg = MqttSinkConfig {
        broker_host: host,
        broker_port: port,
        client_id: device_id.clone(),
        username: None,
        password: None,
        topic_prefix: topic_prefix.clone(),
        qos: MqttQos::AtLeastOnce,
        keep_alive_secs: 5,
        session_expiry_secs: 0,
        tls: false,
        include_patient_id: true,
        stats_interval_secs: 60,
        discovery_enabled: false,
        discovery_prefix: "homeassistant".into(),
        device_name: None,
    };
    let mqtt_sink: Arc<dyn Sink> = Arc::new(MqttSink::new(&mqtt_cfg, None).expect("mqtt sink"));
    let mqtt_dlq = Arc::new(DlqSink::open(mqtt_sink, dlq_dir.path(), 1000).expect("dlq mqtt"));
    let mqtt_router = Arc::new(SinkRouter::new(mqtt_dlq));

    // -- Fan-out one fresh reading.
    let ts = chrono::Utc::now().timestamp() - 60;
    let r = reading(ts, 124.0);
    let (_ns_outcome, ns_result) = ns_router.push_filtered(std::slice::from_ref(&r)).await;
    let (_mq_outcome, mq_result) = mqtt_router.push_filtered(std::slice::from_ref(&r)).await;
    ns_result.expect("ns push");
    mq_result.expect("mqtt push");

    // -- Verify MQTT side.
    let pkt = wait_for_topic(
        &mut rx,
        &format!("{topic_prefix}/glucose"),
        Duration::from_secs(5),
    )
    .await
    .expect("glucose publish must arrive");
    let body: Value = serde_json::from_slice(&pkt.payload).expect("json");
    assert_eq!(body["mgdl"], 124.0);

    // -- Verify NS side.
    let entries = fetch_entries_v3(&ns, &api_secret_sha1(), 5)
        .await
        .expect("fetch entries");
    let result = entries
        .get("result")
        .and_then(|v| v.as_array())
        .expect("result array");
    let entry = result
        .iter()
        .find(|e| e.get("device").and_then(|d| d.as_str()) == Some(device_id.as_str()))
        .unwrap_or_else(|| panic!("entry for device {device_id} not found: {entries:?}"));
    assert_eq!(entry["sgv"], 124.0);
    assert_eq!(entry["direction"], "Flat");
}

#[tokio::test]
async fn watermark_drops_duplicates_across_both_sinks_in_steady_state() {
    // Sends the same batch twice through both routers; the second call
    // must filter every reading because all timestamps are <= watermark.
    let mqtt = start_mosquitto().await.expect("mosquitto");
    let ns = start_nightscout_stack().await.expect("ns stack");
    let device_id = unique_id("e2e-wmark");
    let topic_prefix = format!("itest/{device_id}");

    let dlq_dir = tempfile::tempdir().expect("dlq dir");

    let ns_client = NightscoutClient::new(ns.ns_url(), SecretString::from(API_SECRET))
        .expect("ns client")
        .with_device(&device_id)
        .with_app("gluco-hub-itest");
    let ns_sink: Arc<dyn Sink> = Arc::new(NightscoutSink::new(ns_client));
    let ns_router = Arc::new(SinkRouter::new(Arc::new(
        DlqSink::open(ns_sink, dlq_dir.path(), 1000).expect("dlq ns"),
    )));

    let (host, port) = mqtt.broker_addr();
    let mqtt_cfg = MqttSinkConfig {
        broker_host: host,
        broker_port: port,
        client_id: device_id.clone(),
        username: None,
        password: None,
        topic_prefix: topic_prefix.clone(),
        qos: MqttQos::AtLeastOnce,
        keep_alive_secs: 5,
        session_expiry_secs: 0,
        tls: false,
        include_patient_id: true,
        stats_interval_secs: 60,
        discovery_enabled: false,
        discovery_prefix: "homeassistant".into(),
        device_name: None,
    };
    let mqtt_sink: Arc<dyn Sink> = Arc::new(MqttSink::new(&mqtt_cfg, None).expect("mqtt sink"));
    let mqtt_router = Arc::new(SinkRouter::new(Arc::new(
        DlqSink::open(mqtt_sink, dlq_dir.path(), 1000).expect("dlq mqtt"),
    )));

    // Same batch fired twice. The first time everything goes through;
    // the second time the watermark filters all of it.
    let now = chrono::Utc::now().timestamp();
    let batch: Vec<_> = (0..3)
        .map(|i| reading(now - 60 - (i as i64) * 30, 100.0 + (i as f64) * 5.0))
        .collect();

    let (first_ns, _) = ns_router.push_filtered(&batch).await;
    let (first_mq, _) = mqtt_router.push_filtered(&batch).await;
    assert_eq!(first_ns.pushed, 3);
    assert_eq!(first_mq.pushed, 3);

    let (second_ns, _) = ns_router.push_filtered(&batch).await;
    let (second_mq, _) = mqtt_router.push_filtered(&batch).await;
    assert_eq!(second_ns.pushed, 0, "watermark must filter all 3 readings");
    assert_eq!(second_mq.pushed, 0, "watermark must filter all 3 readings");
    assert_eq!(second_ns.filtered, 3);
    assert_eq!(second_mq.filtered, 3);

    // NS still shows exactly 3 entries for this device.
    let body = fetch_entries_v3(&ns, &api_secret_sha1(), 10)
        .await
        .expect("fetch");
    let result = body
        .get("result")
        .and_then(|v| v.as_array())
        .expect("result array");
    let mine = result
        .iter()
        .filter(|e| e.get("device").and_then(|d| d.as_str()) == Some(device_id.as_str()))
        .count();
    assert_eq!(
        mine, 3,
        "expected 3 NS entries after duplicate batch; got {mine}: {body:?}"
    );
}
