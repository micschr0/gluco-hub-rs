// SPDX-License-Identifier: AGPL-3.0-or-later

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// Trend arrow reported by a CGM. Fixed set — never accept raw strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Trend {
    DoubleUp,
    SingleUp,
    FortyFiveUp,
    Flat,
    FortyFiveDown,
    SingleDown,
    DoubleDown,
    NotComputable,
    RateOutOfRange,
}

/// Newtype for patient identifiers from upstream sources.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PatientId(String);

impl PatientId {
    pub fn new(value: impl Into<String>) -> Result<Self, CoreError> {
        let s: String = value.into();
        if s.is_empty() || s.len() > 128 {
            return Err(CoreError::InvalidId {
                kind: "PatientId",
                reason: "must be 1..=128 chars".into(),
            });
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Newtype for source instance identifiers (config-defined).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceId(String);

impl SourceId {
    pub fn new(value: impl Into<String>) -> Result<Self, CoreError> {
        let s: String = value.into();
        if s.is_empty() || s.len() > 64 {
            return Err(CoreError::InvalidId {
                kind: "SourceId",
                reason: "must be 1..=64 chars".into(),
            });
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Glucose value in mg/dL. Validated on construction.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GlucoseMgDl(f64);

impl GlucoseMgDl {
    /// Plausibility bounds: a reading outside `[20, 600]` mg/dL is treated
    /// as a sensor/transport error rather than a real measurement.
    pub const MIN: f64 = 20.0;
    pub const MAX: f64 = 600.0;

    pub fn new(value: f64) -> Result<Self, CoreError> {
        if !value.is_finite() || !(Self::MIN..=Self::MAX).contains(&value) {
            return Err(CoreError::InvalidGlucose { value });
        }
        Ok(Self(value))
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

/// One CGM reading, normalised across sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reading {
    pub patient_id: PatientId,
    pub source_id: SourceId,
    pub timestamp: DateTime<Utc>,
    pub glucose: GlucoseMgDl,
    pub trend: Trend,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glucose_rejects_out_of_range() {
        assert!(GlucoseMgDl::new(0.0).is_err());
        assert!(GlucoseMgDl::new(1000.0).is_err());
        assert!(GlucoseMgDl::new(f64::NAN).is_err());
        assert!(GlucoseMgDl::new(120.0).is_ok());
    }

    #[test]
    fn patient_id_rejects_empty() {
        assert!(PatientId::new("").is_err());
        assert!(PatientId::new("p1").is_ok());
    }

    #[test]
    fn source_id_rejects_too_long() {
        let long = "x".repeat(65);
        assert!(SourceId::new(long).is_err());
        assert!(SourceId::new("primary").is_ok());
    }
}
