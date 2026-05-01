# cgm-bridge

Rust service that polls LibreLink Up, exposes glucose readings via HTTP,
and pushes them to Nightscout.

```
LibreLink Up  ──poll──▶  in-memory cache  ──┬──▶  GET /glucose/latest
                                            └──▶  Nightscout v3 (POST entries)
```

## Quick start

For a credentials-free smoke test against the in-memory `MockSource`
(useful as the very first thing on a fresh checkout):

```bash
bash scripts/smoke.sh
```

It builds the binary, boots it on `127.0.0.1:18080`, hits `/healthz`,
`/glucose/latest`, and `/metrics`, then sends `SIGTERM`. Exit code `0`
means the full `Source → cache → API` path works end-to-end. No LLU
account, no Nightscout instance, no env vars required.

**Before you wire Nightscout** — once you have an LLU account, run
the live one-shot probe:

```bash
export LLU_EMAIL='you@example.com'
export LLU_PASSWORD='…'
export LLU_REGION='EU'      # optional, defaults to EU
# export LLU_VERSION='4.17.0'   # if LibreView rejects 4.16.0
# export LLU_PATIENT_ID='…'     # multi-patient accounts
bash scripts/llu-dryrun.sh
```

It logs in, lists connections, fetches one graph, and prints a
single-line JSON summary on stdout — *without* an HTTP server,
without writing the cache, and without touching Nightscout.
Exit codes: `0` ok, `2` config/env, `3` invalid credentials,
`4` status / protocol / version mismatch, `5` transport / WAF.
This is the cheapest way to confirm the LLU side works in
isolation before debugging the rest of the pipeline.

**Confirm Nightscout reachability** — once LLU is verified, prove
the sink path with a read-only probe:

```bash
export NS_BASE_URL='https://nightscout.example.com'
export NS_API_SECRET='…'         # or NS_API_SECRET_FILE=/run/secrets/ns
bash scripts/ns-dryrun.sh
```

It hits `GET /api/v3/entries?count=1` exactly once — **never POSTs**
— and prints `{base_url, last_entry_date_ms, last_entry_age_secs}`.
Exit codes: `0` ok, `2` config/env / invalid URL, `3` transport,
`4` 401/403 auth, `5` unexpected status. Catches a wrong api-secret
or unreachable host before the bridge writes anything.

**One command for all three** — `scripts/full-dryrun.sh` chains
smoke + llu-dryrun + ns-dryrun fail-fast, skipping any live stage
whose credentials aren't set. Zero env vars → smoke-only run for CI.
Both LLU and NS env vars set → full pre-flight in under a minute.
Exit code is the offending stage (`1` smoke, `2` llu, `3` ns) or
`0` if everything green.

For a real deployment:

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
| `cgm_bridge_build_info`           | gauge (=1) | `version`, `git_sha`, `features` |
| `cgm_sink_post_retry_total`       | counter | `sink`, `attempt` |

Stable error-code prefixes (`CORE0xx`, `CFG0xx`, `API0xx`, `LLU0xx`,
`NS0xx`, `AUTH0xx`) are used both as metric labels and in JSON log
fields, so dashboards and alerts can keep grep-clean rules across
versions.

## Container

A `Containerfile` ships a multi-stage build (rust:1-bookworm →
gcr.io/distroless/cc-debian12:nonroot). The runtime image has no
shell, no package manager, and runs as uid 65532.

```bash
docker build -t cgm-bridge:dev -f Containerfile .
docker run --rm -p 8080:8080 \
    -v "$PWD/config.toml:/etc/cgm-bridge/config.toml:ro" \
    -e LLU_PASSWORD='…' \
    -e NIGHTSCOUT_API_SECRET='…' \
    cgm-bridge:dev run -c /etc/cgm-bridge/config.toml
```

Pass `--build-arg CGM_BRIDGE_GIT_SHA=$(git rev-parse HEAD)` so the
`cgm_bridge_build_info{git_sha=…}` metric label resolves to the
actual commit instead of `"unknown"`.

Liveness / readiness in Kubernetes:

```yaml
livenessProbe:
  httpGet: { path: /healthz, port: 8080 }
  initialDelaySeconds: 5
readinessProbe:
  httpGet: { path: /glucose/latest, port: 8080 }
  failureThreshold: 5  # tolerate the cache being empty pre-first-poll
```

## Operations

- Logs: structured JSON on stdout. Set `CGM_BRIDGE_LOG_PRETTY=1` for
  human-readable output during development.
- Filtering: `RUST_LOG=cgm_bridge=debug,reqwest=info`.
- Shutdown: `SIGINT` and `SIGTERM` both trigger graceful shutdown.
- LibreLink Up app version: `[source.llu] version` (TOML) or
  `CGM_BRIDGE__SOURCE__LLU__VERSION=4.17.0` (env). Bump and restart
  when LibreView rejects the pinned default; the resolved value is
  logged at INFO on startup as `llu_version`.

## Supply chain

`cargo deny check` enforces the policy in [`deny.toml`](./deny.toml):

- **licenses** — explicit allow-list (MIT, Apache-2.0, BSD-2/3,
  ISC, MPL-2.0, Zlib, CC0-1.0, Unicode-3.0/DFS-2016,
  CDLA-Permissive-2.0). Anything else fails.
- **bans** — `openssl`, `openssl-sys`, `native-tls`, `git2` are
  explicitly denied (rustls only, per `CLAUDE.md`).
- **sources** — only `crates.io`; unknown registries / git-deps fail.
- **advisories** — `cargo-deny` consults the advisory database;
  yanked crates fail.

Run locally:

```bash
cargo install --locked cargo-deny
cargo deny check
```

## Continuous integration

Two GitHub Actions workflows are checked in as drop-ins under
[`docs/ci/`](./docs/ci/) — they live there and not under
`.github/workflows/` because the development OAuth token lacks the
`workflow` scope. Install them once with a token that has
`workflow`:

```bash
mkdir -p .github/workflows
cp docs/ci/ci-workflow.yml   .github/workflows/ci.yml
cp docs/ci/deny-workflow.yml .github/workflows/deny.yml
git add .github/workflows/
git commit -m "ci: install build + cargo-deny workflows"
git push
```

- **`ci.yml`** — `fmt` / `clippy` (all-features and
  no-default-features) / `test` / release `build`. Uses
  `actions-rust-lang/setup-rust-toolchain@v1` (bundles
  `Swatinem/rust-cache@v2`); a concurrency group cancels
  superseded PR runs; the cache only saves on `main`. The release
  build wires `CGM_BRIDGE_GIT_SHA=${{ github.sha }}` so the
  `cgm_bridge_build_info` gauge carries a real commit hash.
- **`deny.yml`** — `EmbarkStudios/cargo-deny-action@v2
  --all-features` on every push, every PR, and a weekly cron.
  Renovate is also wired in — see [`renovate.json`](./renovate.json)
  — with `rangeStrategy: "update-lockfile"` (semantically correct
  for Cargo, where `"1"` already covers `1.x`), grouped tokio /
  serde / axum+tower / tracing PRs, weekly `lockFileMaintenance`,
  and automerge gated on green CI.

## Documentation

- `docs/ARCHITECTURE.md` — data flow + sequence diagrams, error-code
  reference, feature matrix, configuration reference, module map.
- `CLAUDE.md` — engineering conventions (errors, logging, secrets, …).
- `config.example.toml` — schema reference.
- `deny.toml` — supply-chain policy enforced by `cargo deny`.

## License

MIT OR Apache-2.0.
