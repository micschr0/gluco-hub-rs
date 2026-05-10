# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This project uses **CalVer-on-SemVer** versioning (`YYYY.0M0D.PATCH`,
e.g. `2026.510.0` for 2026-05-10). Versions are SemVer-parseable so Cargo
accepts them, while `MAJOR=year`, `MINOR=month*100+day`, and `PATCH` counts
same-day re-releases. Tags `v0.x.y` predate the switch and remain unchanged.
See [SCOPE.md](./SCOPE.md) for the scope roadmap.

## [Unreleased]

### Changed

- **Default Cargo features** now include `sink-nightscout` alongside `source-llu`, so `cargo run` and `cargo build` produce a binary that actually pushes data instead of silently dropping every reading.
- **Container image** now bundles `sink-mqtt` in addition to `source-llu` and `sink-nightscout`, matching the V2 roadmap. Operators with `[sink.mqtt]` in their config no longer need a custom rebuild.
- **`enabled_features()`** in `gluco_hub_build_info{features=…}` now reports `sink-mqtt` (previously omitted, even when active).
- **Container tag scheme** reorganised into three stability channels:
  - `:main` (replaces `:edge`) — every push to `main`, dev/nightly
  - `:testing` — pre-release tags only (`v*-rc.N`, `v*-beta.N`, `v*-alpha.N`)
  - `:latest` + `:stable` — final tags only; suffix-gated `enable=` filters
    ensure no RC ever moves `:latest`, `:stable`, or the floating
    `:YYYY` / `:YYYY.MMDD` tags.

### Added

- **`[CFG006]`** — `verify_features` rejects configs that reference a Source/Sink whose Cargo feature is not compiled in. Replaces the previous silent-ignore behaviour.
- **`[CFG007]`** — `verify_secrets` rejects empty secret strings (an unset `GLUCO_HUB__…` env var deserialises into `SecretString("")`, which the `config` crate cannot reject by length). Covers `[source.llu] password`, `[http] bearer_token`, `[sink.nightscout] api_secret`, `[sink.mqtt] password`.
- **Startup warning** when no sink is configured, mirroring the existing "no source configured" warning so misconfiguration is visible in the logs instead of inferable only from `sink_count=0`.
- **`cargo release` workflow** — single-command tagged releases via `release.toml` (workspace) and `[package.metadata.release]` in `gluco-hub/Cargo.toml`. `task release:dry` previews, `task release` cuts today's CalVer (UTC date), `task release:patch` bumps `PATCH` for same-day hotfixes. `task check` runs as a pre-release gate before tagging.
- **README "Image Tags" reference** — every published GHCR tag with mover, stability, and use case columns.

## [0.1.0] - 2026-05-09

Initial open-source release.

### Added

- **LibreLink Up source** — polls the LLU cloud API for CGM readings with token caching and deduplication
- **Nightscout v3 sink** — pushes glucose entries; idempotent via `deviceId`-based deduplication
- **MQTT v5 sink** — publishes to `<prefix>/glucose`, `<prefix>/_health` (retained, LWT), and `<prefix>/_stats` (retained); schema versioned as `v:1`
- **HTTP API** — `GET /healthz`, `GET /metrics` (Prometheus), `GET /glucose/latest` with optional Bearer auth
- **In-memory mock source** — `mock-source` Cargo feature; runs without LLU credentials for smoke-testing
- **Config validation** — `check-config` subcommand validates every field and resolves all secret references before service start
- **Dry-run modes** — `dryrun` and `ns-dryrun` subcommands for credential validation without starting the server
- **Prometheus metrics** — poll counts, sink push counts, dedup skips, cache hits, and build identity gauge
- **Container build** — 4-stage cargo-chef Dockerfile; runtime on `distroless/cc-debian12:nonroot`
- **Docker Compose template** — `compose.example.yml` for persistent setups
- **ENV overrides** — any TOML key can be overridden via `GLUCO_HUB__SECTION__KEY`
- **Graceful shutdown** — `SIGINT` and `SIGTERM` both drain in-flight work cleanly
- **Structured JSON logs** — `tracing` + `tracing-subscriber`; `GLUCO_HUB_LOG_PRETTY=1` for human-readable output

### Fixed

- Source fetch and HTTP clients now carry explicit connect and read timeouts — previously a hung LLU API could block the poll loop indefinitely

[Unreleased]: https://github.com/micschr0/gluco-hub-rs/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/micschr0/gluco-hub-rs/releases/tag/v0.1.0
