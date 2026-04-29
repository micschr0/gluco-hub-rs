# cgm-bridge

Rust service that polls LibreLink Up, exposes glucose readings via HTTP,
and pushes them to Nightscout.

```
LibreLink Up  ──poll──▶  in-memory cache  ──┬──▶  GET /glucose/latest
                                            └──▶  Nightscout v3 (POST entries)
```

## Quick start

```bash
# 1. Build with the source/sink features you need.
cargo build --release --features "source-llu sink-nightscout"

# 2. Copy the sample config and edit the [source.llu] / [sink.nightscout]
#    blocks. The TOML never holds secrets — it points at env-var names.
cp config.example.toml config.toml

# 3. Export the secrets the config references.
export LLU_PASSWORD='…'
export NIGHTSCOUT_API_SECRET='…'

# 4. Validate the config (returns non-zero with a CFG003 line if a
#    referenced env var is missing).
./target/release/cgm-bridge -c config.toml check-config

# 5. Run.
./target/release/cgm-bridge -c config.toml run
```

## Endpoints

| Path                | Method | Auth        | Body / format                          |
| ------------------- | ------ | ----------- | -------------------------------------- |
| `/healthz`          | GET    | public      | `{"status":"ok","version":"…"}`        |
| `/metrics`          | GET    | public      | Prometheus text exposition (v0.0.4)    |
| `/glucose/latest`   | GET    | optional Bearer | latest cached reading or 503 + `API001` |

`/glucose/*` becomes Bearer-protected when `[http] bearer_token_env` is set
in the config; `/healthz` and `/metrics` always stay public.

## Configuration

TOML with environment-variable references for every secret. See
`config.example.toml` for the full schema. Any value can be overridden
via `CGM_BRIDGE__SECTION__KEY=…`.

Required environment variables (only when their TOML block is present):

| TOML field                              | Env var (example)            | Holds                  |
| --------------------------------------- | ---------------------------- | ---------------------- |
| `[source.llu] password_env`             | `LLU_PASSWORD`               | LibreLink Up password  |
| `[sink.nightscout] api_secret_env`      | `NIGHTSCOUT_API_SECRET`      | Nightscout API secret  |
| `[http] bearer_token_env`               | `CGM_BRIDGE_BEARER_TOKEN`    | API Bearer token       |

Secrets never appear in TOML, logs, or `Debug` output.

## Cargo features

| Feature           | Effect                                                                 |
| ----------------- | ---------------------------------------------------------------------- |
| `mock-source`     | Default. Wires an in-memory canned source so the API runs out of the box. |
| `source-llu`      | Real LibreLink Up source. Honours `[source.llu]`; takes precedence over `mock-source`. |
| `sink-nightscout` | Nightscout v3 sink. Honours `[sink.nightscout]`; fans out from the poller. |

## Metrics

Exported on `/metrics`:

| Metric                            | Type    | Labels                  |
| --------------------------------- | ------- | ----------------------- |
| `cgm_cache_updates_total`         | counter | —                       |
| `cgm_source_fetch_success_total`  | counter | `source_id`             |
| `cgm_source_fetch_errors_total`   | counter | `error_code`            |
| `cgm_sink_push_success_total`     | counter | `sink`                  |
| `cgm_sink_push_errors_total`      | counter | `sink`, `error_code`    |
| `cgm_glucose_mgdl`                | gauge   | `patient_id`, `source_id` |

Stable error-code prefixes (`CORE0xx`, `CFG0xx`, `API0xx`, `LLU0xx`,
`NS0xx`, `AUTH0xx`) are used both as metric labels and in JSON log
fields, so dashboards and alerts can keep grep-clean rules across
versions.

## Operations

- Logs: structured JSON on stdout. Set `CGM_BRIDGE_LOG_PRETTY=1` for
  human-readable output during development.
- Filtering: `RUST_LOG=cgm_bridge=debug,reqwest=info`.
- Shutdown: `SIGINT` and `SIGTERM` both trigger graceful shutdown.

## Documentation

- `CLAUDE.md` — engineering conventions (errors, logging, secrets, …).
- `config.example.toml` — schema reference.

## License

MIT OR Apache-2.0.
