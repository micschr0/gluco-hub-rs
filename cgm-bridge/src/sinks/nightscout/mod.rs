//! Nightscout v3 sink. Iteration 10 ships only the HTTP client + entry
//! serialization; the `Sink` trait impl, `[sink.nightscout]` config, and
//! poller fan-out land in iteration 11.

#![allow(dead_code, unused_imports)]

pub mod client;
pub mod wire;

pub use client::{NightscoutClient, NsError};
pub use wire::{NsDirection, NsEntry, entry_from_reading};
