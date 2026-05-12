# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- **Home Assistant MQTT auto-discovery** (V3) — opt-in via `discovery_enabled = true` in `[sink.mqtt]`. The sink publishes a retained config message on `<discovery_prefix>/sensor/gluco_hub_<client_id>_glucose/config` after every ConnAck so HA picks the glucose sensor up automatically. State reads `mgdl` from `<prefix>/glucose`, availability tracks the `online` flag in `<prefix>/_health`, and the full JSON body is exposed as entity attributes (trend, source, patient, ts). New config keys: `discovery_enabled` (default `false`), `discovery_prefix` (default `homeassistant`), `device_name` (optional override).
