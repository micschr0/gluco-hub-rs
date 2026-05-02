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
//! * Shutdown via `CancellationToken`: on Drop the token is cancelled,
//!   the poll task best-effort publishes `online: false`, and exits.

use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use cgm_bridge_core::{CoreError, Reading, Sink};
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
use super::wire;

/// Reconnect backoff bounds. Initial 1 s doubles up to `MAX` between
/// failed `EventLoop::poll()` calls — rumqttc itself does not space
/// reconnect attempts.
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Internal channel capacity between `AsyncClient::publish()` and the
/// EventLoop. CGM produces ≤ 1 reading/minute; 16 is generous.
const CLIENT_CHANNEL_CAPACITY: usize = 16;

/// V2 MQTT sink. Construction connects (asynchronously) and spawns the
/// EventLoop poll task; readings published via `push()` go through the
/// client's bounded channel.
pub struct MqttSink {
    client: AsyncClient,
    topic_glucose: String,
    qos: QoS,
    include_patient: bool,
    cancel: CancellationToken,
    _poll_task: JoinHandle<()>,
}

impl MqttSink {
    /// Build the sink and spawn the poll task. Returns immediately —
    /// the actual TCP/TLS connect happens asynchronously inside the
    /// poll task; transient connect failures surface via metrics
    /// (`error_code = "MQTT001"` etc.) and the warn-level reconnect
    /// log lines.
    pub fn new(cfg: &MqttSinkConfig, password: Option<SecretString>) -> Result<Self, MqttError> {
        let (client, eventloop) = build_client(cfg, password)?;
        let cancel = CancellationToken::new();

        let poll_cancel = cancel.clone();
        let poll_client = client.clone();
        let health_topic = format!("{}/_health", cfg.topic_prefix);

        let poll_task = tokio::spawn(async move {
            run_poll_loop(eventloop, poll_client, health_topic, poll_cancel).await;
        });

        Ok(Self {
            client,
            topic_glucose: format!("{}/glucose", cfg.topic_prefix),
            qos: mqtt_qos(cfg.qos),
            include_patient: cfg.include_patient_id,
            cancel,
            _poll_task: poll_task,
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

        for reading in readings {
            let payload = wire::glucose_payload(reading, self.include_patient);
            let bytes = serde_json::to_vec(&payload).map_err(|e| CoreError::Sink {
                message: MqttError::Payload {
                    message: format!("serialise reading: {e}"),
                }
                .to_string(),
            })?;

            self.client
                .publish_with_properties(
                    self.topic_glucose.clone(),
                    self.qos,
                    /* retain = */ false,
                    bytes,
                    props.clone(),
                )
                .await
                .map_err(|e| CoreError::Sink {
                    message: classify_client_error(e).to_string(),
                })?;
        }

        debug!(
            count = readings.len(),
            topic = %self.topic_glucose,
            "mqtt batch published"
        );

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// internal builders / helpers
// ---------------------------------------------------------------------------

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
    cancel: CancellationToken,
) {
    let mut backoff = BACKOFF_INITIAL;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("mqtt poll loop: shutdown requested");
                // Best-effort offline marker — ignore errors, the
                // broker will see the LWT either way.
                let _ = publish_health(&client, &health_topic, false).await;
                break;
            }
            event = eventloop.poll() => {
                match event {
                    Ok(Event::Incoming(packet)) => {
                        if matches!(packet, rumqttc::v5::mqttbytes::v5::Packet::ConnAck(_)) {
                            backoff = BACKOFF_INITIAL;
                            info!("mqtt connected");
                            if let Err(e) = publish_health(&client, &health_topic, true).await {
                                warn!(
                                    error_code = e.code(),
                                    error = %e,
                                    "mqtt: failed to publish online health"
                                );
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
