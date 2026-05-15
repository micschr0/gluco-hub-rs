# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [2026.515.0] - 2026-05-15

### Added

- **MQTT HA-discovery: configurable glucose unit** — new optional `[sink.mqtt] discovery_unit` field accepts `"mgdl"` (default, preserves V3 behaviour) or `"mmol"`. When set to `mmol`, the discovery payload reports `unit_of_measurement = "mmol/L"` and `value_template = "{{ value_json.mmol }}"` so EU/UK Home Assistant users see mmol/L directly on the sensor entity. The wire payload is unchanged — both `mgdl` and `mmol` fields are always emitted, so subscribers other than HA see the same JSON they did before.

### Changed

- **CI supply-chain hardening: `step-security/harden-runner` egress audit** — every job in `ci.yml`, `deny.yml`, `release.yml`, and `renovate.yml` now starts with a SHA-pinned `step-security/harden-runner@v2.19.3` step in `egress-policy: audit` mode. The action installs an eBPF agent on the runner that records every outbound network call without blocking it; the resulting audit log is attached to each workflow run and visible at `app.stepsecurity.io`. This is the observation pass — once a baseline allow-list is stable across CI, deny, release, and renovate runs, a follow-up commit will flip the policy to `block` so a hijacked Action cannot exfiltrate or call out to unexpected hosts.
- **CI supply-chain hardening: minimum per-job `permissions:`** — every workflow now declares `permissions: {}` at the top level (deny-all default for `GITHUB_TOKEN`), and each job opts into the narrowest scope it needs. `ci.yml`, `deny.yml`, and `renovate.yml` only get `contents: read`. `release.yml`'s per-arch `build` jobs only add `packages: write` for digest pushes, and only the `merge` job gets `id-token: write` (cosign keyless) + `attestations: write` (SLSA build-provenance). A compromised Action in a `build` job can no longer publish a manifest, and CI/lint jobs cannot push packages or write to the repo even if an upstream Action is hijacked.
- **CI supply-chain hardening: SHA-pin all third-party Actions** — every `uses:` line across `.github/workflows/{ci,deny,release,renovate}.yml` now references a third-party Action by its full 40-char commit SHA with a `# vX.Y.Z` trailing comment, instead of the floating major tag (`@v3`, `@v4`, …). A tag-hijack on any upstream Action repo (docker/*, EmbarkStudios/cargo-deny-action, sigstore/cosign-installer, renovatebot/github-action, etc.) can no longer silently run with our GHCR `packages: write` token. Renovate's `helpers:pinGitHubActionDigestsToSemver` (already active via `config:best-practices`) keeps the SHAs current; the new `groupName: "github-actions"` rule in `renovate.json` bundles all Action bumps into a single weekly PR instead of one per Action.

### Fixed

- **Release workflow: manifest publish on develop pushes** — the "merge & publish manifest" step built its `docker buildx imagetools create` argv via unquoted command substitution. Annotation values containing shell-meaningful characters (e.g. the auto-derived GitHub-repo description `CGM glucose -> Nightscout, MQTT, HTTP-API`) word-split, so `->` reached docker as a stray flag and the step exited 125 — breaking `:develop` and `:sha-<short>` publishes since the V3 work landed. Replaced with a quoted bash-array argv so arbitrary tag, annotation, and digest values are safe (closes a CWE-88 argument-injection footgun where any field flowing into `DOCKER_METADATA_OUTPUT_JSON` could inject docker flags).
- **Release workflow: annotation level for `buildx imagetools create`** — once the argv-quoting fix above let the actual `--annotation` calls through, they surfaced a second latent bug: `docker/metadata-action` emits annotations with the default `manifest:` prefix (intended for `buildx build`), but `buildx imagetools create` rejects it with `"manifest" annotations are not supported yet` because it edits the OCI image index, not per-arch manifests. Set `DOCKER_METADATA_ANNOTATIONS_LEVELS: index` on the metadata-action step so the emitted annotations target the layer imagetools is actually writing.

### Added

- **Home Assistant MQTT auto-discovery** (V3) — opt-in via `discovery_enabled = true` in `[sink.mqtt]`. The sink publishes a retained config message on `<discovery_prefix>/sensor/gluco_hub_<client_id>_glucose/config` after every ConnAck so HA picks the glucose sensor up automatically. State reads `mgdl` from `<prefix>/glucose`, availability tracks the `online` flag in `<prefix>/_health`, and the full JSON body is exposed as entity attributes (trend, source, patient, ts). New config keys: `discovery_enabled` (default `false`), `discovery_prefix` (default `homeassistant`), `device_name` (optional override).
- **Container `:develop` channel** — every push to the `develop` branch now builds a multi-arch image tagged `:develop` + `:sha-<short>`. Lets contributors and testers pull bleeding-edge V3 work without waiting for a release. See README#Container for the full tag matrix.
- **Sink backfill via per-sink watermark** (V3) — each sink is now wrapped in a `SinkRouter` that tracks the highest reading timestamp it has successfully pushed. The fan-out only delivers strictly-newer readings per cycle. Two consequences: (a) the MQTT sink no longer republishes the full ~24 h `graphData` batch every minute — only the new reading; (b) when a sink fails, its watermark stays put and the next poll-cycle replays the missed window automatically (LLU's 24 h history covers most realistic outages — no on-disk DLQ required). New Prometheus counters `cgm_sink_filtered_total` and `cgm_sink_replayed_total` make the behaviour visible. Watermarks are in-memory and reset on restart (persisting them is tracked as part of the V3 DLQ work).
- **Persistent dead-letter queue** (V3) — `DlqSink` sits between `SinkRouter` and the real sink. Failed pushes accumulate in a per-sink JSONL file at `<state_dir>/dlq/<sink>.jsonl` (atomic writes via `tempfile::NamedTempFile::persist`) and replay on the next successful push, surviving process restarts and outages longer than LLU's 24 h history. New config: `[state] dir` (default `./state`), `[dlq] enabled` (default `true`), `[dlq] max_entries` (default `10000` ≈ 35 days at the 5-min raster). Cap-exceeding entries drop oldest-first. Four new metrics: `cgm_dlq_enqueued_total{sink}`, `cgm_dlq_drained_total{sink}`, `cgm_dlq_evicted_total{sink}`, `cgm_dlq_size{sink}` gauge.

[Unreleased]: https://github.com/micschr0/gluco-hub-rs/compare/v2026.515.0...HEAD
[2026.515.0]: https://github.com/micschr0/gluco-hub-rs/compare/v2026.513.0...v2026.515.0
