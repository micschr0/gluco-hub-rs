// SPDX-License-Identifier: AGPL-3.0-or-later

//! Persistent per-sink dead-letter queue (V3).
//!
//! Wraps an inner `Sink` so that failed pushes accumulate on disk and
//! retry automatically on the next successful push, surviving process
//! restarts and arbitrary sink-side outages (beyond LLU's 24 h
//! `graphData` replay window covered by `SinkRouter`).
//!
//! Layering: `SinkRouter (watermark)` → `DlqSink (persistence)` → real
//! sink (NS / MQTT). `SinkRouter` advances its watermark only on a
//! successful `DlqSink::push`, which means the watermark reflects "all
//! readings up to here are confirmed delivered" — including any drained
//! DLQ entries.
//!
//! On-disk format: one JSON-encoded `Reading` per line at
//! `<state_dir>/dlq/<sink>.jsonl`. Atomic writes via
//! `tempfile::NamedTempFile::persist` so a crash mid-write never
//! corrupts the file. Bounded by `max_entries` — on overflow the oldest
//! readings (lowest timestamps) are evicted and counted in
//! `cgm_dlq_evicted_total`.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use gluco_hub_core::{CoreError, Reading, Sink};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::metrics;

/// Bounds + serialization wrapper for one persisted reading entry.
/// Versioned via `v` so a future schema bump can deserialize old files
/// while reading; for now this is plain `v: 1`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DlqEntry {
    v: u8,
    reading: Reading,
}

pub struct DlqSink {
    inner: Arc<dyn Sink>,
    name: &'static str,
    file_path: PathBuf,
    max_entries: usize,
    /// In-memory mirror of the on-disk queue. Sorted by
    /// `(patient_id, timestamp)`; held under a tokio mutex because
    /// every operation crosses an `.await`.
    state: Mutex<Vec<Reading>>,
}

impl DlqSink {
    /// Open (or create-empty) the DLQ for `inner`. Existing on-disk
    /// entries are loaded into memory so the first `push` already
    /// includes them in the retry attempt.
    pub fn open(
        inner: Arc<dyn Sink>,
        state_dir: &std::path::Path,
        max_entries: usize,
    ) -> std::io::Result<Self> {
        let name = inner.name();
        let dlq_dir = state_dir.join("dlq");
        std::fs::create_dir_all(&dlq_dir)?;
        let file_path = dlq_dir.join(format!("{name}.jsonl"));

        let queue = if file_path.exists() {
            load_queue(&file_path)?
        } else {
            Vec::new()
        };
        if !queue.is_empty() {
            info!(
                sink = name,
                size = queue.len(),
                path = %file_path.display(),
                "dlq: loaded persisted entries"
            );
        }
        ::metrics::gauge!(metrics::GAUGE_DLQ_SIZE, "sink" => name).set(queue.len() as f64);

        Ok(Self {
            inner,
            name,
            file_path,
            max_entries,
            state: Mutex::new(queue),
        })
    }

    /// Visible for tests.
    #[cfg(test)]
    async fn current_size(&self) -> usize {
        self.state.lock().await.len()
    }
}

#[async_trait]
impl Sink for DlqSink {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn push(&self, batch: &[Reading]) -> Result<(), CoreError> {
        let mut guard = self.state.lock().await;

        // Merge: queued + new batch, deduplicated by (patient_id, timestamp).
        // Pre-merge size lets us emit the right enqueued/evicted counts
        // when the push fails.
        let pre_queue_len = guard.len();
        let merged = merge_dedup(&guard, batch);
        let pre_cap_len = merged.len();
        let (final_set, evicted) = enforce_cap(merged, self.max_entries);

        if evicted > 0 {
            warn!(
                sink = self.name,
                evicted,
                cap = self.max_entries,
                "dlq: cap exceeded, evicted oldest entries"
            );
            ::metrics::counter!(metrics::COUNTER_DLQ_EVICTED, "sink" => self.name)
                .increment(evicted as u64);
        }

        // If the merged set is empty, there's nothing to push and
        // nothing to write — fast path for the steady state where DLQ
        // is empty and the SinkRouter filter happened to drop everything.
        if final_set.is_empty() {
            *guard = Vec::new();
            // Best-effort cleanup of any stale file from a prior run.
            if self.file_path.exists() {
                let _ = std::fs::remove_file(&self.file_path);
            }
            return Ok(());
        }

        // Attempt the inner push. Release the mutex during the await
        // is NOT what we want — we must keep ordering deterministic.
        // The tokio Mutex held across `.await` is the documented pattern.
        let push_result = self.inner.push(&final_set).await;

        match push_result {
            Ok(()) => {
                let drained = final_set.len();
                debug!(
                    sink = self.name,
                    drained, "dlq: push succeeded, clearing queue"
                );
                *guard = Vec::new();
                // Best-effort delete — atomic-write would also work but
                // delete keeps the dir tidy.
                if self.file_path.exists()
                    && let Err(e) = std::fs::remove_file(&self.file_path)
                {
                    warn!(
                        sink = self.name,
                        error = %e,
                        "dlq: failed to remove queue file after drain"
                    );
                }
                ::metrics::gauge!(metrics::GAUGE_DLQ_SIZE, "sink" => self.name).set(0.0);
                ::metrics::counter!(metrics::COUNTER_DLQ_DRAINED, "sink" => self.name)
                    .increment(drained as u64);
                Ok(())
            }
            Err(e) => {
                // Persist the merged set so a crash here loses nothing.
                let final_set_len = final_set.len();
                if let Err(io_err) = persist_queue(&self.file_path, &final_set) {
                    warn!(
                        sink = self.name,
                        error = %io_err,
                        "dlq: failed to persist queue after sink failure"
                    );
                }
                let newly_enqueued = final_set_len.saturating_sub(pre_queue_len);
                if newly_enqueued > 0 {
                    ::metrics::counter!(metrics::COUNTER_DLQ_ENQUEUED, "sink" => self.name)
                        .increment(newly_enqueued as u64);
                }
                ::metrics::gauge!(metrics::GAUGE_DLQ_SIZE, "sink" => self.name)
                    .set(final_set_len as f64);
                debug!(
                    sink = self.name,
                    queue_size = final_set_len,
                    newly_enqueued,
                    pre_cap_len,
                    "dlq: sink failed, queue persisted"
                );
                *guard = final_set;
                Err(e)
            }
        }
    }
}

/// Merge `existing` and `batch` into a single set keyed by
/// `(patient_id, timestamp_secs)`, sorted oldest-first. Later occurrences
/// (from `batch`) win on collisions — important when LLU later returns a
/// trend-corrected version of the same timestamp.
fn merge_dedup(existing: &[Reading], batch: &[Reading]) -> Vec<Reading> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<(String, i64), Reading> = BTreeMap::new();
    for r in existing.iter().chain(batch.iter()) {
        map.insert(
            (r.patient_id.as_str().to_string(), r.timestamp.timestamp()),
            r.clone(),
        );
    }
    map.into_values().collect()
}

/// Drop the oldest entries when over the cap. Returns the trimmed set
/// and the number of dropped entries.
fn enforce_cap(mut set: Vec<Reading>, max: usize) -> (Vec<Reading>, usize) {
    if set.len() <= max {
        return (set, 0);
    }
    let evicted = set.len() - max;
    set.drain(..evicted);
    (set, evicted)
}

fn load_queue(path: &std::path::Path) -> std::io::Result<Vec<Reading>> {
    use std::io::BufRead;
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let mut out: Vec<Reading> = Vec::new();
    for (lineno, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<DlqEntry>(&line) {
            Ok(entry) => out.push(entry.reading),
            Err(e) => {
                warn!(
                    path = %path.display(),
                    line = lineno + 1,
                    error = %e,
                    "dlq: skipping malformed line"
                );
            }
        }
    }
    Ok(out)
}

fn persist_queue(path: &std::path::Path, queue: &[Reading]) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("dlq path has no parent dir"))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    for r in queue {
        let entry = DlqEntry {
            v: 1,
            reading: r.clone(),
        };
        let line = serde_json::to_string(&entry)
            .map_err(|e| std::io::Error::other(format!("serialise dlq entry: {e}")))?;
        writeln!(tmp, "{line}")?;
    }
    tmp.persist(path)
        .map_err(|e| std::io::Error::other(format!("persist dlq file: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use gluco_hub_core::{GlucoseMgDl, PatientId, SourceId, Trend};
    use std::sync::Mutex as StdMutex;
    use tempfile::TempDir;

    fn reading_at(secs: i64) -> Reading {
        Reading {
            patient_id: PatientId::new("p1").unwrap(),
            source_id: SourceId::new("llu").unwrap(),
            timestamp: Utc.timestamp_opt(secs, 0).unwrap(),
            glucose: GlucoseMgDl::new(110.0).unwrap(),
            trend: Trend::Flat,
        }
    }

    struct ScriptedSink {
        name: &'static str,
        pushes: StdMutex<Vec<Vec<Reading>>>,
        next_results: StdMutex<Vec<Result<(), CoreError>>>,
    }

    impl ScriptedSink {
        fn new(name: &'static str, results: Vec<Result<(), CoreError>>) -> Arc<Self> {
            Arc::new(Self {
                name,
                pushes: StdMutex::new(Vec::new()),
                next_results: StdMutex::new(results),
            })
        }
        fn pushes(&self) -> Vec<Vec<Reading>> {
            self.pushes.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Sink for ScriptedSink {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn push(&self, readings: &[Reading]) -> Result<(), CoreError> {
            self.pushes.lock().unwrap().push(readings.to_vec());
            self.next_results
                .lock()
                .unwrap()
                .pop()
                .unwrap_or(Err(CoreError::Sink {
                    message: "test: unscripted call".into(),
                }))
        }
    }

    #[tokio::test]
    async fn successful_push_does_not_create_file() {
        let dir = TempDir::new().unwrap();
        let inner = ScriptedSink::new("ok", vec![Ok(())]);
        let dlq = DlqSink::open(inner.clone(), dir.path(), 1000).unwrap();
        dlq.push(&[reading_at(100)]).await.unwrap();
        assert!(!dir.path().join("dlq/ok.jsonl").exists());
        assert_eq!(inner.pushes()[0].len(), 1);
        assert_eq!(dlq.current_size().await, 0);
    }

    #[tokio::test]
    async fn failed_push_persists_queue_to_disk() {
        let dir = TempDir::new().unwrap();
        let inner = ScriptedSink::new(
            "ns",
            vec![Err(CoreError::Sink {
                message: "[NS004] 5xx".into(),
            })],
        );
        let dlq = DlqSink::open(inner.clone(), dir.path(), 1000).unwrap();
        let result = dlq.push(&[reading_at(100), reading_at(200)]).await;
        assert!(result.is_err());
        assert_eq!(dlq.current_size().await, 2);
        let path = dir.path().join("dlq/ns.jsonl");
        assert!(path.exists(), "queue must be persisted on failure");
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            contents.lines().count(),
            2,
            "two readings expected, got: {contents:?}"
        );
    }

    #[tokio::test]
    async fn recovery_drains_queue_into_next_push() {
        let dir = TempDir::new().unwrap();
        // Sequence: first call fails, second call succeeds.
        let inner = ScriptedSink::new(
            "mqtt",
            vec![
                Ok(()),
                Err(CoreError::Sink {
                    message: "fail".into(),
                }),
            ],
        );
        let dlq = DlqSink::open(inner.clone(), dir.path(), 1000).unwrap();

        let _ = dlq.push(&[reading_at(100), reading_at(200)]).await;
        assert_eq!(dlq.current_size().await, 2);

        // Cycle 2 — sink recovers, new reading arrives.
        dlq.push(&[reading_at(300)]).await.unwrap();
        assert_eq!(dlq.current_size().await, 0);

        let pushes = inner.pushes();
        assert_eq!(pushes.len(), 2);
        // Recovery push contained 100 + 200 (drained) + 300 (new).
        assert_eq!(pushes[1].len(), 3);
        assert_eq!(pushes[1][0].timestamp.timestamp(), 100);
        assert_eq!(pushes[1][2].timestamp.timestamp(), 300);
        assert!(!dir.path().join("dlq/mqtt.jsonl").exists());
    }

    #[tokio::test]
    async fn cap_evicts_oldest_when_overflow() {
        let dir = TempDir::new().unwrap();
        let inner = ScriptedSink::new(
            "cap",
            vec![Err(CoreError::Sink {
                message: "fail".into(),
            })],
        );
        let dlq = DlqSink::open(inner.clone(), dir.path(), 3).unwrap();
        // 5 readings, cap = 3 → drop oldest 2 (timestamps 100, 200).
        let _ = dlq
            .push(&[
                reading_at(100),
                reading_at(200),
                reading_at(300),
                reading_at(400),
                reading_at(500),
            ])
            .await;
        assert_eq!(dlq.current_size().await, 3);
    }

    #[tokio::test]
    async fn restart_loads_persisted_queue() {
        let dir = TempDir::new().unwrap();
        // First instance: write 3 entries via a failed push.
        {
            let inner = ScriptedSink::new(
                "ns",
                vec![Err(CoreError::Sink {
                    message: "fail".into(),
                })],
            );
            let dlq = DlqSink::open(inner.clone(), dir.path(), 1000).unwrap();
            let _ = dlq
                .push(&[reading_at(100), reading_at(200), reading_at(300)])
                .await;
            assert_eq!(dlq.current_size().await, 3);
        } // first DlqSink dropped — file remains on disk

        // Second instance: load existing queue.
        let inner2 = ScriptedSink::new("ns", vec![Ok(())]);
        let dlq2 = DlqSink::open(inner2.clone(), dir.path(), 1000).unwrap();
        assert_eq!(dlq2.current_size().await, 3, "queue restored from disk");

        // Next push (with no new readings) should drain the queue.
        dlq2.push(&[]).await.unwrap();
        assert_eq!(dlq2.current_size().await, 0);
        assert_eq!(
            inner2.pushes()[0].len(),
            3,
            "drained queue went to inner sink"
        );
    }

    #[test]
    fn merge_dedup_keeps_one_per_timestamp_and_sorts_oldest_first() {
        let a = reading_at(200);
        let b = reading_at(100);
        let c = reading_at(200); // duplicate timestamp
        let merged = merge_dedup(&[a.clone(), b.clone()], std::slice::from_ref(&c));
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].timestamp.timestamp(), 100);
        assert_eq!(merged[1].timestamp.timestamp(), 200);
    }

    #[test]
    fn enforce_cap_drops_oldest_when_over_limit() {
        let set: Vec<Reading> = (1..=5).map(|i| reading_at(i * 100)).collect();
        let (kept, evicted) = enforce_cap(set, 3);
        assert_eq!(evicted, 2);
        assert_eq!(kept.len(), 3);
        assert_eq!(kept[0].timestamp.timestamp(), 300);
        assert_eq!(kept[2].timestamp.timestamp(), 500);
    }

    #[tokio::test]
    async fn malformed_line_in_persisted_file_is_skipped() {
        let dir = TempDir::new().unwrap();
        let dlq_dir = dir.path().join("dlq");
        std::fs::create_dir_all(&dlq_dir).unwrap();
        let path = dlq_dir.join("partial.jsonl");
        // One valid + one garbage line.
        let valid = serde_json::to_string(&DlqEntry {
            v: 1,
            reading: reading_at(123),
        })
        .unwrap();
        std::fs::write(&path, format!("{valid}\nNOT-JSON\n")).unwrap();

        let inner = ScriptedSink::new("partial", vec![Ok(())]);
        let dlq = DlqSink::open(inner.clone(), dir.path(), 1000).unwrap();
        assert_eq!(
            dlq.current_size().await,
            1,
            "garbage line skipped, valid kept"
        );
    }
}
