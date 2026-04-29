//! Nightscout v3 sink.

pub mod client;
pub mod sink;
pub mod wire;

pub use client::NightscoutClient;
pub use sink::NightscoutSink;
