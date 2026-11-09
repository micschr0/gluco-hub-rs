// SPDX-License-Identifier: AGPL-3.0-or-later

//! Exercise configuration, destination construction, pollers and real sink output.
use super::*;
use chrono::{TimeZone, Utc};
use futures::{SinkExt, StreamExt};
use gluco_hub_core::{CoreError, GlucoseMgDl, PatientId, SourceId, Trend};
use rumqttc::v5::mqttbytes::v5::{Codec, ConnAck, ConnectReturnCode, Packet, PubAck, Publish};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::codec::Framed;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Broker {
    port: u16,
    packets: mpsc::Receiver<Publish>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Broker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Broker {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, packets) = mpsc::channel(128);
        let task = tokio::spawn(async move {
            let mut clients = JoinSet::new();
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let tx = tx.clone();
                clients.spawn(async move {
                    let mut framed = Framed::new(
                        stream,
                        Codec {
                            max_incoming_size: None,
                            max_outgoing_size: None,
                        },
                    );
                    while let Some(packet) = framed.next().await {
                        match packet.unwrap() {
                            Packet::Connect(_, _, _) => framed
                                .send(Packet::ConnAck(ConnAck {
                                    session_present: false,
                                    code: ConnectReturnCode::Success,
                                    properties: None,
                                }))
                                .await
                                .unwrap(),
                            Packet::Publish(p) => {
                                let pkid = p.pkid;
                                tx.send(p).await.unwrap();
                                framed
                                    .send(Packet::PubAck(PubAck::new(pkid, None)))
                                    .await
                                    .unwrap();
                            }
                            Packet::Disconnect(_) => break,
                            _ => {}
                        }
                    }
                });
            }
        });
        Self {
            port,
            packets,
            task,
        }
    }

    async fn publication(&mut self, suffix: &str) -> (String, Value) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let packet = self.packets.recv().await.expect("broker still running");
                let topic = String::from_utf8(packet.topic.to_vec()).unwrap();
                if topic.ends_with(suffix) {
                    return (topic, serde_json::from_slice(&packet.payload).unwrap());
                }
            }
        })
        .await
        .expect("MQTT publication must arrive")
    }
}

struct FixedSource(Reading);

#[async_trait::async_trait]
impl Source for FixedSource {
    fn id(&self) -> &SourceId {
        &self.0.source_id
    }

    async fn fetch_latest(&self) -> Result<Vec<Reading>, CoreError> {
        Ok(vec![self.0.clone()])
    }
}

async fn poll_once(name: &str, sinks: Vec<Arc<sink_router::SinkRouter>>, secs: i64, mgdl: f64) {
    // A configured name need not equal the Source ID (default/mock fixture).
    let source = Arc::new(FixedSource(Reading {
        source_id: SourceId::new(format!("sensor-{name}")).unwrap(),
        patient_id: PatientId::new(format!("patient-{name}")).unwrap(),
        timestamp: Utc.timestamp_opt(secs, 0).unwrap(),
        glucose: GlucoseMgDl::new(mgdl).unwrap(),
        trend: Trend::Flat,
    }));
    let (status, mut updates) = tokio::sync::watch::channel(poll_status::PollStatus::default());
    let (tx, _rx) = tokio::sync::broadcast::channel(16);
    let poller = tokio::spawn(poll_loop_single(
        name.into(),
        source,
        ReadingCache::new(),
        sinks,
        Duration::from_secs(60),
        None,
        Arc::new(status),
        api::ClockSink {
            tx: Arc::new(tx),
            history: api::new_history(),
        },
    ));
    let result = tokio::time::timeout(Duration::from_secs(5), updates.changed()).await;
    poller.abort();
    result
        .expect("poll cycle completes")
        .expect("poll status sent");
}

async fn check_routing(per_source: bool, dlq: bool) {
    let mut broker = Broker::start().await;
    let ns = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/entries"))
        .respond_with(ResponseTemplate::new(201))
        .expect(2)
        .mount(&ns)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
[state]
dir = "{}"
[dlq]
enabled = {dlq}
[sink.mqtt]
broker_host = "127.0.0.1"
broker_port = {}
client_id = "routing-test"
topic_prefix = "test"
tls = false
per_source = {per_source}
discovery_enabled = true
"#,
            dir.path().join("state").display(),
            broker.port
        ),
    )
    .unwrap();
    let mut cfg = config::load(Some(&config_path)).unwrap();
    cfg.sink.nightscout = Some(config::NightscoutSinkConfig {
        base_url: ns.uri(),
        api_secret: secrecy::SecretString::from("test-secret"),
        device: None,
        app: None,
    });
    let sinks = build_sinks(
        &cfg,
        &["a".into(), "b".into()],
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicU64::new(0)),
    )
    .await
    .unwrap();

    let (_, first) = broker.publication("_glucose/config").await;
    if per_source {
        let (_, second) = broker.publication("_glucose/config").await;
        let mut state_topics = vec![
            first["state_topic"].as_str().unwrap(),
            second["state_topic"].as_str().unwrap(),
        ];
        state_topics.sort();
        assert_eq!(state_topics, ["test/a/glucose", "test/b/glucose"]);
    } else {
        assert_eq!(first["state_topic"], "test/glucose");
    }

    poll_once("a", sinks.clone(), 1_700_000_100, 110.0).await;
    let (topic, body) = broker.publication("/glucose").await;
    assert_eq!(
        topic,
        if per_source {
            "test/a/glucose"
        } else {
            "test/glucose"
        }
    );
    assert_eq!(body["source"], "sensor-a");
    assert_eq!(body["mgdl"], 110.0);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), broker.publication("/glucose"))
            .await
            .is_err(),
        "A must not publish to B's topic"
    );

    // B's older but fresh reading must reach shared sinks despite A's watermark.
    poll_once("b", sinks, 1_700_000_000, 220.0).await;
    let (topic, body) = broker.publication("/glucose").await;
    assert_eq!(
        topic,
        if per_source {
            "test/b/glucose"
        } else {
            "test/glucose"
        }
    );
    assert_eq!(body["source"], "sensor-b");
    assert_eq!(body["mgdl"], 220.0);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), broker.publication("/glucose"))
            .await
            .is_err(),
        "B must not publish to A's topic"
    );

    ns.verify().await;
    let requests = ns.received_requests().await.unwrap();
    let bodies: Vec<Value> = requests
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(bodies[0][0]["sgv"], 110);
    assert_eq!(bodies[1][0]["sgv"], 220);
}

#[tokio::test]
async fn per_source_topics_only_receive_their_source() {
    check_routing(true, false).await;
    check_routing(true, true).await;
}

#[tokio::test]
async fn shared_destinations_receive_both_sources() {
    check_routing(false, false).await;
    check_routing(false, true).await;
}

#[tokio::test]
async fn legacy_queue_is_not_replayed_to_a_per_source_topic() {
    let mut broker = Broker::start().await;
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    std::fs::create_dir_all(state.join("dlq")).unwrap();
    let old = Reading {
        source_id: SourceId::new("sensor-a").unwrap(),
        patient_id: PatientId::new("patient-a").unwrap(),
        timestamp: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        glucose: GlucoseMgDl::new(110.0).unwrap(),
        trend: Trend::Flat,
    };
    let legacy = state.join("dlq/mqtt.jsonl");
    let contents = format!("{}\n", serde_json::json!({"v": 1, "reading": old}));
    std::fs::write(&legacy, &contents).unwrap();
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
[state]
dir = "{}"
[dlq]
enabled = true
[sink.mqtt]
broker_host = "127.0.0.1"
broker_port = {}
client_id = "legacy-test"
topic_prefix = "test"
tls = false
per_source = true
"#,
            state.display(),
            broker.port
        ),
    )
    .unwrap();
    let cfg = config::load(Some(&config_path)).unwrap();
    let sinks = build_sinks(
        &cfg,
        &["a".into(), "b".into()],
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicU64::new(0)),
    )
    .await
    .unwrap();
    poll_once("b", sinks, 1_700_000_100, 220.0).await;
    let (topic, body) = broker.publication("/glucose").await;
    assert_eq!(topic, "test/b/glucose");
    assert_eq!(body["source"], "sensor-b", "legacy A must not reach B");
    assert_eq!(std::fs::read_to_string(legacy).unwrap(), contents);
}
