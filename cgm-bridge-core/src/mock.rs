//! In-memory `Source` implementation for tests and local demos.
//!
//! Gated by the `mock-source` Cargo feature so it is never compiled into a
//! release binary unless explicitly enabled.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use chrono::Utc;

use crate::error::CoreError;
use crate::model::{GlucoseMgDl, PatientId, Reading, SourceId, Trend};
use crate::source::Source;

/// A `Source` that returns a single canned `Reading` whose timestamp is
/// refreshed on each call. The current value can be overridden at runtime
/// via [`MockSource::set`].
#[derive(Debug)]
pub struct MockSource {
    id: SourceId,
    patient: PatientId,
    state: Arc<RwLock<MockState>>,
}

#[derive(Debug, Clone)]
struct MockState {
    glucose: GlucoseMgDl,
    trend: Trend,
}

impl MockSource {
    pub fn new(id: SourceId, patient: PatientId, glucose: GlucoseMgDl, trend: Trend) -> Self {
        Self {
            id,
            patient,
            state: Arc::new(RwLock::new(MockState { glucose, trend })),
        }
    }

    /// Default test fixture: 120 mg/dL, flat trend.
    pub fn default_fixture() -> Result<Self, CoreError> {
        Ok(Self::new(
            SourceId::new("mock")?,
            PatientId::new("mock-patient")?,
            GlucoseMgDl::new(120.0)?,
            Trend::Flat,
        ))
    }

    pub fn set(&self, glucose: GlucoseMgDl, trend: Trend) {
        let mut guard = self.state.write().expect("MockSource RwLock poisoned");
        guard.glucose = glucose;
        guard.trend = trend;
    }
}

#[async_trait]
impl Source for MockSource {
    fn id(&self) -> &SourceId {
        &self.id
    }

    async fn fetch_latest(&self) -> Result<Vec<Reading>, CoreError> {
        let snapshot = self
            .state
            .read()
            .expect("MockSource RwLock poisoned")
            .clone();
        Ok(vec![Reading {
            patient_id: self.patient.clone(),
            source_id: self.id.clone(),
            timestamp: Utc::now(),
            glucose: snapshot.glucose,
            trend: snapshot.trend,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fetch_returns_one_reading() {
        let src = MockSource::default_fixture().expect("fixture");
        let batch = src.fetch_latest().await.expect("fetch");
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].glucose.get(), 120.0);
        assert_eq!(batch[0].source_id.as_str(), "mock");
    }

    #[tokio::test]
    async fn set_changes_subsequent_reads() {
        let src = MockSource::default_fixture().expect("fixture");
        src.set(GlucoseMgDl::new(180.0).expect("valid"), Trend::SingleUp);
        let batch = src.fetch_latest().await.expect("fetch");
        assert_eq!(batch[0].glucose.get(), 180.0);
        assert_eq!(batch[0].trend, Trend::SingleUp);
    }
}
