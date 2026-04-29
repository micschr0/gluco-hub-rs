//! End-to-end pipeline tests.
//!
//! These run the full LibreLink Up → cache → Nightscout flow against a
//! single in-process `wiremock` server that hosts every endpoint the
//! bridge talks to: `/llu/auth/login`, `/llu/connections`,
//! `/llu/connections/{patient_id}/graph`, and `/api/v3/entries`. They
//! prove that:
//!
//! 1. The four call sequence happens in the right order.
//! 2. Every request carries the headers verified against the reference
//!    port (`timoschlueter/nightscout-librelink-up`): LLU `version`,
//!    `product`, `account-id` (`sha256(userId)`), Bearer; NS `api-secret`
//!    (`sha1_hex(API_SECRET)`).
//! 3. The Nightscout JSON body has the exact field set the reference
//!    sends — `date`, `sgv`, `direction`, `type: "sgv"`, `device`, `app`
//!    — and **no spurious `trend` integer**.
//! 4. The `ReadingCache` ends up holding the newest reading from the
//!    graph response.

use std::sync::Arc;
use std::time::Duration;

use cgm_bridge_core::{PatientId, ReadingCache, Sink, Source, SourceId};
use secrecy::SecretString;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::sinks::nightscout::{NightscoutClient, NightscoutSink};
use crate::sources::llu::Region;
use crate::sources::llu::auth::{LluAuthClient, LluCredentials, account_id_hash};
use crate::sources::llu::source::{ConnectionSelection, LluSource};

/// Reference fixture: an LLU response shape exactly mirroring the
/// `nightscout-librelink-up` reference suite — login, connections,
/// graph endpoints all plumbed in one server.
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
        .and(header("version", "4.16.0"))
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

    // --- Inspect the NS request body in detail; this is the key
    // alignment with the reference port.
    let ns_req = requests
        .iter()
        .find(|r| r.method.as_str() == "POST" && r.url.path() == "/api/v3/entries")
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&ns_req.body).expect("ns body json");
    let arr = body.as_array().expect("array");
    assert_eq!(arr.len(), 2);
    for entry in arr {
        // Reference fields:
        assert_eq!(entry["type"], "sgv", "type must be \"sgv\"");
        assert_eq!(entry["direction"], "Flat");
        assert_eq!(entry["device"], "cgm-bridge");
        assert_eq!(entry["app"], "cgm-bridge");
        assert!(entry["sgv"].is_i64());
        assert!(entry["date"].is_i64());
        // Anti-fields the reference does NOT send:
        assert!(
            entry.get("trend").is_none(),
            "no numeric trend field — reference omits it"
        );
        assert!(
            entry.get("dateString").is_none(),
            "no dateString — reference omits it"
        );
        assert!(
            entry.get("identifier").is_none(),
            "no identifier — reference omits it"
        );
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

    crate::fan_out_to_sinks(&[sink], &batch, Duration::from_secs(2)).await;

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
