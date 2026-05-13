<div align="center">

# gluco-hub-rs

![MSRV](https://img.shields.io/badge/MSRV-1.95-orange)
![License](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue)
![Status](https://img.shields.io/badge/status-beta-yellow)

**CGM glucose readings —> HTTP API -> Nightscout -> MQTT -> ...**

[Features](#features) • [Quick start](#quick-start) • [Configuration](#configuration) • [HTTP API](#http-api) • [MQTT](#mqtt) • [Container](#container) • [Documentation](#documentation) • [Troubleshooting](#troubleshooting--feedback)

</div>

> [!WARNING]
> **Beta / WIP** — pre-1.0; APIs, config schema, and MQTT wire format may break between releases at any time.

---

gluco-hub-rs is a small, self-hosted relay between a CGM (currently LibreLink Up) and the rest of your stack. It exposes a local HTTP API by default and ships optional Nightscout and MQTT sinks.

> [!NOTE]
> **For:** technical LibreLink Up users moving CGM data between systems.
> **Not for:** therapy, dosing, diagnosis, or replacing approved tools — see [DISCLAIMER.md](./DISCLAIMER.md).
> **Vibe:** a practice ground for the Rust ecosystem and agentic coding.

```text
  SOURCES              CORE             SINKS              CONSUMERS
  ───────             ──────           ─────              ─────────
  LibreLink Up  ─►                ─►   HTTP API        ─► Dashboards, scripts
  + your src    ┄►   gluco-hub    ┄►   Nightscout      ─► web / apps
                     (poll +      ┄►   MQTT broker     ─► Smart pixel clocks,
                      fan-out)                            Home Assistant
                                  ┄►   + your sink
```

## Features

- **Multiple destinations** — Nightscout, MQTT for smart displays (smart pixel clocks, Home Assistant, …), HTTP API
- **Lightweight** — self-contained Rust binary with a small footprint; runs on Raspberry Pi, VPS, or home server
- **Modular design** — add sources or sinks with a single file plus a feature flag ([how-to](./docs/EXTENDING.md))
- **Resilient sinks** — per-sink watermark drops already-pushed readings each cycle and replays missed ones automatically when a sink recovers, within LLU's 24 h history
- **Persistent DLQ** — failed pushes are written to a per-sink JSONL queue on disk and replayed on the next successful push, surviving process restarts and arbitrary outage windows beyond LLU's history
- **Operable** — Prometheus metrics, structured JSON logs, graceful shutdown on `SIGINT`/`SIGTERM`

## Roadmap

- **Shipped** — LLU source · Nightscout sink · MQTT v5 sink · HTTP API with optional Bearer · HA auto-discovery · per-sink backfill (SinkRouter) · persistent DLQ
- **V5 (next)** — embedded tailscale-rs · MQTT mTLS · JWT-as-password
- **V6 (later)** — NS-Socket source (Nightscout as upstream via Socket.IO)
- **Deferred** — TUI · generic webhook sink · multi-source routing · NS v1 fallback

See [`CLAUDE.md`](./CLAUDE.md#roadmap) for the canonical, more detailed roadmap.

## Requirements

- A **FreeStyle Libre** sensor (Libre 2 / 3) linked to a **LibreLink Up** account — same email/password as the LibreLinkUp mobile app. No other CGMs supported today.
- One of:
  - **Container** — Docker or Podman; no Rust toolchain needed (see [Container](#container))
  - **Native** — Rust ≥ 1.95 to build from source

A [`Taskfile.yml`](./Taskfile.yml) wraps all common commands — run `task` to list targets (requires [go-task](https://taskfile.dev)).

## Quick start

> [!TIP]
> No Rust on your machine? Skip ahead to [Container](#container).

### 1. Smoke test — no credentials needed

Before you touch any credentials, this starts the service with an in-memory mock source to verify the API:

```bash
bash scripts/smoke.sh
```

### 2. Validate LLU credentials

One-shot probe against the real LibreLink Up API. Prints a JSON summary and exits — no server is started, nothing is written:

```bash
export LLU_EMAIL='you@example.com' LLU_PASSWORD='…' LLU_REGION='EU'
bash scripts/llu-dryrun.sh
```

`LLU_REGION` matches the server your LibreLinkUp app talks to: `AE`, `AP`, `AU`, `CA`, `CN`, `DE`, `EU`, `EU2`, `FR`, `JP`, `LA`, `RU`, `US`.

### 3. Validate the Nightscout connection

Read-only probe that fetches the last entry date and exits — never POSTs anything:

```bash
export NS_BASE_URL='https://nightscout.example.com' NS_API_SECRET='…'
bash scripts/ns-dryrun.sh
```

> [!TIP]
> `bash scripts/full-dryrun.sh` chains all three probes automatically and skips stages whose credentials are not set.

### 4. Run for real

Two flavours produce the same running service — pick whichever fits.

**Env-only (recommended, zero files):**

```bash
cargo build --release --features "source-llu sink-nightscout"

export GLUCO_HUB__SOURCE__LLU__EMAIL='you@example.com'
export GLUCO_HUB__SOURCE__LLU__PASSWORD='…'
export GLUCO_HUB__SOURCE__LLU__REGION='EU'
export GLUCO_HUB__SINK__NIGHTSCOUT__BASE_URL='https://nightscout.example.com'
export GLUCO_HUB__SINK__NIGHTSCOUT__API_SECRET='…'

./target/release/gluco-hub check-config   # validate and exit
./target/release/gluco-hub run            # no -c flag → no config.toml needed
```

**File-based (when you prefer a checked-in TOML):**

```bash
cp config.example.toml config.toml   # edit [source.llu] and [sink.nightscout] — email & region live here

# Secrets still go in env vars, never in the TOML file:
export GLUCO_HUB__SOURCE__LLU__PASSWORD='…'
export GLUCO_HUB__SINK__NIGHTSCOUT__API_SECRET='…'

./target/release/gluco-hub -c config.toml check-config
./target/release/gluco-hub -c config.toml run
```

## Cargo features

`source-llu`, `sink-nightscout` (default), plus optional `mock-source` and `sink-mqtt` — see [`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md#cargo-features) for details. Published GHCR images bundle every stable Source/Sink (`source-llu sink-nightscout sink-mqtt`); only choose a narrower feature set when building locally.

```bash
cargo build --release --features "source-llu sink-nightscout sink-mqtt"
```

## Configuration

`config.toml` is **optional** — `GLUCO_HUB__*` environment variables alone can configure everything (useful for containers / Compose / Kubernetes). When a `config.toml` is present, env vars override its values key by key. Keep secrets in env vars; the TOML file holds none.

For a file-based setup, copy `config.example.toml` and uncomment the sections you need:

```toml
[http]
bind = "0.0.0.0:8080"
# Bearer auth for /glucose/*: set GLUCO_HUB__HTTP__BEARER_TOKEN=<token>

[poller]
interval_secs = 60              # min 30, max 600; LLU updates every ~60 s

[source.llu]
email = "you@example.com"
region = "EU"                   # AE AP AU CA CN DE EU EU2 FR JP LA RU US
# Password via env:  export GLUCO_HUB__SOURCE__LLU__PASSWORD=…
# Password via file: password_file = "/run/secrets/llu_password"

[sink.nightscout]
base_url = "https://nightscout.example.com"
# Secret via env: export GLUCO_HUB__SINK__NIGHTSCOUT__API_SECRET=…

[sink.mqtt]
broker_host    = "mqtt.example.com"
broker_port    = 8883
client_id      = "gluco-hub-1"
topic_prefix   = "gluco-hub/gluco-hub-1"
# Password via env: export GLUCO_HUB__SINK__MQTT__PASSWORD=…
```

Any TOML key can also be overridden at runtime via `GLUCO_HUB__SECTION__KEY` (double-underscore as separator):

```bash
GLUCO_HUB__HTTP__BIND=0.0.0.0:9090 GLUCO_HUB__POLLER__INTERVAL_SECS=30 ./gluco-hub run
```

> [!NOTE]
> Run `./gluco-hub -c config.toml check-config` after editing the config — it validates every field before the service tries to start.

## HTTP API

| Path                  | Auth            | Response                                   |
| --------------------- | --------------- | ------------------------------------------ |
| `GET /healthz`        | public          | `{"status":"ok","version":"…"}`            |
| `GET /metrics`        | public          | Prometheus text exposition (v0.0.4)        |
| `GET /glucose/latest` | optional Bearer | Latest cached reading, or `503` + `API001` |

Setting `GLUCO_HUB__HTTP__BEARER_TOKEN` puts `/glucose/*` behind Bearer auth. Every response also carries the header `X-Disclaimer: not-for-medical-use`. Example reading response:

```json
{
  "patient_id": "00000000-0000-0000-0000-000000000000",
  "source_id": "llu",
  "timestamp": "2025-01-01T12:00:00Z",
  "glucose_mgdl": 112,
  "trend": "Flat",
  "disclaimer": "Not for medical use. Research only."
}
```

## MQTT

Requires `--features sink-mqtt` and a `[sink.mqtt]` config block.

| Topic              | Retained | Payload                                                                                          |
| ------------------ | :------: | ------------------------------------------------------------------------------------------------ |
| `<prefix>/glucose` |    No    | `{"v":1,"ts":<unix-ms>,"mgdl":<float>,"mmol":<float>,"trend":"Flat","source":"llu","patient":"…"}` |
| `<prefix>/_health` |   Yes    | `{"online":true,"v":1}` · LWT: `{"online":false,"v":1}`                                          |
| `<prefix>/_stats`  |   Yes    | `{"v":1,"uptime_secs":…,"publishes_total":…,"connects_total":…, …}` — refreshed every `stats_interval_secs` |

`<prefix>` is the `topic_prefix` value from `[sink.mqtt]`. Set `include_patient_id = false` to drop the `patient` field on shared brokers. The schema is versioned via the `v` field — see [`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md) for the full payload contracts.

**Home Assistant auto-discovery.** Set `discovery_enabled = true` in `[sink.mqtt]`. The sink then publishes a retained config message on `<discovery_prefix>/sensor/gluco_hub_<client_id>_glucose/config` (default `discovery_prefix = "homeassistant"`) after each MQTT ConnAck. Home Assistant picks the entity up automatically — state reads `mgdl` from `<prefix>/glucose`, availability tracks `<prefix>/_health` via the `online` flag, and the full JSON body is exposed as entity attributes (trend, source, patient, ts).

## Container

Multi-arch images (`linux/amd64`, `linux/arm64`) are published to GHCR on every release tag, every push to `main`, and every push to `develop` (active integration branch). Versions use [CalVer-on-SemVer](./CHANGELOG.md) (`YYYY.MMDD.PATCH`, e.g. `2026.510.0`). Tags split into four stability tiers:

| Tag                   | Mover                                  | Stability                                | Use case                       |
| --------------------- | -------------------------------------- | ---------------------------------------- | ------------------------------ |
| `:develop`            | every push to `develop`                | bleeding edge, often-broken              | preview unreleased V3 work     |
| `:main`               | every push to `main`                   | stabilising — main is release-cut source | dev tracking after V3 lands    |
| `:testing`            | latest pre-release tag                 | RC / beta / alpha — pre-validation only  | beta channel                   |
| `:sha-<short>`        | immutable                              | snapshot of one commit                   | reproducible pinning           |
| `:YYYY.MMDD.PATCH-rc.N` | immutable                            | release candidate                        | pre-release tests              |
| `:YYYY.MMDD.PATCH`    | immutable                              | a specific final release                 | production pin                 |
| `:YYYY.MMDD`          | rolls forward to latest `PATCH` of that day | day-level rolling                   | auto-patch within a day        |
| `:YYYY`               | rolls forward to latest release in year     | year-level rolling                  | "current year" tracking        |
| `:latest`             | rolls forward, finals only             | highest final release (excludes `-rc`)   | "always current" convenience   |
| `:stable`             | rolls forward, finals only             | semantic alias of `:latest`              | "always stable" convenience    |

> [!WARNING]
> This project is in **beta**. Every release is a dated snapshot — breaking changes can land on any release while the config schema, HTTP API, and MQTT wire format stabilise. For predictable upgrades, pin to an immutable tag (`:YYYY.MMDD.PATCH` or `:sha-<short>`). `:latest` / `:stable` track the most recent final release but their underlying digests move.

No config file required — everything via env vars:

```bash
docker run --rm -p 127.0.0.1:8080:8080 \
    -e GLUCO_HUB__HTTP__BIND=0.0.0.0:8080 \
    -e GLUCO_HUB__SOURCE__LLU__EMAIL='you@example.com' \
    -e GLUCO_HUB__SOURCE__LLU__PASSWORD='…' \
    -e GLUCO_HUB__SOURCE__LLU__REGION='EU' \
    -e GLUCO_HUB__SINK__NIGHTSCOUT__BASE_URL='https://nightscout.example.com' \
    -e GLUCO_HUB__SINK__NIGHTSCOUT__API_SECRET='…' \
    ghcr.io/micschr0/gluco-hub:latest
```

### Compose

For a persistent setup, use the provided Compose file — no `config.toml` needed:

```bash
cp compose.example.yml compose.yml   # tweak if you like the defaults
cp .env.example .env                 # fill in LLU + sink credentials
docker compose up -d
docker compose logs -f gluco-hub
```

The Compose file reads `.env` automatically and ignores `config.toml`. If you prefer a file-based config (e.g. a checked-in deployment repo), uncomment the `volumes:` / `command:` block at the bottom of `compose.example.yml` and bind-mount `config.toml` read-only.

### Verifying the image

Each published image is keyless-signed with [Sigstore cosign](https://docs.sigstore.dev/) and ships a SLSA build-provenance attestation. Verify either:

```bash
gh attestation verify oci://ghcr.io/micschr0/gluco-hub:latest --owner micschr0
```

### Building locally

```bash
docker build -t gluco-hub:dev \
    --build-arg GLUCO_HUB_GIT_SHA=$(git rev-parse HEAD) \
    --build-arg BUILD_DATE=$(date -u +%Y-%m-%dT%H:%M:%SZ) .
```

> [!TIP]
> `SIGINT` and `SIGTERM` both trigger a graceful shutdown. For Kubernetes, use the exec-form `ENTRYPOINT` so PID 1 receives the signal directly.

## Documentation

- [`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md) — data flow, error codes, module map, Cargo features, config reference
- [`docs/OPERATIONS.md`](./docs/OPERATIONS.md) — CLI, endpoints, MQTT topics, metrics, env vars, troubleshooting
- [`docs/EXTENDING.md`](./docs/EXTENDING.md) — how to add new sources or sinks
- [`config.example.toml`](./config.example.toml) · [`compose.example.yml`](./compose.example.yml) — config and Compose templates
- [`DISCLAIMER.md`](./DISCLAIMER.md) · [`SECURITY.md`](./SECURITY.md) · [`CHANGELOG.md`](./CHANGELOG.md) · [`LICENSE`](./LICENSE)

## Troubleshooting & feedback

For common error codes and their fixes, see the [troubleshooting table](./docs/OPERATIONS.md#troubleshooting) in the operations runbook.

Found a bug or have a question? [Open an issue](https://github.com/micschr0/gluco-hub-rs/issues) — include the error code (e.g. `LLU003`) and the output of `check-config` if the service fails to start.

## Contributing

Please file bug reports and feature requests via [GitHub issues](https://github.com/micschr0/gluco-hub-rs/issues). Before opening a PR, read [DISCLAIMER.md](./DISCLAIMER.md) to ensure the change fits the project's intent. For adding new sources or sinks, see [docs/EXTENDING.md](./docs/EXTENDING.md). For security-sensitive reports, see [SECURITY.md](./SECURITY.md).
