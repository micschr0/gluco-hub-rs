// SPDX-License-Identifier: AGPL-3.0-or-later

//! Nightscout sink. Authenticates via `api-secret` (v1 entries API) or a
//! JWT minted from an access token (v3 entries API); see [`client`].

pub mod client;
pub mod sink;
pub mod wire;

pub use client::NightscoutClient;
pub use sink::NightscoutSink;
