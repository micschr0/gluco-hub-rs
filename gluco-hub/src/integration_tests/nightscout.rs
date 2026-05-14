// SPDX-License-Identifier: AGPL-3.0-or-later

//! Phase B: NightscoutSink against a real Nightscout + MongoDB stack.
//!
//! Coverage beyond the wiremock-based tests in `src/e2e_tests.rs`:
//!  * sha1 `api-secret` validated by an actual NS Express middleware
//!    (not just header-matched by a mock);
//!  * idempotency: NS dedups by `(deviceId, date)` in the `entries`
//!    collection — verified end-to-end against MongoDB persistence;
//!  * auth-failure path: real NS returns 401, surfaced as a sink-side
//!    `CoreError::Sink` with NS003.
//!
//! Each `#[tokio::test]` spins up its own compose stack (NS + Mongo).
//! Boot is ~30 s; mark `--test-threads=1` in CI to avoid simultaneous
//! NS startup contention on smaller runners.

use std::sync::Arc;

use gluco_hub_core::Sink;
use secrecy::SecretString;

use crate::sinks::nightscout::{NightscoutClient, NightscoutSink};

use super::common::nightscout_container::{
    API_SECRET, api_secret_sha1, fetch_entries_v3, start_nightscout_stack,
};
use super::common::{reading, unique_id};

/// Wire a `NightscoutSink` against the running stack. `device_id`
/// keyed per-test so concurrent runs do not collide on NS's
/// `(deviceId, date)` dedup index.
fn build_sink(ns_url: &str, device_id: &str) -> Arc<NightscoutSink> {
    let client = NightscoutClient::new(ns_url.to_string(), SecretString::from(API_SECRET))
        .expect("ns client")
        .with_device(device_id)
        .with_app("gluco-hub-itest");
    Arc::new(NightscoutSink::new(client))
}

#[tokio::test]
async fn push_persists_reading_in_real_mongo() {
    let stack = start_nightscout_stack()
        .await
        .expect("start nightscout stack");
    let device_id = unique_id("itest-push");
    let sink = build_sink(&stack.ns_url(), &device_id);

    // Use a recent timestamp so NS's optional retention window doesn't
    // discard the entry. 60 s ago avoids any clock-skew rejection.
    let ts = chrono::Utc::now().timestamp() - 60;
    sink.push(&[reading(ts, 138.0)]).await.expect("push");

    let body = fetch_entries_v3(&stack, &api_secret_sha1(), 5)
        .await
        .expect("fetch back");
    let result = body
        .get("result")
        .and_then(|v| v.as_array())
        .expect("result array");
    let entry = result
        .iter()
        .find(|e| e.get("device").and_then(|d| d.as_str()) == Some(device_id.as_str()))
        .unwrap_or_else(|| panic!("entry for device {device_id} not found: {body:?}"));
    assert_eq!(entry["type"], "sgv");
    assert_eq!(entry["sgv"], 138.0);
    assert_eq!(entry["direction"], "Flat");
}

#[tokio::test]
async fn duplicate_pushes_deduplicate_in_mongo() {
    let stack = start_nightscout_stack()
        .await
        .expect("start nightscout stack");
    let device_id = unique_id("itest-dedup");
    let sink = build_sink(&stack.ns_url(), &device_id);

    let ts = chrono::Utc::now().timestamp() - 60;
    let r = reading(ts, 142.0);

    sink.push(std::slice::from_ref(&r))
        .await
        .expect("first push");
    sink.push(std::slice::from_ref(&r))
        .await
        .expect("second push (should noop)");

    let body = fetch_entries_v3(&stack, &api_secret_sha1(), 10)
        .await
        .expect("fetch");
    let result = body
        .get("result")
        .and_then(|v| v.as_array())
        .expect("result array");
    let matches: Vec<&serde_json::Value> = result
        .iter()
        .filter(|e| e.get("device").and_then(|d| d.as_str()) == Some(device_id.as_str()))
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "NS must dedup by (deviceId, date); got {} entries: {body:?}",
        matches.len()
    );
}

#[tokio::test]
async fn wrong_api_secret_surfaces_as_sink_error() {
    let stack = start_nightscout_stack()
        .await
        .expect("start nightscout stack");
    let device_id = unique_id("itest-auth");

    // Build a client with a wrong secret — NS expects sha1("itest-secret-
    // please-rotate") but receives sha1("nope").
    let client = NightscoutClient::new(stack.ns_url(), SecretString::from("nope"))
        .expect("ns client")
        .with_device(&device_id)
        .with_app("gluco-hub-itest");
    let sink = NightscoutSink::new(client);

    let result = sink.push(&[reading(1_700_000_000, 110.0)]).await;
    let err = result.expect_err("must fail with wrong api_secret");
    let msg = err.to_string();
    assert!(
        msg.contains("NS00") || msg.contains("401") || msg.contains("Unauthorized"),
        "expected NS-auth-error code in {msg:?}"
    );
}

#[tokio::test]
async fn pushing_a_batch_of_three_persists_each_distinct_entry() {
    let stack = start_nightscout_stack()
        .await
        .expect("start nightscout stack");
    let device_id = unique_id("itest-batch");
    let sink = build_sink(&stack.ns_url(), &device_id);

    let now = chrono::Utc::now().timestamp();
    let batch: Vec<_> = (0..3)
        .map(|i| reading(now - 60 - (i as i64) * 30, 100.0 + i as f64 * 5.0))
        .collect();
    sink.push(&batch).await.expect("push batch of 3");

    let body = fetch_entries_v3(&stack, &api_secret_sha1(), 10)
        .await
        .expect("fetch");
    let result = body
        .get("result")
        .and_then(|v| v.as_array())
        .expect("result array");
    let count = result
        .iter()
        .filter(|e| e.get("device").and_then(|d| d.as_str()) == Some(device_id.as_str()))
        .count();
    assert_eq!(
        count, 3,
        "expected 3 entries for device {device_id}, got {count}: {body:?}"
    );
}
