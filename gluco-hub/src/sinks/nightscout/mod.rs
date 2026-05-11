// SPDX-License-Identifier: AGPL-3.0-or-later

//! Nightscout v3 sink.

pub mod client;
pub mod sink;
pub mod wire;

pub use client::NightscoutClient;
pub use sink::NightscoutSink;
