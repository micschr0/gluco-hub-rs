// SPDX-License-Identifier: AGPL-3.0-or-later

use std::sync::{Arc, RwLock};

use crate::model::Reading;

/// Read-heavy cache holding the most recent `Reading` observed across all
/// sources. The lock is only held for the duration of a clone or a small
/// timestamp comparison — never across an `.await` — so `std::sync::RwLock`
/// is preferred over the tokio variant.
#[derive(Debug, Clone, Default)]
pub struct ReadingCache {
    inner: Arc<RwLock<Option<Reading>>>,
}

impl ReadingCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a clone of the latest reading, if any.
    pub fn latest(&self) -> Option<Reading> {
        self.inner
            .read()
            .expect("ReadingCache RwLock poisoned")
            .clone()
    }

    /// Merge a batch of readings into the cache, keeping the one with the
    /// highest `timestamp`. Empty batches are a no-op.
    pub fn update(&self, batch: &[Reading]) {
        let Some(newest_in_batch) = batch.iter().max_by_key(|r| r.timestamp) else {
            return;
        };

        let mut guard = self.inner.write().expect("ReadingCache RwLock poisoned");
        let should_replace = match guard.as_ref() {
            None => true,
            Some(existing) => newest_in_batch.timestamp > existing.timestamp,
        };
        if should_replace {
            *guard = Some(newest_in_batch.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{GlucoseMgDl, PatientId, SourceId, Trend};
    use chrono::{TimeZone, Utc};

    fn reading(secs: i64, value: f64) -> Reading {
        Reading {
            patient_id: PatientId::new("p1").unwrap(),
            source_id: SourceId::new("primary").unwrap(),
            timestamp: Utc.timestamp_opt(secs, 0).unwrap(),
            glucose: GlucoseMgDl::new(value).unwrap(),
            trend: Trend::Flat,
        }
    }

    #[test]
    fn empty_cache_returns_none() {
        let cache = ReadingCache::new();
        assert!(cache.latest().is_none());
    }

    #[test]
    fn empty_batch_is_noop() {
        let cache = ReadingCache::new();
        cache.update(&[]);
        assert!(cache.latest().is_none());
    }

    #[test]
    fn keeps_newest_when_out_of_order() {
        let cache = ReadingCache::new();
        let r_old = reading(1_000, 100.0);
        let r_new = reading(2_000, 110.0);
        cache.update(&[r_new.clone(), r_old]);
        let latest = cache.latest().expect("cache populated");
        assert_eq!(latest.timestamp, r_new.timestamp);
        assert_eq!(latest.glucose.get(), 110.0);
    }

    #[test]
    fn does_not_overwrite_with_older_reading() {
        let cache = ReadingCache::new();
        cache.update(&[reading(2_000, 110.0)]);
        cache.update(&[reading(1_000, 100.0)]);
        let latest = cache.latest().expect("cache populated");
        assert_eq!(latest.glucose.get(), 110.0);
    }
}
