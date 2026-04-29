use async_trait::async_trait;

use crate::error::CoreError;
use crate::model::Reading;

/// A destination for normalised readings. V1 ships a Nightscout sink;
/// V2 adds MQTT.
#[async_trait]
pub trait Sink: Send + Sync + 'static {
    /// Human-readable name used in logs/metrics (e.g. `"nightscout"`).
    fn name(&self) -> &'static str;

    /// Push a batch of readings. Implementations must be idempotent — the
    /// poller may resend the same reading on retry.
    async fn push(&self, readings: &[Reading]) -> Result<(), CoreError>;
}
