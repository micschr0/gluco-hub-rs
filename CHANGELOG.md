# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- **Home Assistant MQTT auto-discovery** (V3) — opt-in via `discovery_enabled = true` in `[sink.mqtt]`. The sink publishes a retained config message on `<discovery_prefix>/sensor/gluco_hub_<client_id>_glucose/config` after every ConnAck so HA picks the glucose sensor up automatically. State reads `mgdl` from `<prefix>/glucose`, availability tracks the `online` flag in `<prefix>/_health`, and the full JSON body is exposed as entity attributes (trend, source, patient, ts). New config keys: `discovery_enabled` (default `false`), `discovery_prefix` (default `homeassistant`), `device_name` (optional override).
- **Container `:develop` channel** — every push to the `develop` branch now builds a multi-arch image tagged `:develop` + `:sha-<short>`. Lets contributors and testers pull bleeding-edge V3 work without waiting for a release. See README#Container for the full tag matrix.
- **Sink backfill via per-sink watermark** (V3) — each sink is now wrapped in a `SinkRouter` that tracks the highest reading timestamp it has successfully pushed. The fan-out only delivers strictly-newer readings per cycle. Two consequences: (a) the MQTT sink no longer republishes the full ~24 h `graphData` batch every minute — only the new reading; (b) when a sink fails, its watermark stays put and the next poll-cycle replays the missed window automatically (LLU's 24 h history covers most realistic outages — no on-disk DLQ required). New Prometheus counters `cgm_sink_filtered_total` and `cgm_sink_replayed_total` make the behaviour visible. Watermarks are in-memory and reset on restart (persisting them is tracked as part of the V3 DLQ work).
