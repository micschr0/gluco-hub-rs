// SPDX-License-Identifier: AGPL-3.0-or-later

//! Eclipse Mosquitto v5 broker for integration tests.
//!
//! `eclipse-mosquitto:2` ships with `allow_anonymous false` by default,
//! so we mount a minimal `mosquitto.conf` via a temp dir to enable a
//! single anonymous listener on 1883.
//!
//! Lifecycle: each test that wants a broker calls [`start_mosquitto`].
//! testcontainers cleans up the container when the returned guard is
//! dropped — usually at test end. Mosquitto starts in <1s so each test
//! gets its own instance for clean retained-message and LWT semantics.

use std::io::Write;
use tempfile::TempDir;
use testcontainers::core::{IntoContainerPort, Mount, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

const MQTT_PORT: u16 = 1883;

/// Wrapper around a running Mosquitto container plus the temp dir
/// holding its config. The temp dir lives as long as the container so
/// the mounted file stays valid.
pub struct Mosquitto {
    container: ContainerAsync<GenericImage>,
    host: String,
    port: u16,
    _config_dir: TempDir,
}

impl Mosquitto {
    /// `<host>, <port>` ready to drop into `MqttSinkConfig`.
    pub fn broker_addr(&self) -> (String, u16) {
        (self.host.clone(), self.port)
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Stop the broker mid-flight (e.g. to simulate an outage and
    /// verify DLQ behaviour from above).
    pub async fn stop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.container.stop().await?;
        Ok(())
    }
}

/// Start a fresh, anonymous-allowed Mosquitto broker.
pub async fn start_mosquitto() -> Result<Mosquitto, Box<dyn std::error::Error + Send + Sync>> {
    let dir = tempfile::tempdir()?;
    let conf_path = dir.path().join("mosquitto.conf");
    {
        let mut f = std::fs::File::create(&conf_path)?;
        writeln!(f, "listener {MQTT_PORT}")?;
        writeln!(f, "allow_anonymous true")?;
        writeln!(f, "persistence false")?;
        writeln!(f, "log_dest stdout")?;
    }

    let host_conf = conf_path
        .to_str()
        .ok_or("non-UTF8 path for mosquitto.conf")?
        .to_string();

    let image = GenericImage::new("eclipse-mosquitto", "2")
        .with_exposed_port(MQTT_PORT.tcp())
        .with_wait_for(WaitFor::message_on_stdout("mosquitto version 2."));

    let container = image
        .with_mount(Mount::bind_mount(
            host_conf,
            "/mosquitto/config/mosquitto.conf",
        ))
        .start()
        .await?;

    let host = container.get_host().await?.to_string();
    let port = container.get_host_port_ipv4(MQTT_PORT.tcp()).await?;

    Ok(Mosquitto {
        container,
        host,
        port,
        _config_dir: dir,
    })
}

/// Captured PUBLISH packet — the subset of fields tests assert on.
#[derive(Debug, Clone)]
pub struct CapturedPublish {
    pub topic: String,
    pub payload: Vec<u8>,
    pub retain: bool,
}

/// Subscribe to `topic_filter` on the broker, drive the rumqttc event
/// loop in a tokio task, and capture every matching PUBLISH into a
/// channel.
pub async fn subscribe_to(
    mqtt: &Mosquitto,
    topic_filter: &str,
    client_id_prefix: &str,
) -> Result<
    (
        tokio::sync::mpsc::Receiver<CapturedPublish>,
        tokio_util::sync::CancellationToken,
    ),
    Box<dyn std::error::Error + Send + Sync>,
> {
    use rumqttc::v5::mqttbytes::QoS;
    use rumqttc::v5::{AsyncClient, Event, MqttOptions};

    let client_id = super::unique_id(client_id_prefix);
    let mut opts = MqttOptions::new(client_id, mqtt.host(), mqtt.port());
    opts.set_keep_alive(std::time::Duration::from_secs(5));

    let (client, mut eventloop) = AsyncClient::new(opts, 16);
    client
        .subscribe(topic_filter.to_string(), QoS::AtLeastOnce)
        .await?;

    let (tx, rx) = tokio::sync::mpsc::channel::<CapturedPublish>(64);
    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_for_task = cancel.clone();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel_for_task.cancelled() => return,
                event = eventloop.poll() => match event {
                    Ok(Event::Incoming(packet)) => {
                        use rumqttc::v5::mqttbytes::v5::Packet;
                        if let Packet::Publish(p) = packet {
                            let cap = CapturedPublish {
                                topic: std::str::from_utf8(&p.topic)
                                    .unwrap_or("")
                                    .to_string(),
                                payload: p.payload.to_vec(),
                                retain: p.retain,
                            };
                            if tx.send(cap).await.is_err() {
                                return;
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(_) => return,
                },
            }
        }
    });

    Ok((rx, cancel))
}

/// Wait for the next matching PUBLISH on `expected_topic` within
/// `deadline`. Drains intervening publishes that don't match. Panics on
/// timeout.
pub async fn wait_for_topic(
    rx: &mut tokio::sync::mpsc::Receiver<CapturedPublish>,
    expected_topic: &str,
    deadline: std::time::Duration,
) -> Option<CapturedPublish> {
    let start = std::time::Instant::now();
    loop {
        let remaining = deadline.checked_sub(start.elapsed()).unwrap_or_default();
        let recv = tokio::time::timeout(remaining, rx.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for topic {expected_topic}"));
        let pkt = match recv {
            Some(p) => p,
            None => return None,
        };
        if pkt.topic == expected_topic {
            return Some(pkt);
        }
    }
}
