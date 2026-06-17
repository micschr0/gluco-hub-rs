# V3 Manual Validation Checklist

The container-backed integration tests in `gluco-hub/src/integration_tests/`
cover the protocol-level behaviour against real Mosquitto and Nightscout
instances. They do **not** cover:

- the real LibreLink Up cloud (rate-limited, requires real credentials),
- the actual Home Assistant entity rendering (we validate the discovery
  schema, not HA's parser),
- long-running operational behaviour (24 h+ DLQ persistence, sustained
  recovery cycles).

This checklist is the manual validation pass to run on a feature-branch
PR before merge — or on the `:main` integration build before cutting a
CalVer release. Pull the image under test (`:main` for the latest build
on the default branch, or `:sha-<short>` for an immutable commit
snapshot) and run it against your live infrastructure. Tick each box.
File any unticked item as an issue before the merge / release.

## 0. Setup

- [ ] `docker pull ghcr.io/micschr0/gluco-hub:main`
- [ ] Container has the right version stamp:
      `docker run --rm ghcr.io/micschr0/gluco-hub:main --version`
      → expected: `gluco-hub <YYYY.MMDD.PATCH>` matching `Cargo.toml`.
- [ ] Persistent volume mounted at the configured `[state] dir` (DLQ
      survives container recreation).

## 1. Bring-up against real infrastructure

- [ ] Container starts with real LLU credentials in env vars
      (`GLUCO_HUB__SOURCE__LLU__EMAIL`, `..._PASSWORD`, `..._REGION`)
      and real Nightscout + MQTT broker reachable.
- [ ] First glucose reading appears in logs within one `poller.interval_secs`.
- [ ] `GET /healthz` returns `{"status":"ok","version":"..."}`.
- [ ] `GET /metrics` exposes `gluco_hub_build_info{...} 1` with the
      expected `version`, `git_sha`, `features` labels.

## 2. Nightscout sink (Production V1)

- [ ] First reading reaches Nightscout: visible in `/api/v3/entries?count=1`.
- [ ] `cgm_sink_push_success_total{sink="nightscout"}` increments on
      each successful push cycle.
- [ ] Stopping NS mid-flight: `cgm_sink_push_errors_total{sink="nightscout"}`
      increments, but `gluco-hub` keeps polling LLU (no crash).
- [ ] Restarting NS: next cycle either replays via SinkRouter watermark
      OR drains the DLQ — `cgm_sink_replayed_total` and / or
      `cgm_dlq_drained_total` increment, depending on outage duration.

## 3. MQTT sink (V2)

- [ ] First reading visible on `<topic_prefix>/glucose` (subscribe with
      `mosquitto_sub -h <broker> -t '<prefix>/#' -v`).
- [ ] **Burst regression**: `<prefix>/glucose` publishes ~1 message
      per `poller.interval_secs`, NOT one per cached graphData entry.
      (Pre-V3 behaviour was ~288 publishes/minute; SinkRouter must
      suppress this.)
- [ ] `<prefix>/_health` retained with `{"online":true,"v":1}` after
      ConnAck.
- [ ] Stop gluco-hub: subscribers see `<prefix>/_health` flip to
      `{"online":false,"v":1}` via MQTT LWT within `keep_alive_secs`.

## 4. Home Assistant MQTT auto-discovery (V3 new)

- [ ] `[sink.mqtt] discovery_enabled = true` in config; restart container.
- [ ] HA's MQTT integration auto-creates a `sensor.gluco_hub_<client_id>_glucose`
      entity within seconds of ConnAck.
- [ ] HA *also* auto-creates a sibling `sensor.gluco_hub_<client_id>_trend`
      entity, grouped under the same device as the glucose entity.
- [ ] Glucose entity state = current mg/dL (or mmol/L).
- [ ] Glucose entity attributes carry `trend`, `source`, `patient`, `ts`.
- [ ] Trend entity state = current `Trend` variant string (`Flat`,
      `SingleUp`, …) and updates whenever glucose updates.
- [ ] Trend entity is classified as an enum sensor (HA UI: device_class
      "enum"; allowed states match `options` in the discovery payload).
- [ ] Both entities' availability flip to `unavailable` together when
      gluco-hub is stopped (LWT-driven `_health` flip).
- [ ] HA "Device info" panel shows the gluco-hub-rs `origin` block
      (integration name + sw_version + support URL).
- [ ] Changing `discovery_enabled = false` → restart → entities stay
      in HA (retained config not deleted; manual cleanup required —
      documented behaviour).

## 5. Backfill via SinkRouter watermark (V3 new)

- [ ] Steady-state metrics: `cgm_sink_filtered_total{sink=...}`
      increments by ~287 per poll cycle (LLU returns ~288 readings,
      one is new).
- [ ] `cgm_sink_replayed_total{sink=...}` stays at 0 during normal
      operation.
- [ ] Simulate 5-min sink outage: kill MQTT broker / NS. Restart it.
      Next push cycle drains the missed window — `cgm_sink_replayed_total`
      increments by ~1 per missed cycle.

## 6. Persistent DLQ (V3 new)

- [ ] Stop NS (or MQTT) and let the container run for ~10 minutes.
- [ ] `<state_dir>/dlq/nightscout.jsonl` (or `mqtt.jsonl`) accumulates
      entries: `wc -l <state_dir>/dlq/*.jsonl` matches metric
      `cgm_dlq_size{sink=...}`.
- [ ] Restart gluco-hub container (with persistent volume still mounted).
- [ ] After restart, `cgm_dlq_size{sink=...}` reflects the file's line
      count — DLQ loaded from disk.
- [ ] Restart the downstream sink. Within one poll cycle:
      `<state_dir>/dlq/<sink>.jsonl` is deleted; `cgm_dlq_drained_total`
      increments by the prior queue size.
- [ ] Set `[dlq] max_entries = 100`, force >100 failures: oldest
      entries dropped, `cgm_dlq_evicted_total` increments. NS
      eventually sees only the newer 100 entries.

## 7. Negative / error paths

- [ ] Wrong NS api_secret: `cgm_sink_push_errors_total{sink="nightscout",error_code="NS003"}`
      increments; DLQ accumulates; container does not crash.
- [ ] Wrong MQTT password: `cgm_sink_push_errors_total{sink="mqtt",error_code="MQTT003"}`
      increments; container retries via rumqttc backoff.
- [ ] `check-config` rejects an empty `GLUCO_HUB__SOURCE__LLU__PASSWORD`
      with `[CFG007]`.

## 8. Tag / channel sanity

- [ ] `docker pull ghcr.io/micschr0/gluco-hub:latest` — equal-or-older
      than `:main` (no final release cut yet from this batch of changes).
- [ ] `docker pull ghcr.io/micschr0/gluco-hub:sha-<main-HEAD>` —
      identical digest to `:main`.

## Sign-off

- Tester: ___________________________
- Date:   ___________________________
- Notes / open issues:
