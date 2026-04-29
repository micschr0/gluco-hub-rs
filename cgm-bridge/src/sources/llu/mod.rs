//! LibreLink Up source.
//!
//! Iteration 4a ships only the auth client (login flow + region routing).
//! Connections + graph fetch and the `Source` trait impl land in 4b, at
//! which point all of these symbols become live.
//!
//! Reference: <https://github.com/timoschlueter/nightscout-librelink-up>
#![allow(dead_code)]

pub mod auth;
pub mod error;
pub mod headers;
pub mod mapping;
pub mod region;
pub mod wire;

// Re-exports kept stable for the upcoming 4b iteration that wires the
// `Source` impl. They are dead-code under `source-llu` alone — silence the
// warning rather than churn the public path later.
#[allow(unused_imports)]
pub use auth::{LluAuthClient, LluCredentials, LluTokens};
#[allow(unused_imports)]
pub use error::LluError;
#[allow(unused_imports)]
pub use region::Region;
