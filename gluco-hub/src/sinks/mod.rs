// SPDX-License-Identifier: AGPL-3.0-or-later

//! Concrete `Sink` implementations. Each lives behind its own Cargo
//! feature so the binary stays small when only some sinks are enabled.

#[cfg(feature = "sink-nightscout")]
pub mod nightscout;

#[cfg(feature = "sink-mqtt")]
pub mod mqtt;
