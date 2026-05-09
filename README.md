<div align="center">

# gluco-hub-rs

![MSRV](https://img.shields.io/badge/MSRV-1.95-orange)
![License](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue)
![Status](https://img.shields.io/badge/status-V3%20%C2%B7%20MQTT-brightgreen)

**CGM glucose readings —> HTTP API -> Nightscout -> MQTT -> ...**

[Features](#features) • [Quick start](#quick-start) • [Configuration](#configuration) • [HTTP API](#http-api) • [MQTT](#mqtt) • [Container](#container) • [Documentation](#documentation) • [Troubleshooting](#troubleshooting--feedback)

</div>

---

gluco-hub-rs is a small self-hosted relay between your CGM like LibreLink Up and the rest of your stack: a local HTTP API by default, optional Nightscout and MQTT sinks, modular by design.

> [!NOTE]
> **For:** technical LibreLink Up users moving CGM data between systems.
> **Not for:** therapy, dosing, diagnosis, or replacing approved tools — see [DISCLAIMER.md](./DISCLAIMER.md).

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

- A LibreLink Up account with at least one linked sensor — use the same email/password as the LibreLinkUp mobile app
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
export LLU_PASSWORD='…' NIGHTSCOUT_API_SECRET='…'   # only secrets go in env vars

./target/release/gluco-hub -c config.toml check-config   # validate and exit
./target/release/gluco-hub -c config.toml run
```

## Cargo features

`source-llu` (default), `mock-source`, `sink-nightscout`, `sink-mqtt` — see [`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md#cargo-features) for details.

```bash
cargo build --release --features "source-llu sink-nightscout sink-mqtt"
```

## Configuration

Copy `config.example.toml`, uncomment the sections you need, and set the referenced environment variables. Secrets are never stored in the file — only the name of the environment variable that holds them.

```toml
[http]
bind = "0.0.0.0:8080"                          # default
# bearer_token_env = "GLUCO_HUB_BEARER_TOKEN"  # protects /glucose/* with Bearer auth

[poller]
interval_secs = 60                             # min 30, max 600; LLU updates every ~60 s

[source.llu]
email = "you@example.com"
password_env = "LLU_PASSWORD"                  # env-var name, not the password itself
region = "EU"                                  # AE AP AU CA CN DE EU EU2 FR JP LA RU US

[sink.nightscout]
base_url = "https://nightscout.example.com"
api_secret_env = "NIGHTSCOUT_API_SECRET"

[sink.mqtt]
broker_host    = "mqtt.example.com"
broker_port    = 8883
client_id      = "gluco-hub-1"
topic_prefix   = "gluco-hub/gluco-hub-1"
password_env   = "MQTT_PASSWORD"
```

Any TOML key can be overridden at runtime via the `GLUCO_HUB__SECTION__KEY` environment variable (double-underscore as separator), useful in containers where mounting a config file is inconvenient:

```bash
GLUCO_HUB__HTTP__BIND=0.0.0.0:9090 GLUCO_HUB__POLLER__INTERVAL_SECS=30 ./gluco-hub run
```

> [!NOTE]
> Run `./gluco-hub -c config.toml check-config` after editing the config — it validates every field and resolves all secret references before the service tries to start.

## HTTP API

| Path                  | Auth            | Response                                   |
| --------------------- | --------------- | ------------------------------------------ |
| `GET /healthz`        | public          | `{"status":"ok","version":"…"}`            |
| `GET /metrics`        | public          | Prometheus text exposition (v0.0.4)        |
| `GET /glucose/latest` | optional Bearer | Latest cached reading, or `503` + `API001` |

`/glucose/*` becomes Bearer-protected only when `bearer_token_env` is configured. Every response also carries the header `X-Disclaimer: not-for-medical-use`. Example reading response:

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

```bash
docker build -t gluco-hub:dev -f Containerfile \
    --build-arg GLUCO_HUB_GIT_SHA=$(git rev-parse HEAD) \
    --build-arg BUILD_DATE=$(date -u +%Y-%m-%dT%H:%M:%SZ) .

docker run --rm -p 8080:8080 \
    -v "$PWD/config.toml:/etc/gluco-hub/config.toml:ro" \
    -e LLU_PASSWORD='…' \
    -e NIGHTSCOUT_API_SECRET='…' \
    gluco-hub:dev run -c /etc/gluco-hub/config.toml
```

For a persistent setup, use the provided Compose file:

```bash
cp compose.example.yml compose.yml   # edit environment variables
cp config.example.toml config.toml   # edit [source.llu] and [sink.*]
docker compose up -d
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
