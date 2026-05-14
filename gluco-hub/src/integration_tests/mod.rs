// SPDX-License-Identifier: AGPL-3.0-or-later

//! Container-backed integration tests (V3+).
//!
//! Gated by the `integration-tests` Cargo feature and `cfg(test)` so
//! they only compile + run via `cargo test --features integration-tests`.
//! Default `cargo test` skips them entirely.
//!
//! Why they live under `src/` and not in `tests/`: gluco-hub is a
//! binary-only crate (no `[lib]` target), and Cargo's integration-test
//! convention (`tests/*.rs`) requires a library to import from. Putting
//! tests here as a feature-gated module gives them full `crate::*`
//! visibility without the lib + bin split.
//!
//! Submodules:
//!  * `common` — shared helpers (test readings, ids, containers,
//!    HA-discovery-schema validator)
//!  * `mqtt`   — Phase A: MqttSink against real Mosquitto
//!  * `nightscout` — Phase B: NightscoutSink against real NS + Mongo
//!  * `multi_sink` — Phase C: NS+MQTT in parallel via fan-out
//!
//! Requires Docker on the test host.

#![allow(dead_code)] // helpers used selectively per submodule

pub mod common;

#[cfg(feature = "sink-mqtt")]
pub mod mqtt;

#[cfg(feature = "sink-nightscout")]
pub mod nightscout;

#[cfg(all(
    feature = "sink-mqtt",
    feature = "sink-nightscout",
    feature = "mock-source"
))]
pub mod multi_sink;
