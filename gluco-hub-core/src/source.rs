// SPDX-License-Identifier: AGPL-3.0-or-later

use async_trait::async_trait;

use crate::error::CoreError;
use crate::model::{Reading, SourceId};

/// A CGM data source. Implementations poll an upstream service and return
/// the most recent readings they can observe.
///
/// Sources are expected to be cheap to clone (typically `Arc`-backed) so the
/// same instance can be shared between the poller task and any inspection
/// endpoints.
#[async_trait]
pub trait Source: Send + Sync + 'static {
    /// Stable identifier for this source instance — used in logs/metrics.
    fn id(&self) -> &SourceId;

    /// Fetch the latest available readings, oldest first.
    ///
    /// Implementations should authenticate transparently (refreshing tokens
    /// when needed) and surface transport errors as `CoreError::Source`.
    async fn fetch_latest(&self) -> Result<Vec<Reading>, CoreError>;
}
