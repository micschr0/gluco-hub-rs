// SPDX-License-Identifier: AGPL-3.0-or-later

//! End-to-end pipeline tests.
//!
//! These run the full LibreLink Up → cache → Nightscout flow against a
//! single in-process `wiremock` server that hosts every endpoint the
//! bridge talks to: `/llu/auth/login`, `/llu/connections`,
//! `/llu/connections/{patient_id}/graph`, and `/api/v3/entries`. They
//! prove that:
//!
//! 1. The four call sequence happens in the right order.
//! 2. Every request carries the right headers: LLU `version`, `product`,
//!    `account-id` (`sha256(userId)`), Bearer; NS `api-secret`
//!    (`sha1_hex(API_SECRET)`).
//! 3. The Nightscout JSON body has the expected field set — `date`,
//!    `sgv`, `direction`, `type: "sgv"`, `device`, `app` — and **no
//!    spurious `trend` integer**.
//! 4. The `ReadingCache` ends up holding the newest reading from the
//!    graph response.

use std::sync::Arc;
use std::time::Duration;

use gluco_hub_core::{PatientId, ReadingCache, Sink, Source, SourceId};
use secrecy::SecretString;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::sinks::nightscout::{NightscoutClient, NightscoutSink};
use crate::sources::llu::Region;
use crate::sources::llu::auth::{LluAuthClient, LluCredentials, account_id_hash};
use crate::sources::llu::source::{ConnectionSelection, LluSource};

/// LLU response shape — login, connections, graph endpoints all plumbed
/// in one server.
fn login_body() -> serde_json::Value {
    serde_json::json!({
        "status": 0,
        "data": {
            "authTicket": {
                "token": "tok-e2e",
                "expires": 9_999_999_999u64,
                "duration": 3600u64
            },
            "user": { "id": "user-42" }
        }
    })
}

fn connections_body() -> serde_json::Value {
    serde_json::json!({
        "status": 0,
        "data": [
            {
                "id": "conn-1",
                "patientId": "patient-42",
                "firstName": "Ignored",
                "lastName": "Ignored",
                "glucoseMeasurement": {
                    "Timestamp": "3/26/2024 4:38:38 PM",
                    "ValueInMgPerDl": 142.0,
                    "TrendArrow": 3
                }
            }
        ]
    })
}

fn graph_body() -> serde_json::Value {
    serde_json::json!({
        "status": 0,
        "data": {
            "connection": { "id": "conn-1", "patientId": "patient-42" },
            "activeSensors": [],
            "graphData": [
                { "Timestamp": "3/26/2024 4:33:38 PM", "ValueInMgPerDl": 138.0, "TrendArrow": 3 },
                { "Timestamp": "3/26/2024 4:38:38 PM", "ValueInMgPerDl": 142.0, "TrendArrow": 3 }
            ]
        }
    })
}

async fn mount_llu(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/llu/auth/login"))
        .and(header("product", "llu.ios"))
        .and(header("version", "4.17.0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(login_body()))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/llu/connections"))
        .and(header("authorization", "Bearer tok-e2e"))
        .and(header("account-id", account_id_hash("user-42").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(connections_body()))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/llu/connections/patient-42/graph"))
        .and(header("authorization", "Bearer tok-e2e"))
        .and(header("account-id", account_id_hash("user-42").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(graph_body()))
        .mount(server)
        .await;
}

async fn mount_nightscout(server: &MockServer) {
    // sha1("e2e-secret") = 631a0d6c3813ee3a11e19b0a37a10ad75bbe8a0c
    // The sink calls `GET /api/v3/entries?count=1` first to read the
    // high-water mark; a 404 means "no prior entries, post everything".
    Mock::given(method("GET"))
        .and(path("/api/v3/entries"))
        .and(header(
            "api-secret",
            "631a0d6c3813ee3a11e19b0a37a10ad75bbe8a0c",
        ))
        .respond_with(ResponseTemplate::new(404))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v3/entries"))
        .and(header(
            "api-secret",
            "631a0d6c3813ee3a11e19b0a37a10ad75bbe8a0c",
        ))
        .respond_with(ResponseTemplate::new(201))
        .mount(server)
        .await;
}

#[tokio::test]
async fn full_pipeline_pulls_from_llu_and_pushes_to_nightscout() {
    let server = MockServer::start().await;
    mount_llu(&server).await;
    mount_nightscout(&server).await;

    // --- Build the source: real LluSource pointing at the wiremock URL.
    let llu = LluAuthClient::new()
        .expect("client")
        .with_base_url(server.uri());
    let source: Arc<dyn Source> = Arc::new(LluSource::new(
        SourceId::new("llu").unwrap(),
        llu,
        LluCredentials {
            email: "patient@example.com".to_string(),
            password: SecretString::from("hunter2"),
            region: Region::Eu,
        },
        ConnectionSelection::ByPatientId(PatientId::new("patient-42").unwrap()),
        chrono_tz::Tz::UTC,
    ));

    // --- Build the sink: real NightscoutSink pointing at the same wiremock.
    let ns_client = NightscoutClient::new(server.uri(), SecretString::from("e2e-secret"))
        .expect("ns client")
        .with_device("cgm-bridge")
        .with_app("cgm-bridge");
    let sink: Arc<dyn Sink> = Arc::new(NightscoutSink::new(ns_client));

    // --- Run one tick: source → cache → sink.
    let cache = ReadingCache::new();
    let batch = source.fetch_latest().await.expect("fetch_latest");
    assert_eq!(batch.len(), 2, "expected 2 graph readings");
    assert!(
        batch[0].timestamp < batch[1].timestamp,
        "readings must be sorted oldest first"
    );

    cache.update(&batch);
    let cached = cache.latest().expect("cache populated");
    assert_eq!(cached.glucose.get(), 142.0);
    assert_eq!(cached.patient_id.as_str(), "patient-42");

    sink.push(&batch).await.expect("ns push");

    // --- Verify every endpoint was hit exactly once and the bodies match.
    let requests = server.received_requests().await.expect("requests");

    let logins = requests
        .iter()
        .filter(|r| r.url.path() == "/llu/auth/login")
        .count();
    let connections = requests
        .iter()
        .filter(|r| r.url.path() == "/llu/connections")
        .count();
    let graphs = requests
        .iter()
        .filter(|r| r.url.path() == "/llu/connections/patient-42/graph")
        .count();
    // Dedup adds a GET hit before the POST; assert each verb separately.
    let entries_get = requests
        .iter()
        .filter(|r| r.method.as_str() == "GET" && r.url.path() == "/api/v3/entries")
        .count();
    let entries_post = requests
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == "/api/v3/entries")
        .count();
    assert_eq!(logins, 1, "exactly one /auth/login");
    assert_eq!(connections, 1, "exactly one /connections");
    assert_eq!(graphs, 1, "exactly one /graph");
    assert_eq!(entries_get, 1, "exactly one GET /api/v3/entries (dedup)");
    assert_eq!(entries_post, 1, "exactly one POST /api/v3/entries");

    // --- Inspect the NS request body in detail.
    let ns_req = requests
        .iter()
        .find(|r| r.method.as_str() == "POST" && r.url.path() == "/api/v3/entries")
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&ns_req.body).expect("ns body json");
    let arr = body.as_array().expect("array");
    assert_eq!(arr.len(), 2);
    for entry in arr {
        assert_eq!(entry["type"], "sgv", "type must be \"sgv\"");
        assert_eq!(entry["direction"], "Flat");
        assert_eq!(entry["device"], "cgm-bridge");
        assert_eq!(entry["app"], "cgm-bridge");
        assert!(entry["sgv"].is_i64());
        assert!(entry["date"].is_i64());
        assert!(entry.get("trend").is_none(), "no numeric trend field");
        assert!(entry.get("dateString").is_none(), "no dateString");
        assert!(entry.get("identifier").is_none(), "no identifier");
    }
    // Spot-check the newest entry's value rounded to integer per NS
    // convention (142.0 → 142).
    assert!(arr.iter().any(|e| e["sgv"] == 142));

    // --- Inspect the LLU login body to confirm we sent both creds.
    let login_req = requests
        .iter()
        .find(|r| r.url.path() == "/llu/auth/login")
        .unwrap();
    let login_body: serde_json::Value =
        serde_json::from_slice(&login_req.body).expect("login body json");
    assert_eq!(login_body["email"], "patient@example.com");
    assert_eq!(login_body["password"], "hunter2");
}

#[tokio::test]
async fn full_pipeline_survives_nightscout_502_and_keeps_cache_fresh() {
    let server = MockServer::start().await;
    mount_llu(&server).await;
    Mock::given(method("POST"))
        .and(path("/api/v3/entries"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;

    let llu = LluAuthClient::new()
        .expect("client")
        .with_base_url(server.uri());
    let source: Arc<dyn Source> = Arc::new(LluSource::new(
        SourceId::new("llu").unwrap(),
        llu,
        LluCredentials {
            email: "patient@example.com".to_string(),
            password: SecretString::from("hunter2"),
            region: Region::Eu,
        },
        ConnectionSelection::ByPatientId(PatientId::new("patient-42").unwrap()),
        chrono_tz::Tz::UTC,
    ));

    let ns_client =
        NightscoutClient::new(server.uri(), SecretString::from("e2e-secret")).expect("ns client");
    let sink: Arc<dyn Sink> = Arc::new(NightscoutSink::new(ns_client));

    // The source path succeeds; the cache is updated; the sink push
    // returns an error which `fan_out_to_sinks` would absorb.
    let cache = ReadingCache::new();
    let batch = source.fetch_latest().await.expect("fetch_latest");
    cache.update(&batch);
    assert_eq!(cache.latest().unwrap().glucose.get(), 142.0);

    let router = std::sync::Arc::new(crate::sink_router::SinkRouter::new(sink));
    crate::fan_out_to_sinks(&[router], &batch, Duration::from_secs(2)).await;

    // Cache state must be unchanged after the failed sink push — the
    // poll loop does NOT roll back on sink errors.
    assert_eq!(cache.latest().unwrap().glucose.get(), 142.0);
}

/// Regression lock for iteration 12 — `with_version` was once removed
/// because nothing called it; the resulting silent pin to
/// `DEFAULT_LLU_VERSION` would block every operator the moment LibreView
/// rotates the accepted app version. This test fails loudly if any
/// future refactor breaks the `[source.llu] version` → `version` header
/// wiring.
#[tokio::test]
async fn custom_version_propagates_to_login_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/llu/auth/login"))
        // Strict matcher: anything other than "9.9.9" fails the test.
        .and(header("version", "9.9.9"))
        .and(header("product", "llu.ios"))
        .respond_with(ResponseTemplate::new(200).set_body_json(login_body()))
        .mount(&server)
        .await;

    let client = LluAuthClient::new()
        .expect("client")
        .with_base_url(server.uri())
        .with_version("9.9.9");
    let creds = LluCredentials {
        email: "patient@example.com".to_string(),
        password: SecretString::from("hunter2"),
        region: Region::Eu,
    };
    let tokens = client.login(&creds).await.expect("login");
    assert_eq!(
        secrecy::ExposeSecret::expose_secret(&tokens.bearer),
        "tok-e2e"
    );
}

// ─── DLQ E2E tests ───────────────────────────────────────────────────────
//
// These tests exercise the full layered stack:
//   `SinkRouter` (watermark) → `DlqSink` (persistence) → `NightscoutSink`
// against a wiremock NS server. Readings are constructed in-process so we
// can vary timestamps precisely and don't depend on the LLU graph fixture.

mod dlq_e2e {
    use std::sync::Arc;
    use std::time::Duration;

    use chrono::{TimeZone, Utc};
    use gluco_hub_core::{GlucoseMgDl, PatientId, Reading, Sink, SourceId, Trend};
    use secrecy::SecretString;
    use tempfile::TempDir;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::dlq::DlqSink;
    use crate::sink_router::SinkRouter;
    use crate::sinks::nightscout::{NightscoutClient, NightscoutSink};

    /// sha1("e2e-secret") — must match the header wiremock asserts on.
    /// Lifted verbatim from `mount_nightscout` so all tests in this
    /// file use the same secret.
    const API_SECRET_SHA1: &str = "631a0d6c3813ee3a11e19b0a37a10ad75bbe8a0c";

    fn reading_at(ts_secs: i64, mgdl: f64) -> Reading {
        Reading {
            patient_id: PatientId::new("p1").unwrap(),
            source_id: SourceId::new("llu").unwrap(),
            timestamp: Utc
                .timestamp_opt(ts_secs, 0)
                .single()
                .expect("ts_secs is a valid non-ambiguous UTC timestamp"),
            glucose: GlucoseMgDl::new(mgdl).unwrap(),
            trend: Trend::Flat,
        }
    }

    /// Mount the read-side `GET /api/v3/entries` (always 404 = "NS empty,
    /// post everything"). Tests mount the POST mock separately so each
    /// test can control success/failure on its own.
    async fn mount_ns_get_empty(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/api/v3/entries"))
            .and(header("api-secret", API_SECRET_SHA1))
            .respond_with(ResponseTemplate::new(404))
            .named("GET /api/v3/entries always-404")
            .mount(server)
            .await;
    }

    /// Build the production-shape layered sink stack pointing at
    /// `server`'s URL and using `state_dir` for DLQ persistence.
    /// Returns the `SinkRouter` (what `fan_out_to_sinks` expects); the
    /// DLQ file path inside `state_dir` is `dlq/nightscout.jsonl`
    /// (derived from `NightscoutSink::name()`).
    fn build_layered_sink(
        server: &MockServer,
        state_dir: &std::path::Path,
        max_entries: usize,
    ) -> Arc<SinkRouter> {
        let ns_client = NightscoutClient::new(server.uri(), SecretString::from("e2e-secret"))
            .expect("ns client")
            .with_device("cgm-bridge")
            .with_app("cgm-bridge");
        let ns_sink: Arc<dyn Sink> = Arc::new(NightscoutSink::new(ns_client));
        let dlq = DlqSink::open(ns_sink, state_dir, max_entries).expect("open dlq");
        Arc::new(SinkRouter::new(Arc::new(dlq)))
    }

    /// Outage → DLQ persistence → Recovery happy path.
    ///
    /// Cycle 1: NS returns 502 on POST. Push 2 readings through the
    /// layered stack via `fan_out_to_sinks`. Assert:
    ///   - DLQ file exists at `<state_dir>/dlq/nightscout.jsonl`
    ///   - DLQ file has 2 lines (one per reading)
    ///   - SinkRouter watermark is still `None`
    ///
    /// Cycle 2: NS returns 201 on POST. Push 1 *new* reading. Assert:
    ///   - DLQ file is gone
    ///   - NS received exactly one POST with 3 readings (2 drained + 1 new)
    ///   - SinkRouter watermark advanced to the newest reading's timestamp
    #[tokio::test]
    async fn outage_then_recovery_drains_dlq() {
        let server = MockServer::start().await;
        mount_ns_get_empty(&server).await;

        let state = TempDir::new().unwrap();
        let dlq_file = state.path().join("dlq").join("nightscout.jsonl");
        let router = build_layered_sink(&server, state.path(), 1000);

        // ── Cycle 1: NS outage. `mount_as_scoped` so we can drop it. ─
        let outage = Mock::given(method("POST"))
            .and(path("/api/v3/entries"))
            .and(header("api-secret", API_SECRET_SHA1))
            .respond_with(ResponseTemplate::new(502))
            .named("cycle-1 NS 502 outage")
            .mount_as_scoped(&server)
            .await;

        // 1_700_000_{100,200,300} = arbitrary epoch seconds picked to give
        // three strictly-ordered instants (~ 2023-11-14 22:13:20 UTC base).
        let batch1 = vec![
            reading_at(1_700_000_100, 110.0),
            reading_at(1_700_000_200, 112.0),
        ];
        crate::fan_out_to_sinks(
            std::slice::from_ref(&router),
            &batch1,
            Duration::from_secs(5),
        )
        .await;

        assert!(dlq_file.exists(), "DLQ file must exist after sink failure");
        let lines = std::fs::read_to_string(&dlq_file).unwrap();
        assert_eq!(
            lines.lines().count(),
            2,
            "DLQ should hold exactly 2 readings after cycle 1: {lines:?}"
        );
        assert!(
            router.watermark().is_none(),
            "watermark must not advance on failure"
        );

        // ── Cycle 2: NS recovers (drop the 502 scoped mock first). ───
        drop(outage);
        Mock::given(method("POST"))
            .and(path("/api/v3/entries"))
            .and(header("api-secret", API_SECRET_SHA1))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;

        let batch2 = vec![reading_at(1_700_000_300, 115.0)];
        crate::fan_out_to_sinks(
            std::slice::from_ref(&router),
            &batch2,
            Duration::from_secs(5),
        )
        .await;

        assert!(!dlq_file.exists(), "DLQ file must be deleted after drain");
        assert_eq!(
            router.watermark().map(|t| t.timestamp()),
            Some(1_700_000_300),
            "watermark advances to newest reading after success"
        );

        // ── Verify wiremock saw a single recovery POST with all 3 readings. ─
        let requests = server.received_requests().await.unwrap();
        let posts: Vec<_> = requests
            .iter()
            .filter(|r| r.method.as_str() == "POST" && r.url.path() == "/api/v3/entries")
            .collect();
        // NS retries 502 internally via `NightscoutClient::post_entries`
        // (MAX_POST_RETRIES = 2 → 1 initial + 2 retries = 3 attempts).
        // DlqSink does NOT retry — it just propagates the final Err and
        // persists. So cycle 1 = 3 POSTs (all 502), cycle 2 = 1 POST (201).
        assert_eq!(
            posts.len(),
            4,
            "3 failing POSTs (1 + 2 retries on cycle 1) + 1 recovery POST"
        );
        let recovery_req = posts.last().expect("at least one POST must exist");
        let recovery_body: serde_json::Value =
            serde_json::from_slice(&recovery_req.body).expect("recovery POST body json");
        let arr = recovery_body.as_array().expect("array");
        assert_eq!(
            arr.len(),
            3,
            "recovery POST must carry 2 drained + 1 new = 3 readings"
        );
        // Oldest-first ordering is the DLQ's merge_dedup contract.
        let dates: Vec<i64> = arr.iter().map(|e| e["date"].as_i64().unwrap()).collect();
        let mut sorted = dates.clone();
        sorted.sort();
        assert_eq!(dates, sorted, "DLQ drain must emit oldest-first");
    }

    /// DLQ persistence survives dropping & re-opening the sink stack.
    ///
    /// Simulates a process restart: Phase 1 builds a layered stack,
    /// forces NS failure, writes the DLQ file, then drops the stack.
    /// Phase 2 builds a fresh stack against the same `state_dir` and
    /// pushes ONE new reading (with a timestamp not in the persisted
    /// set). NS recovers and the recovery POST must carry FOUR
    /// readings — 3 loaded from disk + 1 new — proving the fresh
    /// `DlqSink` loaded the persisted queue (otherwise the POST would
    /// only have the 1 new reading).
    #[tokio::test]
    async fn dlq_survives_simulated_restart() {
        let server = MockServer::start().await;
        mount_ns_get_empty(&server).await;

        let state = TempDir::new().unwrap();
        let dlq_file = state.path().join("dlq").join("nightscout.jsonl");

        // ── Phase 1: build stack, fail, drop ─────────────────────────
        let outage = Mock::given(method("POST"))
            .and(path("/api/v3/entries"))
            .and(header("api-secret", API_SECRET_SHA1))
            .respond_with(ResponseTemplate::new(502))
            .named("phase-1 NS 502 outage")
            .mount_as_scoped(&server)
            .await;

        {
            let router = build_layered_sink(&server, state.path(), 1000);
            let batch = vec![
                reading_at(1_700_000_100, 110.0),
                reading_at(1_700_000_200, 112.0),
                reading_at(1_700_000_300, 115.0),
            ];
            crate::fan_out_to_sinks(
                std::slice::from_ref(&router),
                &batch,
                Duration::from_secs(5),
            )
            .await;
            assert!(dlq_file.exists(), "DLQ file must exist after phase-1 push");
        } // router + DlqSink + NightscoutSink dropped here

        assert!(dlq_file.exists(), "DLQ file must survive sink-stack drop");
        let persisted_lines = std::fs::read_to_string(&dlq_file).unwrap();
        assert_eq!(
            persisted_lines.lines().count(),
            3,
            "3 readings persisted before restart"
        );

        // ── Phase 2: NS recovers, fresh stack drains the queue ───────
        drop(outage);
        Mock::given(method("POST"))
            .and(path("/api/v3/entries"))
            .and(header("api-secret", API_SECRET_SHA1))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;

        let router2 = build_layered_sink(&server, state.path(), 1000);
        // Push ONE new reading whose timestamp is NOT in the persisted set.
        // If `DlqSink::open` correctly loaded the file, merge-dedup yields
        // 3 (disk) + 1 (new) = 4 readings in the recovery POST. If open
        // silently failed, only the 1 new reading would appear — catching
        // the bug immediately.
        let replay_batch = vec![
            reading_at(1_700_000_400, 117.0), // NEW timestamp — not on disk
        ];
        crate::fan_out_to_sinks(
            std::slice::from_ref(&router2),
            &replay_batch,
            Duration::from_secs(5),
        )
        .await;

        assert!(!dlq_file.exists(), "DLQ file removed after drain");
        assert_eq!(
            router2.watermark().map(|t| t.timestamp()),
            Some(1_700_000_400),
            "watermark advanced to newest reading after drain+replay"
        );

        let requests = server.received_requests().await.unwrap();
        // 4 readings = 3 from disk + 1 from `replay_batch`. If `DlqSink::open`
        // had silently failed to load the file, this assertion would catch it
        // (the POST would only carry the 1 new reading).
        // Use a single combined predicate so we search all POSTs, not just the
        // first one (Phase 1 POSTs carried 3 readings and would defeat a
        // `.find().filter()` chain).
        let recovery_post = requests
            .iter()
            .find(|r| {
                r.method.as_str() == "POST"
                    && r.url.path() == "/api/v3/entries"
                    && serde_json::from_slice::<serde_json::Value>(&r.body)
                        .ok()
                        .and_then(|v| v.as_array().cloned())
                        .map(|arr| arr.len() == 4)
                        .unwrap_or(false)
            })
            .expect("recovery POST with 4 readings (3 disk + 1 new) must be present");
        let body: serde_json::Value = serde_json::from_slice(&recovery_post.body).unwrap();
        let arr = body.as_array().expect("array");
        assert_eq!(
            arr.len(),
            4,
            "drained POST contains 3 persisted + 1 new reading"
        );
        // Disk-load → merge → POST must preserve oldest-first order
        // (`DlqSink::merge_dedup` contract). Catches a regression where
        // restart-reload returns entries out of order.
        let dates: Vec<i64> = arr.iter().map(|e| e["date"].as_i64().unwrap()).collect();
        let mut sorted = dates.clone();
        sorted.sort();
        assert_eq!(
            dates, sorted,
            "drained POST after restart must be oldest-first"
        );
    }

    /// During an extended outage the DLQ must cap at `max_entries` by
    /// evicting the oldest readings. This is the only E2E test in the
    /// file that uses a non-default cap — value 3 with 5 readings makes
    /// the eviction visible without parsing 9999 lines of JSON.
    #[tokio::test]
    async fn dlq_cap_evicts_oldest_during_outage() {
        let server = MockServer::start().await;
        mount_ns_get_empty(&server).await;
        Mock::given(method("POST"))
            .and(path("/api/v3/entries"))
            .and(header("api-secret", API_SECRET_SHA1))
            .respond_with(ResponseTemplate::new(502))
            .named("extended-outage NS 502")
            .mount(&server)
            .await;

        let state = TempDir::new().unwrap();
        let dlq_file = state.path().join("dlq").join("nightscout.jsonl");
        let router = build_layered_sink(&server, state.path(), 3);

        // Five readings, cap = 3 → oldest two (100, 200) must be evicted.
        let batch = vec![
            reading_at(1_700_000_100, 110.0),
            reading_at(1_700_000_200, 111.0),
            reading_at(1_700_000_300, 112.0),
            reading_at(1_700_000_400, 113.0),
            reading_at(1_700_000_500, 114.0),
        ];
        crate::fan_out_to_sinks(
            std::slice::from_ref(&router),
            &batch,
            Duration::from_secs(5),
        )
        .await;

        assert!(dlq_file.exists(), "DLQ file must exist after failed push");
        let lines: Vec<String> = std::fs::read_to_string(&dlq_file)
            .unwrap()
            .lines()
            .map(|l| l.to_string())
            .collect();
        assert_eq!(lines.len(), 3, "cap = 3 must trim oldest two");

        // Each line is a `DlqEntry { v: 1, reading: Reading }`. Deserialise
        // into the concrete `Reading` type — this anchors the test to the
        // actual serde contract and fails loudly (with a clear field-level
        // error) if the on-disk schema ever changes shape.
        #[derive(serde::Deserialize)]
        struct PersistedEntry {
            reading: Reading,
        }
        let parsed: Vec<i64> = lines
            .iter()
            .map(|l| {
                let entry: PersistedEntry =
                    serde_json::from_str(l).expect("parse DLQ line as PersistedEntry");
                entry.reading.timestamp.timestamp()
            })
            .collect();
        assert_eq!(
            parsed,
            vec![1_700_000_300, 1_700_000_400, 1_700_000_500],
            "DLQ must keep the newest 3 readings in oldest-first order"
        );
    }
}
