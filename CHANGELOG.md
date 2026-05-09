# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project does not yet follow Semantic Versioning — see [SCOPE.md](./SCOPE.md) for the current roadmap.

## [Unreleased]

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
