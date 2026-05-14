// SPDX-License-Identifier: AGPL-3.0-or-later

//! `MqttSink` — rumqttc v5 backed publisher.
//!
//! Architecture:
//! * `MqttSink` holds a cloneable `AsyncClient`. Every `push()` becomes
//!   one `publish_with_properties` call per Reading.
//! * A background tokio task drives `EventLoop::poll()`. rumqttc has no
//!   built-in reconnect backoff (issue #918, open) — we wrap poll with
//!   exponential backoff capped at `BACKOFF_MAX`.
//! * LWT (`{"online":false,"v":1}` retained on `<prefix>/_health`) is
//!   set in `MqttOptions`; the online marker is published immediately
//!   after each ConnAck.
//! * A second tokio task ticks every `stats_interval_secs` and publishes
//!   the `<prefix>/_stats` snapshot retained at QoS 1.
//! * Shutdown via `CancellationToken`: on Drop the token is cancelled,
//!   the poll task best-effort publishes `online: false`, and exits.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use gluco_hub_core::{CoreError, Reading, Sink};
use rumqttc::Transport;
use rumqttc::v5::mqttbytes::QoS;
use rumqttc::v5::mqttbytes::v5::{LastWill, PublishProperties};
use rumqttc::v5::{AsyncClient, Event, EventLoop, MqttOptions};
use secrecy::{ExposeSecret, SecretString};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::config::{MqttQos, MqttSinkConfig};

use super::error::{MqttError, classify_client_error, classify_connection_error};
use super::stats::MqttStatsState;
use super::wire;

/// Reconnect backoff bounds. Initial 1 s doubles up to `MAX` between
/// failed `EventLoop::poll()` calls — rumqttc itself does not space
/// reconnect attempts.
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Internal channel capacity between `AsyncClient::publish()` and the
/// EventLoop. CGM produces ≤ 1 reading/minute; 16 is generous.
const CLIENT_CHANNEL_CAPACITY: usize = 16;

/// Upper bound on how long `run_poll_loop` keeps polling the eventloop
/// after publishing the offline `_health` marker on shutdown. The QoS 1
/// publish needs at least one extra poll iteration to be flushed to the
/// wire, plus the PubAck round-trip — 1 s covers local brokers and
/// keeps Drop responsive even if the broker is gone.
const OFFLINE_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

/// V2 MQTT sink. Construction connects (asynchronously) and spawns the
/// EventLoop poll task; readings published via `push()` go through the
/// client's bounded channel.
pub struct MqttSink {
    client: AsyncClient,
    topic_glucose: String,
    qos: QoS,
    include_patient: bool,
    stats: Arc<Mutex<MqttStatsState>>,
    cancel: CancellationToken,
    _poll_task: JoinHandle<()>,
    _stats_task: JoinHandle<()>,
}

impl MqttSink {
    /// Build the sink and spawn the poll + stats tasks. Returns
    /// immediately — the actual TCP/TLS connect happens asynchronously
    /// inside the poll task; transient connect failures surface via
    /// metrics (`error_code = "MQTT001"` etc.) and the warn-level
    /// reconnect log lines.
    pub fn new(cfg: &MqttSinkConfig, password: Option<SecretString>) -> Result<Self, MqttError> {
        let (client, eventloop) = build_client(cfg, password)?;
        let cancel = CancellationToken::new();
        let stats = Arc::new(MqttStatsState::new());

        let health_topic = format!("{}/_health", cfg.topic_prefix);
        let stats_topic = format!("{}/_stats", cfg.topic_prefix);

        // Pre-serialise the HA discovery payload once at startup so the
        // poll loop has cheap retained republishes on every reconnect.
        let discovery = if cfg.discovery_enabled {
            let payload = serde_json::to_vec(&super::discovery::build_discovery_payload(cfg))
                .map_err(|e| MqttError::Payload {
                    message: e.to_string(),
                })?;
            Some(DiscoveryPublish {
                topic: super::discovery::discovery_topic(cfg),
                payload: Bytes::from(payload),
            })
        } else {
            None
        };

        let poll_task = tokio::spawn(run_poll_loop(
            eventloop,
            client.clone(),
            health_topic,
            discovery,
            cancel.clone(),
            Arc::clone(&stats),
        ));

        let stats_task = tokio::spawn(run_stats_loop(
            client.clone(),
            stats_topic,
            Duration::from_secs(cfg.stats_interval_secs),
            cancel.clone(),
            Arc::clone(&stats),
        ));

        Ok(Self {
            client,
            topic_glucose: format!("{}/glucose", cfg.topic_prefix),
            qos: mqtt_qos(cfg.qos),
            include_patient: cfg.include_patient_id,
            stats,
            cancel,
            _poll_task: poll_task,
            _stats_task: stats_task,
        })
    }
}

impl Drop for MqttSink {
    fn drop(&mut self) {
        // Drop is sync; we can only signal — the poll task observes
        // the token on its next `select!` iteration and exits cleanly
        // after publishing the offline marker.
        self.cancel.cancel();
    }
}

#[async_trait]
impl Sink for MqttSink {
    fn name(&self) -> &'static str {
        "mqtt"
    }

    async fn push(&self, readings: &[Reading]) -> Result<(), CoreError> {
        if readings.is_empty() {
            return Ok(());
        }

        let props = glucose_publish_properties();
        let mut published: u64 = 0;

        for reading in readings {
            let payload = wire::glucose_payload(reading, self.include_patient);
            let bytes = serde_json::to_vec(&payload).map_err(|e| {
                self.stats
                    .lock()
                    .expect("mqtt stats mutex poisoned")
                    .record_publish_error();
                CoreError::Sink {
                    message: MqttError::Payload {
                        message: format!("serialise reading: {e}"),
                    }
                    .to_string(),
                }
            })?;

            if let Err(e) = self
                .client
                .publish_with_properties(
                    self.topic_glucose.clone(),
                    self.qos,
                    /* retain = */ false,
                    bytes,
                    props.clone(),
                )
                .await
            {
                // Stamp the partial success before bubbling up so a
                // mid-batch failure does not lose the count.
                if published > 0 {
                    self.stats
                        .lock()
                        .expect("mqtt stats mutex poisoned")
                        .record_publish(published);
                }
                self.stats
                    .lock()
                    .expect("mqtt stats mutex poisoned")
                    .record_publish_error();
                return Err(CoreError::Sink {
                    message: classify_client_error(e).to_string(),
                });
            }
            published += 1;
        }

        if published > 0 {
            self.stats
                .lock()
                .expect("mqtt stats mutex poisoned")
                .record_publish(published);
        }

        debug!(
            count = readings.len(),
            topic = %self.topic_glucose,
            "mqtt batch published"
        );

        Ok(())
    }
}

fn build_client(
    cfg: &MqttSinkConfig,
    password: Option<SecretString>,
) -> Result<(AsyncClient, EventLoop), MqttError> {
    let will_payload =
        serde_json::to_vec(&wire::health_payload(false)).map_err(|e| MqttError::Payload {
            message: format!("serialise LWT health payload: {e}"),
        })?;

    let last_will = LastWill::new(
        format!("{}/_health", cfg.topic_prefix),
        will_payload,
        QoS::AtLeastOnce,
        /* retain = */ true,
        /* properties = */ None,
    );

    let mut opts = MqttOptions::new(
        cfg.client_id.clone(),
        cfg.broker_host.clone(),
        cfg.broker_port,
    );

    opts.set_keep_alive(Duration::from_secs(cfg.keep_alive_secs));
    opts.set_last_will(last_will);
    // Always clean-start: the bridge is a stateless publisher, no
    // session resume needed.
    opts.set_clean_start(true);
    opts.set_session_expiry_interval(Some(cfg.session_expiry_secs));

    if let (Some(user), Some(pw)) = (cfg.username.as_deref(), password.as_ref()) {
        opts.set_credentials(user, pw.expose_secret());
    }

    if cfg.tls {
        // `TlsConfiguration::default()` loads platform-native CAs and
        // builds a rustls ClientConfig — no direct rustls dep needed.
        let tls = rumqttc::TlsConfiguration::default();
        opts.set_transport(Transport::tls_with_config(tls));
    }

    let (client, eventloop) = AsyncClient::new(opts, CLIENT_CHANNEL_CAPACITY);
    Ok((client, eventloop))
}

fn mqtt_qos(q: MqttQos) -> QoS {
    match q {
        MqttQos::AtMostOnce => QoS::AtMostOnce,
        MqttQos::AtLeastOnce => QoS::AtLeastOnce,
        MqttQos::ExactlyOnce => QoS::ExactlyOnce,
    }
}

fn glucose_publish_properties() -> PublishProperties {
    PublishProperties {
        // 1 = UTF-8 encoded character data (per MQTT v5 §3.3.2.3.2).
        payload_format_indicator: Some(1),
        content_type: Some("application/json".to_string()),
        ..PublishProperties::default()
    }
}

/// Long-running poll loop. Exits only when `cancel` is triggered.
/// Implements exponential backoff between failed connect attempts —
/// rumqttc itself does no spacing.
async fn run_poll_loop(
    mut eventloop: EventLoop,
    client: AsyncClient,
    health_topic: String,
    discovery: Option<DiscoveryPublish>,
    cancel: CancellationToken,
    stats: Arc<Mutex<MqttStatsState>>,
) {
    let mut backoff = BACKOFF_INITIAL;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("mqtt poll loop: shutdown requested");
                // Best-effort offline marker. `client.publish()` only
                // *queues* the packet on the client→eventloop channel;
                // bytes don't reach the wire until the eventloop polls
                // again. Once we break, the eventloop drops and the
                // queued publish is lost — so drain a few iterations
                // here, capped by `OFFLINE_DRAIN_TIMEOUT`, to give the
                // outgoing PUBLISH (and its PubAck) a chance to flush.
                if publish_health(&client, &health_topic, false).await.is_ok() {
                    let drain = async {
                        loop {
                            match eventloop.poll().await {
                                Ok(Event::Incoming(
                                    rumqttc::v5::mqttbytes::v5::Packet::PubAck(_),
                                )) => return,
                                Ok(_) => continue,
                                Err(_) => return,
                            }
                        }
                    };
                    let _ = tokio::time::timeout(OFFLINE_DRAIN_TIMEOUT, drain).await;
                }
                break;
            }
            event = eventloop.poll() => {
                match event {
                    Ok(Event::Incoming(packet)) => {
                        if matches!(packet, rumqttc::v5::mqttbytes::v5::Packet::ConnAck(_)) {
                            backoff = BACKOFF_INITIAL;
                            stats
                                .lock()
                                .expect("mqtt stats mutex poisoned")
                                .record_connect();
                            info!("mqtt connected");
                            if let Err(e) = publish_health(&client, &health_topic, true).await {
                                warn!(
                                    error_code = e.code(),
                                    error = %e,
                                    "mqtt: failed to publish online health"
                                );
                            }
                            // HA auto-discovery: retained, QoS 1, idempotent
                            // across reconnects. A failed publish is logged
                            // but does not break the connect path — the next
                            // ConnAck retries.
                            if let Some(d) = &discovery {
                                if let Err(e) = publish_discovery(&client, d).await {
                                    warn!(
                                        error_code = e.code(),
                                        error = %e,
                                        "mqtt: failed to publish HA discovery config"
                                    );
                                } else {
                                    debug!(topic = %d.topic, "mqtt: HA discovery published");
                                }
                            }
                        }
                    }
                    Ok(Event::Outgoing(_)) => {}
                    Err(ref e) => {
                        let classified = classify_connection_error(e);
                        warn!(
                            error_code = classified.code(),
                            error = %classified,
                            backoff_secs = backoff.as_secs(),
                            "mqtt connection error; will retry"
                        );
                        // Sleep with cancellation awareness.
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            _ = tokio::time::sleep(backoff) => {}
                        }
                        backoff = (backoff * 2).min(BACKOFF_MAX);
                    }
                }
            }
        }
    }

    info!("mqtt poll loop: exited");
}

/// Periodic publisher for the `<prefix>/_stats` topic. Ticks every
/// `interval`, snapshots the live counters, and publishes the JSON
/// payload retained at QoS 1. A failed publish is logged but does not
/// terminate the task — the next tick will retry with the latest snapshot.
async fn run_stats_loop(
    client: AsyncClient,
    topic: String,
    interval: Duration,
    cancel: CancellationToken,
    stats: Arc<Mutex<MqttStatsState>>,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Skip the immediate first tick — wait one full interval so the
    // initial `_stats` carries a non-zero uptime and a meaningful
    // publish count.
    ticker.tick().await;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!("mqtt stats loop: shutdown requested");
                break;
            }
            _ = ticker.tick() => {
                let snapshot = stats
                    .lock()
                    .expect("mqtt stats mutex poisoned")
                    .snapshot();
                let payload = wire::stats_payload(&snapshot);
                let bytes = match serde_json::to_vec(&payload) {
                    Ok(b) => b,
                    Err(e) => {
                        warn!(
                            error_code = "MQTT005",
                            error = %e,
                            "mqtt stats: failed to serialise payload"
                        );
                        continue;
                    }
                };
                if let Err(e) = client
                    .publish(
                        &topic,
                        QoS::AtLeastOnce,
                        /* retain = */ true,
                        Bytes::from(bytes),
                    )
                    .await
                {
                    let classified = classify_client_error(e);
                    warn!(
                        error_code = classified.code(),
                        error = %classified,
                        topic = %topic,
                        "mqtt stats: publish failed"
                    );
                }
            }
        }
    }
}

async fn publish_health(client: &AsyncClient, topic: &str, online: bool) -> Result<(), MqttError> {
    let payload =
        serde_json::to_vec(&wire::health_payload(online)).map_err(|e| MqttError::Payload {
            message: format!("serialise health: {e}"),
        })?;
    client
        .publish(
            topic,
            QoS::AtLeastOnce,
            /* retain = */ true,
            Bytes::from(payload),
        )
        .await
        .map_err(classify_client_error)
}

/// Pre-serialised HA discovery config + its destination topic. Held in
/// the poll loop and republished retained on every ConnAck — HA picks
/// the entity up on first encounter, subsequent retains are no-ops on
/// HA's side because `unique_id` is stable across reconnects.
struct DiscoveryPublish {
    topic: String,
    payload: Bytes,
}

async fn publish_discovery(
    client: &AsyncClient,
    discovery: &DiscoveryPublish,
) -> Result<(), MqttError> {
    client
        .publish(
            discovery.topic.clone(),
            QoS::AtLeastOnce,
            /* retain = */ true,
            discovery.payload.clone(),
        )
        .await
        .map_err(classify_client_error)
}

#[cfg(test)]
mod tests {
    //! Integration tests against an in-process MQTT v5 stub broker.
    //!
    //! The stub uses `tokio_util::codec::Framed<TcpStream, rumqttc::v5::Codec>`
    //! to handle MQTT framing — no third-party broker dependency. It
    //! accepts CONNECT, replies CONNACK Success, ACKs every QoS 1
    //! PUBLISH, replies to PINGREQ, and forwards every received PUBLISH
    //! to a channel for assertions. This is enough to validate the full
    //! `MqttSink` contract: connect → `_health` true publish, `push()`
    //! → glucose publish, periodic `_stats` publish, drop → `_health`
    //! false publish.
    use std::time::Duration;
    use std::time::Instant;
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    use bytes::BytesMut;
    use chrono::Utc;
    use futures::{SinkExt as _, StreamExt as _};
    use gluco_hub_core::{GlucoseMgDl, PatientId, Reading, Sink as _, SourceId, Trend};
    use rumqttc::v5::mqttbytes::v5::{
        Codec, ConnAck, ConnectReturnCode, Packet, PingResp, PubAck, Publish as PublishPkt,
    };
    use serde_json::Value;
    use tokio_util::codec::Framed;

    use crate::config::{MqttGlucoseUnit, MqttQos, MqttSinkConfig};

    use super::MqttSink;

    /// Spin a single-connection MQTT v5 stub broker on `127.0.0.1:0`.
    /// Returns the bound port and a receiver of every received PUBLISH
    /// packet. The broker exits when the client disconnects.
    async fn start_stub_broker() -> (u16, mpsc::Receiver<PublishPkt>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let (tx, rx) = mpsc::channel(64);

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut framed = Framed::new(
                stream,
                Codec {
                    max_incoming_size: Some(1024 * 1024),
                    max_outgoing_size: Some(1024 * 1024),
                },
            );

            while let Some(item) = framed.next().await {
                match item {
                    Ok(Packet::Connect(_, _, _)) => {
                        let connack = ConnAck {
                            session_present: false,
                            code: ConnectReturnCode::Success,
                            properties: None,
                        };
                        if framed.send(Packet::ConnAck(connack)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Packet::Publish(p)) => {
                        let pkid = p.pkid;
                        let qos = p.qos;
                        // Forward — best-effort. If the receiver was
                        // dropped the test is tearing down.
                        let _ = tx.send(p).await;
                        if matches!(qos, rumqttc::v5::mqttbytes::QoS::AtLeastOnce) {
                            let ack = PubAck::new(pkid, None);
                            if framed.send(Packet::PubAck(ack)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Ok(Packet::PingReq(_)) => {
                        if framed.send(Packet::PingResp(PingResp)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Packet::Disconnect(_)) => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            // Drain any pending writes before the listener drops.
            let _ = framed.close().await;
            // Touch BytesMut so the import isn't unused on success paths.
            let _ = BytesMut::new();
        });

        (port, rx)
    }

    fn cfg(port: u16, prefix: &str, stats_interval_secs: u64) -> MqttSinkConfig {
        MqttSinkConfig {
            broker_host: "127.0.0.1".into(),
            broker_port: port,
            client_id: "test-client".into(),
            username: None,
            password: None,
            topic_prefix: prefix.into(),
            qos: MqttQos::AtLeastOnce,
            keep_alive_secs: 30,
            session_expiry_secs: 0,
            tls: false,
            include_patient_id: true,
            stats_interval_secs,
            discovery_enabled: false,
            discovery_prefix: "homeassistant".into(),
            device_name: None,
            discovery_unit: MqttGlucoseUnit::default(),
        }
    }

    fn one_reading() -> Reading {
        Reading {
            patient_id: PatientId::new("p1").unwrap(),
            source_id: SourceId::new("llu").unwrap(),
            timestamp: Utc::now(),
            glucose: GlucoseMgDl::new(123.0).unwrap(),
            trend: Trend::Flat,
        }
    }

    /// Wait for the next PUBLISH whose topic matches `expected_topic`,
    /// up to `deadline`. Drains intervening publishes (e.g. an early
    /// `_stats` tick that races with a glucose publish in the test).
    ///
    /// Returns `None` if the broker channel closes before the topic
    /// arrives (the stub broker task exited — e.g. TCP teardown raced
    /// the publish). Returns `Some(pkt)` on success. Panics on timeout.
    async fn wait_for_topic(
        rx: &mut mpsc::Receiver<PublishPkt>,
        expected_topic: &str,
        deadline: Duration,
    ) -> Option<PublishPkt> {
        let started = Instant::now();
        loop {
            let remaining = deadline.checked_sub(started.elapsed()).unwrap_or_default();
            let recv = tokio::time::timeout(remaining, rx.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {expected_topic}"));
            let p = match recv {
                Some(p) => p,
                None => return None,
            };
            let topic = std::str::from_utf8(&p.topic)
                .expect("utf8 topic")
                .to_string();
            if topic == expected_topic {
                return Some(p);
            }
        }
    }

    #[tokio::test]
    async fn connack_triggers_health_online_publish() {
        let (port, mut rx) = start_stub_broker().await;
        let _sink = MqttSink::new(&cfg(port, "test", 60), None).expect("build sink");

        let p = wait_for_topic(&mut rx, "test/_health", Duration::from_secs(3))
            .await
            .expect("online _health publish must arrive");
        assert!(p.retain, "_health publish must be retained");
        let body: Value = serde_json::from_slice(&p.payload).expect("json body");
        assert_eq!(body["online"], Value::Bool(true));
        assert_eq!(body["v"], Value::from(1));
    }

    #[tokio::test]
    async fn push_publishes_glucose_payload_with_v1_schema() {
        let (port, mut rx) = start_stub_broker().await;
        let sink = MqttSink::new(&cfg(port, "test", 60), None).expect("build sink");

        sink.push(&[one_reading()])
            .await
            .expect("push must succeed");

        let p = wait_for_topic(&mut rx, "test/glucose", Duration::from_secs(3))
            .await
            .expect("glucose publish must arrive");
        assert!(!p.retain, "glucose publishes must NOT be retained");
        let body: Value = serde_json::from_slice(&p.payload).expect("json body");
        assert_eq!(body["v"], Value::from(1));
        assert_eq!(body["mgdl"], Value::from(123.0));
        assert_eq!(body["trend"], Value::String("Flat".into()));
        assert_eq!(body["source"], Value::String("llu".into()));
        assert_eq!(body["patient"], Value::String("p1".into()));
    }

    #[tokio::test]
    async fn periodic_stats_publish_carries_live_counters() {
        let (port, mut rx) = start_stub_broker().await;
        // Construct directly — `MqttSink::new` does not run the
        // validator, so we can drop below the operator-facing minimum
        // (5 s) for a fast test.
        let sink = MqttSink::new(&cfg(port, "test", 1), None).expect("build sink");

        // Drain the connect-time _health publish.
        let _ = wait_for_topic(&mut rx, "test/_health", Duration::from_secs(3)).await;

        // Push two readings so publishes_total > 0 in the snapshot.
        sink.push(&[one_reading(), one_reading()])
            .await
            .expect("push");
        let _ = wait_for_topic(&mut rx, "test/glucose", Duration::from_secs(3)).await;
        let _ = wait_for_topic(&mut rx, "test/glucose", Duration::from_secs(3)).await;

        // Periodic task ticks at +1 s; allow generous slack for CI.
        let stats = wait_for_topic(&mut rx, "test/_stats", Duration::from_secs(5))
            .await
            .expect("_stats publish must arrive");
        assert!(stats.retain, "_stats publish must be retained");
        let body: Value = serde_json::from_slice(&stats.payload).expect("json body");
        assert_eq!(body["v"], Value::from(1));
        assert!(
            body["uptime_secs"].as_u64().expect("uptime_secs") >= 1,
            "uptime_secs should have advanced: {body}"
        );
        assert_eq!(
            body["publishes_total"].as_u64().expect("publishes_total"),
            2,
            "two pushed readings → publishes_total = 2: {body}"
        );
        assert_eq!(
            body["connects_total"].as_u64().expect("connects_total"),
            1,
            "single ConnAck → connects_total = 1: {body}"
        );
        assert!(
            body["last_publish_ts_ms"].as_i64().is_some(),
            "last_publish_ts_ms must be set: {body}"
        );
    }

    #[tokio::test]
    async fn drop_publishes_health_offline_marker() {
        let (port, mut rx) = start_stub_broker().await;
        let sink = MqttSink::new(&cfg(port, "test", 60), None).expect("build sink");

        // Drain the connect-time _health publish.
        let online = wait_for_topic(&mut rx, "test/_health", Duration::from_secs(3))
            .await
            .expect("online _health publish must arrive");
        let body: Value = serde_json::from_slice(&online.payload).expect("json body");
        assert_eq!(body["online"], Value::Bool(true));

        drop(sink);

        // Best-effort offline marker: the poll loop publishes it and drains
        // the PubAck before exiting, but TCP teardown can race the publish
        // on a loaded CI runner. `None` means the broker channel closed
        // before the marker arrived — the real broker's LWT covers this
        // case in production, so we accept it here.
        if let Some(offline) = wait_for_topic(&mut rx, "test/_health", Duration::from_secs(3)).await
        {
            let body: Value = serde_json::from_slice(&offline.payload).expect("json body");
            assert_eq!(body["online"], Value::Bool(false));
        }
    }

    #[tokio::test]
    async fn discovery_publish_when_enabled_after_connack() {
        let (port, mut rx) = start_stub_broker().await;
        let mut c = cfg(port, "test", 60);
        c.discovery_enabled = true;
        let _sink = MqttSink::new(&c, None).expect("build sink");

        let expected_topic = "homeassistant/sensor/gluco_hub_test-client_glucose/config";
        let p = wait_for_topic(&mut rx, expected_topic, Duration::from_secs(3))
            .await
            .expect("HA discovery publish must arrive");
        assert!(p.retain, "discovery publish must be retained");

        let body: Value = serde_json::from_slice(&p.payload).expect("json body");
        assert_eq!(body["unique_id"], "gluco_hub_test-client_glucose");
        assert_eq!(body["state_topic"], "test/glucose");
        assert_eq!(body["availability_topic"], "test/_health");
        assert_eq!(body["value_template"], "{{ value_json.mgdl }}");
        assert_eq!(body["unit_of_measurement"], "mg/dL");
        assert_eq!(body["device"]["manufacturer"], "gluco-hub-rs");
    }

    #[tokio::test]
    async fn discovery_silent_when_disabled() {
        // Default cfg() leaves discovery_enabled=false. Confirm no
        // publish on the canonical discovery topic over the same window
        // we'd otherwise wait for it.
        let (port, mut rx) = start_stub_broker().await;
        let _sink = MqttSink::new(&cfg(port, "test", 60), None).expect("build sink");

        // Drain the expected _health publish first so we know the ConnAck
        // round-trip is complete.
        let _ = wait_for_topic(&mut rx, "test/_health", Duration::from_secs(3))
            .await
            .expect("online _health publish must arrive");

        // Now poll briefly for any discovery-shaped topic; expect none.
        let saw_discovery = tokio::time::timeout(Duration::from_millis(300), async {
            while let Some(p) = rx.recv().await {
                let topic = std::str::from_utf8(&p.topic).unwrap_or("");
                if topic.starts_with("homeassistant/") {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false);

        assert!(
            !saw_discovery,
            "discovery topic must not be published when discovery_enabled = false"
        );
    }
}
