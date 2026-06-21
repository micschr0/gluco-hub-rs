# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [2026.621.0] - 2026-06-21

### Fixed

- **Tailscale MagicDNS resolution** — peer list iteration was broken because the Tailscale local API returns `Peer` as a JSON object (map keyed by node-key), not an array. `as_array()` always returned `None`, silently falling back to the configured `broker_host`. Fixed by using `as_object()?.values()`.
- **`GET /api/v1/status` — real MQTT and DLQ values** — `mqtt.connected` was hard-coded `true` and `dlq.depth` was hard-coded `0`. Now wired to shared atomics: MQTT connected reflects actual ConnAck/disconnect events; DLQ depth tracks the live queue size across restarts.
- **`GET /api/v1/status` — `llu.connected` reflects poll failures** — previously derived from `last_successful_reading_at.is_some()` which stays `true` forever after the first poll. Now cleared by a new `last_poll_failed_at` field set on every error/timeout and cleared on the next successful reading.
- **DLQ dedup key** — `merge_dedup` keyed entries on `(patient_id, timestamp_secs)`. In multi-source deployments two sources can produce readings with the same patient_id + timestamp, causing silent data loss. Added `source_id` as the first key element.
- **Clock View `lo`/`hi` float validation** — `?lo=NaN` or values outside `[20, 600]` mg/dL produced broken JavaScript in `window.CLOCK_CONFIG`. Both `GET /clock` and `GET /clock/state` now validate parameters and return `400` with a descriptive error.
- **LLU rate-limit error code** — HTTP 429 responses collapsed into `LluError::Transport` (`[LLU001]`). Added dedicated `LluError::RateLimited` variant with code `[LLU010]` carrying the `Retry-After` value.
- **Secret file BOM/whitespace** — `resolve_secret_file` only stripped trailing `\r\n`. A UTF-8 BOM (`﻿`) or leading whitespace (from Windows editors / `kubectl`) silently corrupted the secret. Now strips BOM first, then trims all whitespace.
- **LLU JWT base64url decoding** — replaced hand-rolled base64url decoder (linear scan, ignored residue bits) with the `base64` crate's `URL_SAFE_NO_PAD` engine for correct and validated decoding.

### Added

- **NS-Socket source scaffold (V6, feature-gated, off by default)** — new `source-ns-socket` Cargo feature and `[source.ns_socket]` config block lay the groundwork for using a Nightscout site as an upstream data source via its Socket.IO real-time feed (a standalone alternative to LibreLink Up). The `NsSocketSource` registers in the binary wiring and implements the `Source` trait, but the actual Socket.IO connect/subscribe loop is **stubbed**: it returns a typed `[NSS001] not yet implemented` error rather than panicking, so the poller surfaces a clean error code until the loop lands. Config supports `auth = "token"` (default, via `GLUCO_HUB__SOURCE__NS_SOCKET__TOKEN`) or `auth = "api_secret"` (via `GLUCO_HUB__SOURCE__NS_SOCKET__API_SECRET`), validated at the boundary; secrets stay in `SecretString` and are never logged. Adds **zero new runtime dependencies**. The verified Nightscout Socket.IO contract (default namespace, `authorize` handshake, `dataUpdate`/`sgvs` payload shape) is documented in the module and in `docs/EXTENDING.md`.

## [2026.607.2] - 2026-06-07

### Fixed

- **Clock View root path** — `GET /` now serves `clock.html` directly
  instead of a 308 redirect to `/clock`. The HA Ingress proxy proxies
  `/hassio/ingress/<slug>/` to `/` on the add-on. A redirect causes the
  browser to follow `Location: /clock` outside the Ingress URL prefix
  (outside `/api/hassio_ingress/`), resulting in a 404 on the HA server.
  Serving `clock.html` directly at `/` keeps the response inside the
  proxy context.
- **cargo-deny: waive `proc-macro-error2` unmaintained advisory** — the
  RUSTSEC advisory for the transitive `proc-macro-error2` crate is
  ignored in `deny.toml` until an upstream replacement lands, so
  `cargo deny check` stays green.

## [2026.607.1] - 2026-06-07

### Fixed

- **HA Ingress root path** — `GET /` redirects to `/clock` so the Home
  Assistant Ingress proxy serves the Clock View at the add-on root.
  (Superseded by direct serving in 2026.607.2.)

## [2026.606.0] - 2026-06-06

### Added

- **Multi-source support** — the poller now drives multiple configured
  sources concurrently (one poll loop per source), with per-source MQTT
  sinks when `per_source` is set. Includes HTTP 429 retry-after handling,
  relaxed field optionality on the LLU wire format, and an LLU schema
  fingerprint logged at startup.
- **Clock View** — new SSE-backed live glucose view at `GET /clock` with a
  polished UI and `GET /clock/history`.
- **Poll status API** — `GET /api/v1/status` exposes `PollStatus`
  (last poll, last reading, per-source health) from `AppState`.
- **End-to-end verification runbook** (`docs/VERIFICATION.md`) — structured manual verification covering automated tests, Docker testcontainer suite, Clock View E2E browser tests, HA live validation, and PHI checklist. Complements `V3_VALIDATION.md` and the CI gate.
- **V5: mTLS for MQTT** — optional `client_cert_file` and `client_key_file` fields in `[sink.mqtt]`. When both are set, the MQTT sink presents a client certificate during TLS handshake for mutual TLS authentication. Backward-compatible: omitting both fields preserves standard server-only TLS.
- **V5: JWT-as-password for LLU** — the `[source.llu] password` field now accepts a pre-obtained JWT. When the password looks like a JWT (3 segments, header starts `eyJ`), the bridge skips the LibreLink Up login call and uses the token directly as the Bearer credential. Non-JWT passwords continue through the normal login flow unchanged.
- **V5: Tailscale MagicDNS discovery for MQTT** — new optional `tailscale_hostname` field in `[sink.mqtt]`. When set, gluco-hub resolves the hostname to a tailnet IP via the local `tailscaled` daemon's HTTP API at startup and uses the resolved IP as the broker address. Falls back to `broker_host` if tailscaled is unreachable. No new Cargo dependencies — uses the existing `reqwest` client.

### Fixed

- **Nightscout v1 authentication** — the Nightscout sink now authenticates
  against the v1 API with the hashed `api-secret` header instead of the v3
  JWT flow, fixing rejected entry uploads. (#24, #39)

## [2026.524.1] - 2026-05-24

## [2026.524.0] - 2026-05-24

### Added

- **CI: Grype container CVE scan** — `release.yml` now builds a local amd64
  image after every push/release and scans it with Grype. Builds fail on
  CRITICAL CVEs; findings upload to the GitHub Security tab as SARIF. CVE
  exceptions are managed in `.grype.yaml` with mandatory 90-day review dates.
- **CI: Zizmor GitHub Actions audit** — new `zizmor` job in `ci.yml` audits
  all workflow files for template injection, unpinned actions, and excessive
  permissions (min-severity: medium).
- **`.grype.yaml`** — baseline Grype config; empty exception list with an
  inline template for adding justified, time-boxed CVE ignores.

## [2026.516.2] - 2026-05-16

### Added

- **MQTT HA-discovery: dedicated trend sensor entity** — when `discovery_enabled = true`, the sink now publishes a *second* retained config message on `<discovery_prefix>/sensor/gluco_hub_<client_id>_trend/config` alongside the existing glucose one. The trend entity reads `value_json.trend` from the same `<prefix>/glucose` topic, declares `device_class = "enum"` with the full `Trend` variant list in `options`, and shares the glucose entity's `device` block so HA groups both entities under one device. Operators get a first-class `sensor.<device>_trend` they can put directly on a dashboard card (with state→icon mapping for arrows) instead of having to wrap the glucose entity's `trend` attribute in a template sensor. The wire payload is unchanged. Backward-compatible: existing templates reading `state_attr('sensor.glucose', 'trend')` keep working because the glucose entity still exposes `json_attributes_topic`.

### Changed

- **MQTT HA-discovery: `has_entity_name: true` + `origin:` block on both entities** — the glucose and trend discovery payloads now carry `has_entity_name: true` (HA 2024+ idiom: HA renders entities as `<Device Name> Glucose` / `<Device Name> Trend` and respects user-driven renames) and an `origin: { name, sw_version, support_url }` block (HA 2024.6+ recommendation: surfaces the integration name and gluco-hub-rs version in HA's device picker). Pure visibility improvements — no entity IDs change, no breaking schema changes.

## [2026.516.1] - 2026-05-16

### Changed

- **Security reporting now goes through GitHub Private Vulnerability Reporting** — `SECURITY.md` no longer lists a maintainer email; sensitive reports use the in-platform [advisory form](https://github.com/micschr0/gluco-hub-rs/security/advisories/new) instead. Public bug reports still go to GitHub issues. Reduces address harvesting from the public repo and routes coordinated disclosure through GitHub's audit trail.

## [2026.516.0] - 2026-05-16

### Added

- **`http.enabled` toggle + liveness heartbeat file** — new optional `[http] enabled` field (default `true`, backward-compatible). Setting `enabled = false` (or `GLUCO_HUB__HTTP__ENABLED=false`) suppresses the embedded axum listener entirely so MQTT-only deployments (e.g. the Home Assistant add-on) don't run an unused TCP server. The poller and all sinks behave identically regardless. Liveness is now observable independently from HTTP via the heartbeat file at `<state.dir>/.alive` — atomically rewritten after every poll iteration (success, fetch error, OR timeout) so Docker/Supervisor healthchecks can use `stat -c %Y` on it instead of probing port 8080. A configured `bearer_token` is ignored with a one-shot startup WARN when `enabled = false`. The state directory is now created unconditionally on startup (was previously created lazily by the DLQ; needed for the heartbeat path when DLQ is off).

### Changed

- **Disclaimer JSON field removed from `/glucose/*` responses (BREAKING wire-format change)** — the `disclaimer` field is no longer carried in the JSON body. Consumers that read `response.disclaimer` will see `undefined` and should switch to the `X-Disclaimer: not-for-medical-use` HTTP header (present on every response) or to `DISCLAIMER.md` for the long-form text. Rationale: the body-carried string duplicated the signal three other paths already convey (header, startup banner, DISCLAIMER.md) and added ~150 bytes to every reading response. The test in `gluco-hub/src/api/glucose.rs` now asserts the field is absent (regression sentinel).
- **Startup banner closer** — `print_disclaimer_banner` now ends with `Use at your own risk.` on its own line (was previously inline with the `See SCOPE.md, DISCLAIMER.md, LICENSE.` reference). Banner grew from 6 to 7 content lines. Matches the canonical disclaimer phrasing used across docs.
- **`DISCLAIMER.md` (en) + `Cargo.toml` `description`** — appended `Use at your own risk.` so every disclaimer-bearing surface closes with the same sentence.
- **Startup disclaimer banner + HTTP-API disclaimer string** now spell out three previously-implicit risks alongside the existing not-for-medical-use posture: (a) the project is unofficial and not affiliated with Abbott; (b) use may violate Abbott's LibreLink Up Terms of Service; (c) the software is provided "as is" — use at your own risk. The banner gains two lines (now six), and `READING_DISCLAIMER` (inlined into every `/glucose/*` JSON body) is expanded from `"Not for medical use. Research only."` to the full multi-sentence statement so API consumers that only parse the body see the complete warning. Triggered by aligning the ha-libre-glucose-mqtt addon's disclaimer wording with the upstream binary's startup output.

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

[Unreleased]: https://github.com/micschr0/gluco-hub-rs/compare/v2026.621.0...HEAD
[2026.621.0]: https://github.com/micschr0/gluco-hub-rs/compare/v2026.607.2...v2026.621.0
[2026.607.2]: https://github.com/micschr0/gluco-hub-rs/compare/v2026.607.1...v2026.607.2
[2026.607.1]: https://github.com/micschr0/gluco-hub-rs/compare/v2026.606.0...v2026.607.1
[2026.606.0]: https://github.com/micschr0/gluco-hub-rs/compare/v2026.524.1...v2026.606.0
[2026.524.1]: https://github.com/micschr0/gluco-hub-rs/compare/v2026.524.0...v2026.524.1
[2026.524.0]: https://github.com/micschr0/gluco-hub-rs/compare/v2026.516.2...v2026.524.0
[2026.516.2]: https://github.com/micschr0/gluco-hub-rs/compare/v2026.516.1...v2026.516.2
[2026.516.1]: https://github.com/micschr0/gluco-hub-rs/compare/v2026.516.0...v2026.516.1
[2026.516.0]: https://github.com/micschr0/gluco-hub-rs/compare/v2026.515.0...v2026.516.0
[2026.515.0]: https://github.com/micschr0/gluco-hub-rs/compare/v2026.513.0...v2026.515.0
