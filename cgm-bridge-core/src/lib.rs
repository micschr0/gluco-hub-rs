//! Domain types, `Source`/`Sink` traits, and shared error type for cgm-bridge.

pub mod cache;
pub mod error;
pub mod model;
pub mod sink;
pub mod source;

#[cfg(feature = "mock-source")]
pub mod mock;

pub use cache::ReadingCache;
pub use error::CoreError;
pub use model::{GlucoseMgDl, PatientId, Reading, SourceId, Trend};
pub use sink::Sink;
pub use source::Source;

#[cfg(feature = "mock-source")]
pub use mock::MockSource;
