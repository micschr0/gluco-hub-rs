<div align="center">

# gluco-hub-rs

![MSRV](https://img.shields.io/badge/MSRV-1.95-orange)
![License](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue)
![Status](https://img.shields.io/badge/status-beta-yellow)

**CGM glucose readings —> HTTP API -> Nightscout -> MQTT -> ...**

[Features](#features) • [Quick start](#quick-start) • [Configuration](#configuration) • [HTTP API](#http-api) • [MQTT](#mqtt) • [Container](#container) • [Documentation](#documentation) • [Troubleshooting](#troubleshooting--feedback)

</div>

> [!WARNING]
> **Beta / WIP** — pre-1.0; APIs, config schema, and MQTT wire format may break between releases without notice.

---

gluco-hub-rs is a small self-hosted relay between your CGM like LibreLink Up and the rest of your stack: a local HTTP API by default, optional Nightscout and MQTT sinks, modular by design.

> [!NOTE]
> **For:** technical LibreLink Up users moving CGM data between systems.
> **Not for:** therapy, dosing, diagnosis, or replacing approved tools — see [DISCLAIMER.md](./DISCLAIMER.md).
> **Vibe:** picking up the Rust ecosystem and agentic coding side by side — the project is the practice ground for both.

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
- **Lightweight** — self-contained Rust binary, small footprint. runs on Raspberry Pi, VPS, or home server
- **Modular design** — add sources or sinks with a single file plus a feature flag ([how-to](./docs/EXTENDING.md))
- **Operable** — Prometheus metrics, structured JSON logs, graceful shutdown on `SIGINT`/`SIGTERM`

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

Starts the service with an in-memory mock source to verify the API works before touching any credentials:

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

```bash
cargo build --release --features "source-llu sink-nightscout"
cp config.example.toml config.toml   # edit [source.llu] and [sink.nightscout] — email & region live here

# Secrets go in env vars, never in the TOML file:
export GLUCO_HUB__SOURCE__LLU__PASSWORD='…'
export GLUCO_HUB__SINK__NIGHTSCOUT__API_SECRET='…'

./target/release/gluco-hub -c config.toml check-config   # validate and exit
./target/release/gluco-hub -c config.toml run
```

## Cargo features

`source-llu`, `sink-nightscout` (default), plus optional `mock-source` and `sink-mqtt` — see [`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md#cargo-features) for details. Published GHCR images bundle every stable Source/Sink (`source-llu sink-nightscout sink-mqtt`); only choose a narrower feature set when building locally.

```bash
cargo build --release --features "source-llu sink-nightscout sink-mqtt"
```

## Configuration

Copy `config.example.toml`, uncomment the sections you need, and pass secrets via `GLUCO_HUB__*` environment variables. Secrets are never stored in the TOML file.

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

`/glucose/*` becomes Bearer-protected only when `GLUCO_HUB__HTTP__BEARER_TOKEN` is set. Every response also carries the header `X-Disclaimer: not-for-medical-use`. Example reading response:

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

`<prefix>` is the `topic_prefix` value from `[sink.mqtt]`. The `patient` field is omitted when `include_patient_id = false` (privacy on shared brokers). The schema is versioned via the `v` field — see [`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md) for the full payload contracts. Home Assistant auto-discovery (MQTT) is planned for V3.

## Container

Multi-arch images (`linux/amd64`, `linux/arm64`) are published to GHCR on every release tag and on every push to `main`. Versions use [CalVer-on-SemVer](./CHANGELOG.md) (`YYYY.MMDD.PATCH`, e.g. `2026.510.0`). Tags split into three stability channels:

| Tag                   | Mover                                  | Stability                                | Use case                       |
| --------------------- | -------------------------------------- | ---------------------------------------- | ------------------------------ |
| `:main`               | every push to `main`                   | unstable, may break                      | dev tracking, brave testers    |
| `:testing`            | latest pre-release tag                 | RC / beta / alpha — pre-validation only  | beta channel                   |
| `:sha-<short>`        | immutable                              | snapshot of one commit                   | reproducible pinning           |
| `:YYYY.MMDD.PATCH-rc.N` | immutable                            | release candidate                        | pre-release tests              |
| `:YYYY.MMDD.PATCH`    | immutable                              | a specific final release                 | production pin                 |
| `:YYYY.MMDD`          | rolls forward to latest `PATCH` of that day | day-level rolling                   | auto-patch within a day        |
| `:YYYY`               | rolls forward to latest release in year     | year-level rolling                  | "current year" tracking        |
| `:latest`             | rolls forward, finals only             | highest final release (excludes `-rc`)   | "always current" convenience   |
| `:stable`             | rolls forward, finals only             | semantic alias of `:latest`              | "always stable" convenience    |

> [!WARNING]
> This project is in **beta**. Every release is a dated snapshot — breaking changes can land on any release while config schema, HTTP API, and MQTT wire format stabilise. For predictable upgrades, pin to an immutable tag (`:YYYY.MMDD.PATCH` or `:sha-<short>`). `:latest` / `:stable` track the most recent final release but their underlying digests move.

```bash
docker run --rm -p 8080:8080 \
    -v "$PWD/config.toml:/etc/gluco-hub/config.toml:ro" \
    -e GLUCO_HUB__SOURCE__LLU__PASSWORD='…' \
    -e GLUCO_HUB__SINK__NIGHTSCOUT__API_SECRET='…' \
    ghcr.io/micschr0/gluco-hub:latest run -c /etc/gluco-hub/config.toml
```

For a persistent setup, use the provided Compose file:

```bash
cp compose.example.yml compose.yml   # edit environment variables
cp config.example.toml config.toml   # edit [source.llu] and [sink.*]
docker compose up -d
```

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

Bug reports and feature requests are welcome via [GitHub issues](https://github.com/micschr0/gluco-hub-rs/issues). Before opening a PR, read [DISCLAIMER.md](./DISCLAIMER.md) to ensure the change fits the project's intent. For adding new sources or sinks, see [docs/EXTENDING.md](./docs/EXTENDING.md). For security-sensitive reports, see [SECURITY.md](./SECURITY.md).
