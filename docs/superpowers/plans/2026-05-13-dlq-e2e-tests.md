# DLQ End-to-End Tests Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add three E2E tests that exercise the full layered sink stack (`SinkRouter` → `DlqSink` → `NightscoutSink`) against a `wiremock` Nightscout, covering outage→recovery, persistence across simulated restart, and overflow eviction.

**Architecture:** Extend `gluco-hub/src/e2e_tests.rs` with a small set of helpers and three `#[tokio::test]` functions. Tests construct `Reading` values directly (no LLU source needed for DLQ behaviour) and drive the production `fan_out_to_sinks` function so the wiring is identical to the real poll loop. Wiremock's `mount_as_scoped` provides the outage-then-recovery response sequence by dropping a 502 mock between cycles.

**Tech Stack:** Rust 1.95, `tokio`, `wiremock` 0.6 (`MockServer`, `mount_as_scoped`), `tempfile::TempDir`, `chrono`, `secrecy`. Existing crates only — no new dependencies.

---

## File Structure

- **Modify:** `gluco-hub/src/e2e_tests.rs` — add helpers + 3 tests at the bottom of the existing file. The file is already gated on `#[cfg(all(test, feature = "source-llu", feature = "sink-nightscout"))]`; new tests stay under the same gate (LLU feature is harmless even though unused — keeps the `cfg` block consistent and matches the existing test `full_pipeline_survives_nightscout_502_and_keeps_cache_fresh` which lives in the same file).

No other files are touched — DLQ, SinkRouter, NightscoutSink, and `fan_out_to_sinks` already exist and are public-within-crate.

---

## Background context the engineer needs

These facts are documented in the source but worth restating because the assertions hinge on them:

1. `DlqSink::open(inner, state_dir, max_entries)` is the constructor. The on-disk file lives at `<state_dir>/dlq/<sink_name>.jsonl`. `NightscoutSink::name()` returns `"nightscout"`, so the file is `<state_dir>/dlq/nightscout.jsonl`.
2. `DlqSink::push` returns `Err(_)` on inner-sink failure AND persists the merged set to disk before returning. On `Ok(())` it clears in-memory state and deletes the file.
3. `SinkRouter::push_filtered` only advances its watermark on a successful inner push. After a failed cycle the watermark stays `None` (or its previous value), so the next cycle's filter still includes the missed readings.
4. `fan_out_to_sinks` absorbs sink errors (logs + metrics, returns `()`). Tests must assert via *side effects*: file existence, file contents, wiremock requests received, `SinkRouter::watermark()`.
5. NightscoutSink performs `GET /api/v3/entries?count=1` for dedup before each `POST`. Returning `404` makes the sink post the full batch without filtering — matches what the existing test `full_pipeline_pulls_from_llu_and_pushes_to_nightscout` does.
6. The `api-secret` header for the literal secret `"e2e-secret"` is the sha1 hex digest `631a0d6c3813ee3a11e19b0a37a10ad75bbe8a0c` — reuse the matcher pattern from `mount_nightscout` in the existing file.
7. wiremock's `mount_as_scoped` returns a `MockGuard`; dropping it unmounts the mock. This is the cleanest way to flip NS from 502 to 201 between cycles.

---

## Task 1: Shared helpers + outage-then-recovery test

**Files:**
- Modify: `gluco-hub/src/e2e_tests.rs` — append at the bottom (after the existing `custom_version_propagates_to_login_header` test).

- [ ] **Step 1: Add the test imports and helpers**

Open `gluco-hub/src/e2e_tests.rs` and, at the very bottom of the file (after the last `}` of `custom_version_propagates_to_login_header`), append:

```rust
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
            timestamp: Utc.timestamp_opt(ts_secs, 0).unwrap(),
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
            .mount(server)
            .await;
    }

    /// Build the production-shape layered sink stack pointing at
    /// `server`'s URL and using `state_dir` for DLQ persistence.
    /// Returns the `SinkRouter` (what `fan_out_to_sinks` expects) plus
    /// the inner-most NS-name-string for file-path construction.
    fn build_layered_sink(
        server: &MockServer,
        state_dir: &std::path::Path,
        max_entries: usize,
    ) -> Arc<SinkRouter> {
        let ns_client =
            NightscoutClient::new(server.uri(), SecretString::from("e2e-secret"))
                .expect("ns client")
                .with_device("cgm-bridge")
                .with_app("cgm-bridge");
        let ns_sink: Arc<dyn Sink> = Arc::new(NightscoutSink::new(ns_client));
        let dlq = DlqSink::open(ns_sink, state_dir, max_entries).expect("open dlq");
        Arc::new(SinkRouter::new(Arc::new(dlq)))
    }
}
```

**Why:** All three DLQ tests share these helpers. Putting them in a private submodule `dlq_e2e` inside the existing file keeps them out of the LLU-pipeline tests' namespace and avoids any chance of collision with the existing `mount_nightscout`. The constant `API_SECRET_SHA1` is intentionally duplicated rather than extracted — the cost of touching the existing test surface to expose it is higher than the cost of one duplicated 40-character hex literal.

- [ ] **Step 2: Run the build to confirm the helpers compile**

```bash
cargo build --tests --features "source-llu sink-nightscout sink-mqtt"
```

Expected: `Finished ... profile [unoptimized + debuginfo] target(s) in N.NNs`. No errors. If you see "unused" warnings on the helpers, that's fine for now — the next step's test will use them.

- [ ] **Step 3: Write the outage-then-recovery test**

Inside the `mod dlq_e2e { ... }` block from Step 1, before its closing `}`, append:

```rust
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
            .mount_as_scoped(&server)
            .await;

        let batch1 = vec![reading_at(1_700_000_100, 110.0), reading_at(1_700_000_200, 112.0)];
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
        assert_eq!(posts.len(), 2, "1 failing POST + 1 recovery POST");
        let recovery_body: serde_json::Value =
            serde_json::from_slice(&posts[1].body).expect("recovery POST body json");
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
```

- [ ] **Step 4: Run the test and verify it passes**

```bash
cargo test --features "source-llu sink-nightscout" \
    --bin gluco-hub e2e_tests::dlq_e2e::outage_then_recovery_drains_dlq \
    -- --nocapture
```

Expected: `test e2e_tests::dlq_e2e::outage_then_recovery_drains_dlq ... ok` and `test result: ok. 1 passed`.

If the test fails:
- Re-read the failure message carefully. The most likely failure modes are:
  - DLQ file path wrong: print `state.path()` and verify the helper builds `<state_dir>/dlq/nightscout.jsonl`.
  - `wiremock` matcher mismatch: drop the `header("api-secret", ...)` matcher temporarily to see whether the GET/POST is hitting at all.
  - Watermark assertion: confirm the assertion runs *after* both `fan_out_to_sinks` awaits.

- [ ] **Step 5: Run clippy on the test**

```bash
cargo clippy --all-targets --features "source-llu sink-nightscout sink-mqtt" -- -D warnings
```

Expected: clean exit.

- [ ] **Step 6: Commit**

```bash
git add gluco-hub/src/e2e_tests.rs
git commit -m "$(cat <<'EOF'
test(dlq): E2E outage-then-recovery covers SinkRouter+DlqSink+NS

Adds a wiremock-driven E2E test that exercises the production layered
sink stack (SinkRouter → DlqSink → NightscoutSink). Cycle 1 with NS
returning 502 must persist the batch to disk and leave the watermark
untouched; cycle 2 with NS recovered must drain the queue into a single
POST containing both backfilled and new readings, then delete the file
and advance the watermark.
EOF
)"
```

---

## Task 2: DLQ survives simulated process restart

**Files:**
- Modify: `gluco-hub/src/e2e_tests.rs` — append the test inside the `mod dlq_e2e { ... }` block.

- [ ] **Step 1: Write the restart test**

Inside `mod dlq_e2e { ... }`, after `outage_then_recovery_drains_dlq`, append:

```rust
    /// DLQ persistence survives dropping & re-opening the sink stack.
    ///
    /// Simulates a process restart: build a layered stack, force NS
    /// failure to write the DLQ file, drop the stack, then build a
    /// fresh stack against the same `state_dir`. The fresh `DlqSink`
    /// must load the persisted queue from disk and drain it on the
    /// next successful push — even with an empty incoming batch.
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
            assert!(dlq_file.exists(), "DLQ file must exist after cycle 1");
        } // router + DlqSink + NightscoutSink dropped here

        assert!(
            dlq_file.exists(),
            "DLQ file must survive sink-stack drop"
        );
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
        // Empty incoming batch — DLQ must still drain on its own.
        crate::fan_out_to_sinks(
            std::slice::from_ref(&router2),
            &[],
            Duration::from_secs(5),
        )
        .await;

        assert!(!dlq_file.exists(), "DLQ file removed after drain");
        assert_eq!(
            router2.watermark().map(|t| t.timestamp()),
            Some(1_700_000_300),
            "watermark advanced to newest drained reading"
        );

        let requests = server.received_requests().await.unwrap();
        let recovery_post = requests
            .iter()
            .find(|r| r.method.as_str() == "POST" && r.url.path() == "/api/v3/entries")
            .filter(|r| {
                // The recovery POST is the *second* POST (first one returned 502
                // and was the one that persisted the queue).
                serde_json::from_slice::<serde_json::Value>(&r.body)
                    .ok()
                    .and_then(|v| v.as_array().cloned())
                    .map(|arr| arr.len() == 3)
                    .unwrap_or(false)
            })
            .expect("recovery POST with 3 readings must be present");
        let body: serde_json::Value =
            serde_json::from_slice(&recovery_post.body).unwrap();
        assert_eq!(
            body.as_array().unwrap().len(),
            3,
            "drained POST contains all 3 persisted readings"
        );
    }
```

- [ ] **Step 2: Run the test**

```bash
cargo test --features "source-llu sink-nightscout" \
    --bin gluco-hub e2e_tests::dlq_e2e::dlq_survives_simulated_restart \
    -- --nocapture
```

Expected: `test result: ok. 1 passed`.

Possible failure mode: if the test fails on `dlq_file.exists()` after the inner block, suspect `TempDir` being dropped early — `state` is declared in the outer scope, so the temp dir lives for the whole test. If the file disappears, suspect a bug in `DlqSink::open` (it should NOT delete the file on construction).

- [ ] **Step 3: Run the full DLQ-E2E subset**

```bash
cargo test --features "source-llu sink-nightscout" \
    --bin gluco-hub e2e_tests::dlq_e2e:: \
    -- --nocapture
```

Expected: both tests pass. Confirms the new test didn't break the previous one.

- [ ] **Step 4: Commit**

```bash
git add gluco-hub/src/e2e_tests.rs
git commit -m "$(cat <<'EOF'
test(dlq): E2E covers persistence across simulated restart

A failed sink push leaves the DLQ file behind; dropping the sink stack
and re-opening it against the same state_dir must load the persisted
queue and drain it on the next successful push — even when the
incoming batch is empty.
EOF
)"
```

---

## Task 3: DLQ cap evicts oldest during extended outage

**Files:**
- Modify: `gluco-hub/src/e2e_tests.rs` — append inside `mod dlq_e2e { ... }`.

- [ ] **Step 1: Write the cap-eviction test**

Inside `mod dlq_e2e { ... }`, after `dlq_survives_simulated_restart`, append:

```rust
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

        // Each line is a `DlqEntry { v: 1, reading: Reading }`. Parse and
        // assert we kept the *newest* three timestamps.
        let parsed: Vec<i64> = lines
            .iter()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).expect("parse DLQ line");
                v["reading"]["timestamp"]
                    .as_str()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .expect("timestamp")
                    .timestamp()
            })
            .collect();
        assert_eq!(
            parsed,
            vec![1_700_000_300, 1_700_000_400, 1_700_000_500],
            "DLQ must keep the newest 3 readings in oldest-first order"
        );
    }
```

- [ ] **Step 2: Run the test**

```bash
cargo test --features "source-llu sink-nightscout" \
    --bin gluco-hub e2e_tests::dlq_e2e::dlq_cap_evicts_oldest_during_outage \
    -- --nocapture
```

Expected: `test result: ok. 1 passed`.

If the `parsed` assertion fails with unexpected timestamps, suspect the on-disk serialization format. `DlqEntry` is `{ v: 1, reading: Reading }`. `Reading::timestamp` is a `DateTime<Utc>` which serde-serializes as an RFC3339 string by default. If your read produces an integer, the chrono feature flags have changed — update the test to match (`as_i64()` + `Utc.timestamp_opt(...)`).

- [ ] **Step 3: Run the full pre-PR gate**

```bash
cargo fmt --all
cargo clippy --all-targets --features "source-llu sink-nightscout sink-mqtt" -- -D warnings
cargo test --all-features
cargo deny check
```

Expected: every step succeeds. If any DLQ unit test in `dlq.rs` regressed, suspect the new tests racing on TempDir paths — they shouldn't, because each test owns its own `TempDir`, but if you see flaky behaviour run with `--test-threads=1` to confirm.

- [ ] **Step 4: Commit**

```bash
git add gluco-hub/src/e2e_tests.rs
git commit -m "$(cat <<'EOF'
test(dlq): E2E covers cap eviction during extended outage

With `max_entries = 3` and 5 readings pushed during a sustained NS
outage, the on-disk DLQ must keep only the newest 3 readings,
oldest-first, with the oldest two evicted. Parses the persisted JSONL
to assert both the count and the surviving timestamps.
EOF
)"
```

---

## Final verification

- [ ] **Step 1: Full test suite, all features**

```bash
cargo test --all-features
```

Expected: all tests pass, including the three new ones in `e2e_tests::dlq_e2e`.

- [ ] **Step 2: Verify CHANGELOG entry**

This change is **test-only and user-invisible**, so CHANGELOG.md does NOT need an entry per the project's contributor rule (`CHANGELOG entry required for user-visible behaviour changes`). Confirm by skimming `CLAUDE.md` → "Releasing & Branching" section. If you disagree, add a line under `## [Unreleased]` → `### Changed`: "Add DLQ E2E tests."

- [ ] **Step 3: Push develop**

```bash
git push origin develop
```

CI on develop must stay green. If `cargo deny check` fails for an unrelated yanked-crate reason, fix it with `cargo update -p <crate>` in a separate commit — do not bundle it with the test PR.

---

## What this plan deliberately does NOT do

- No new DLQ feature work. DLQ is shipped (`gluco-hub/src/dlq.rs`, 490 LoC, 8 unit tests) and exercised by `SinkRouter` in production wiring already.
- No HTTP admin endpoints for the DLQ. That's a separate plan if needed.
- No MQTT-DLQ E2E coverage. The MQTT sink would need a broker mock (mqtts or `rumqttd` in-process), which is a substantially larger setup. Out of scope.
- No metric assertions. The `metrics-exporter-prometheus` recorder is global state and resetting it between tests is fragile. The DLQ's unit tests cover the metric paths; the E2E tests cover the wiring.
