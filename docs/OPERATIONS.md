# gluco-hub — Operations

Runbook for endpoints, CLI, MQTT topics, metrics, configuration, troubleshooting, supply chain, and CI.

---

## CLI reference

```
gluco-hub [-c <config>] <subcommand>
```

| Subcommand     | Feature gate      | Purpose                                                   |
| -------------- | ----------------- | --------------------------------------------------------- |
| `run`          | —                 | Start the poll loop and HTTP API.                         |
| `check-config` | —                 | Validate config and exit (non-zero + `CFG0xx` on error).  |
| `dryrun`       | `source-llu`      | One-shot LLU probe; prints JSON summary, no server.       |
| `ns-dryrun`    | `sink-nightscout` | One-shot Nightscout read-only probe; never POSTs.         |

`scripts/llu-dryrun.sh` and `scripts/ns-dryrun.sh` wrap `dryrun` and `ns-dryrun`
for use without a compiled binary.

---

## Endpoints

| Path              | Method | Auth            | Response                                     |
| ----------------- | ------ | --------------- | -------------------------------------------- |
| `/healthz`        | GET    | public          | `{"status":"ok","version":"…"}`              |
| `/metrics`        | GET    | public          | Prometheus text exposition (v0.0.4) — includes a `patient_id` label on `cgm_glucose_mgdl`; protect at the network or proxy layer when exposing externally |
| `/glucose/latest` | GET    | optional Bearer | latest cached reading or `503` + `API001`    |

Setting `GLUCO_HUB__HTTP__BEARER_TOKEN` requires Bearer auth on `/glucose/*`; `/healthz` and `/metrics` remain public.

---

## MQTT topics (V2)

Requires `--features sink-mqtt` and a `[sink.mqtt]` config block.

| Topic              | Retained | Payload                                                    |
| ------------------ | :------: | ---------------------------------------------------------- |
| `<prefix>/glucose` | No       | `{"v":1,"glucose_mgdl":…,"trend":…,"timestamp_utc":…}`    |
| `<prefix>/_health` | Yes      | `{"online":true}` · LWT: `{"online":false}`                |
| `<prefix>/_stats`  | No       | periodic stats payload                                     |

`<prefix>` is `topic_prefix` from `[sink.mqtt]` (e.g. `gluco-hub/gluco-hub-1`).

Home Assistant auto-discovery via MQTT is planned for V3.

---

## Metrics

Exported on `/metrics`:

| Metric                           | Type       | Labels                      |
| -------------------------------- | ---------- | --------------------------- |
| `cgm_cache_updates_total`        | counter    | —                           |
| `cgm_source_fetch_success_total` | counter    | `source_id`                 |
| `cgm_source_fetch_errors_total`  | counter    | `error_code`                |
| `cgm_sink_push_success_total`    | counter    | `sink`                      |
| `cgm_sink_push_errors_total`     | counter    | `sink`, `error_code`        |
| `cgm_sink_post_retry_total`      | counter    | `sink`, `attempt`           |
| `cgm_sink_dedup_skipped_total`   | counter    | `sink`                      |
| `cgm_glucose_mgdl`               | gauge      | `patient_id`, `source_id`   |
| `gluco_hub_build_info`          | gauge (=1) | `version`, `git_sha`, `features` |

Stable error-code prefixes (`CORE0xx`, `CFG0xx`, `API0xx`, `LLU0xx`, `NS0xx`, `AUTH0xx`, `MQTT0xx`) appear as metric labels and in JSON log fields so dashboards can use grep-clean rules across versions.

---

## Environment variables

| Variable               | Example                         | Effect                                       |
| ---------------------- | ------------------------------- | -------------------------------------------- |
| `GLUCO_HUB_LOG_PRETTY`| `1`                             | Human-readable logs instead of JSON (dev).   |
| `RUST_LOG`             | `gluco_hub=debug,reqwest=info` | Standard `tracing` filter.                   |

**`config.toml` is optional.** Without it (no `-c` flag, no mounted file), the binary loads built-in defaults and reads every value from `GLUCO_HUB__*` env vars. With it, env vars override TOML values key by key.

Config overrides: any TOML key can be set or overridden at runtime with `GLUCO_HUB__SECTION__KEY=…`
(double-underscore delimited), e.g. `GLUCO_HUB__HTTP__BIND=0.0.0.0:9090` or `GLUCO_HUB__SOURCE__LLU__EMAIL=you@example.com`.

Secrets are injected the same way — never embedded in TOML:

| Secret              | Environment variable                        | Holds                  |
| ------------------- | ------------------------------------------- | ---------------------- |
| LLU password        | `GLUCO_HUB__SOURCE__LLU__PASSWORD`          | LibreLink Up password  |
| Nightscout secret   | `GLUCO_HUB__SINK__NIGHTSCOUT__API_SECRET`   | Nightscout API secret  |
| HTTP Bearer token   | `GLUCO_HUB__HTTP__BEARER_TOKEN`             | API Bearer token       |
| MQTT password       | `GLUCO_HUB__SINK__MQTT__PASSWORD`           | MQTT broker password   |

The LLU password can alternatively be supplied via `password_file = "/run/secrets/…"` in `[source.llu]` — useful for Docker secrets and Kubernetes secret volumes.

Secrets never appear in TOML, logs, or `Debug` output.

---

## Graceful shutdown

`SIGINT` and `SIGTERM` both trigger graceful shutdown. Under Docker or Kubernetes, use the exec-form
`ENTRYPOINT` so the signal reaches PID 1 directly.

Kubernetes liveness / readiness:

```yaml
livenessProbe:
  httpGet: { path: /healthz, port: 8080 }
  initialDelaySeconds: 5
readinessProbe:
  httpGet: { path: /glucose/latest, port: 8080 }
  failureThreshold: 5   # cache is empty before the first successful poll
```

---

## Container build

```mermaid
graph TB
    subgraph "Dockerfile — 4-stage cargo-chef build"
        S1["Stage 1 · chef\nrust:1.87.0-bookworm\ncmake · cargo-chef"]
        S2["Stage 2 · planner\nmanifests + stubs\n→ recipe.json"]
        S3["Stage 3 · build\ncargo chef cook (deps)\n→ cargo build --release"]
        S4["Stage 4 · runtime\ndistroless/cc-debian12:nonroot\nuid 65532 · no shell"]
        S1 --> S2 --> S3 --> S4
    end
```

Docker caches the dep-cook layer until `Cargo.lock` or `Cargo.toml` changes. Source edits rebuild only from the `cargo build` step.

`GLUCO_HUB_GIT_SHA` populates `gluco_hub_build_info{git_sha=…}`; `BUILD_DATE` sets `org.opencontainers.image.created`.

### Dead-letter queue (V3)

Failed sink pushes are persisted to `<state_dir>/dlq/<sink>.jsonl` (one
JSON-encoded `Reading` per line). On the next successful push the file
is deleted and the readings drained to the sink.

Inspect: `wc -l ./state/dlq/*.jsonl` (counts pending readings per sink).
Metrics: `cgm_dlq_size{sink=...}` (current size), `cgm_dlq_enqueued_total`
+ `cgm_dlq_drained_total` + `cgm_dlq_evicted_total` (counters).

Clear a stuck queue manually: stop the service, `rm
./state/dlq/<sink>.jsonl`, restart. The watermark in `SinkRouter` is
in-memory so a restart causes the next cycle to resend the full 24 h
window; the deleted DLQ stays empty.

In containers, mount a persistent volume at the configured `[state] dir`
or accept that DLQ contents reset on each container recreation.

### GHCR storage hygiene

Every push to `main`, `develop`, or a `v*` tag publishes a multi-arch
manifest list plus its underlying per-arch image manifests. GHCR shows
the per-arch manifests as separate "package versions"; over time these
accumulate as untagged digests even after the named tag has rolled
forward.

Two recommended cleanup mechanisms:

1. **GHCR retention policy** (UI-only, one-time). Open the package
   settings (`github.com/users/<owner>/packages/container/gluco-hub/settings`),
   under "Manage retention" enable a rule like "delete untagged
   versions older than 30 days". GitHub enforces it automatically.

2. **Scheduled `actions/delete-package-versions` workflow** for
   commit-snapshot tags (`:sha-<short>`). Old commit pins from merged
   feature branches are rarely worth keeping forever — a monthly job
   that prunes `sha-` tags older than 90 days keeps the registry trim
   without breaking the immutable-pin promise for recent work.

Manual cleanup (the rare nuclear option) needs the `delete:packages`
scope on a personal access token plus careful selection to avoid
deleting per-arch manifests still referenced by current tags — see
`gh api -X DELETE /user/packages/container/gluco-hub/versions/<id>` in
the GitHub Packages API docs.

---

## Troubleshooting

| Symptom | Likely cause | Fix |
| ------- | ------------ | --- |
| `[CFG001]` on startup | Missing required config field (e.g. `api_secret`) | Ensure the corresponding `GLUCO_HUB__*` env var is exported before starting. |
| `[CFG003]` on startup | `password_file` path not readable | Check the file path and permissions; the error message includes the path. |
| `[CFG006]` on startup | TOML configures a Source/Sink whose Cargo feature is not compiled in | Use a build that includes the feature (the published GHCR image bundles `source-llu sink-nightscout sink-mqtt`), or remove the unused `[…]` block from the config. |
| `[CFG007]` on startup | A referenced secret env var resolved to empty | Export the env var before starting (e.g. `GLUCO_HUB__SINK__NIGHTSCOUT__API_SECRET`). The error message names the offending field. |
| `[LLU002]` / `[LLU004]` | Wrong region or app version rejected | Check `region` in config; bump `version` via `GLUCO_HUB__SOURCE__LLU__VERSION=4.17.0`. |
| `[LLU003]` | Invalid credentials | Re-check email / password in the LibreLinkUp mobile app. |
| `[NS002]` | Wrong Nightscout api-secret | `GLUCO_HUB__SINK__NIGHTSCOUT__API_SECRET` must be the plain-text secret, not the SHA-1 hash. |
| `503` on `/glucose/latest` | Cache empty before first poll | Normal at startup; wait one poll interval (default 60 s). |
| App version rejected by LibreView | LibreView raised minimum version | Set `GLUCO_HUB__SOURCE__LLU__VERSION=4.17.0` (or latest) and restart. |

Full error-code reference: [`docs/ARCHITECTURE.md#error-code-namespaces`](./ARCHITECTURE.md#error-code-namespaces).

---

## Supply chain

`cargo deny check` enforces the policy in [`deny.toml`](../deny.toml):

- **licenses** — explicit allow-list (MIT, Apache-2.0, BSD-2/3, ISC, MPL-2.0, Zlib, CC0-1.0, Unicode-3.0/DFS-2016, CDLA-Permissive-2.0).
- **bans** — `openssl`, `openssl-sys`, `native-tls`, `git2` denied (rustls only).
- **sources** — only `crates.io`; git-deps and unknown registries fail.
- **advisories** — yanked crates fail.

```bash
cargo install --locked cargo-deny
cargo deny check
```

---

## Continuous integration

Two GitHub Actions workflows still live under [`docs/ci/`](./ci/) pending a token with the `workflow` scope. Install them with such a token:

```bash
mkdir -p .github/workflows
cp docs/ci/ci-workflow.yml   .github/workflows/ci.yml
cp docs/ci/deny-workflow.yml .github/workflows/deny.yml
git add .github/workflows/
git commit -m "ci: install build + cargo-deny workflows"
git push
```

- **`ci.yml`** — `fmt` / `clippy` / `test` / release `build` on every push and PR. Wires `GLUCO_HUB_GIT_SHA=${{ github.sha }}` into the release build. Concurrency group cancels superseded PR runs; cache saves only on `main`.
- **`deny.yml`** — `cargo deny --all-features` on every push, PR, and weekly cron. Renovate is also configured — see [`renovate.json`](../renovate.json) — with grouped tokio / serde / axum+tower / tracing PRs, weekly `lockFileMaintenance`, and automerge gated on green CI.

```mermaid
graph LR
    Push[git push / PR] --> CI[ci.yml\nfmt · clippy · test · build]
    Push --> Deny[deny.yml\ncargo deny check]
    Cron[weekly cron] --> Deny
    Renovate[Renovate bot] --> Push
    CI -->|green| Automerge[automerge\nRenovate PRs]
    Deny -->|green| Automerge
```
