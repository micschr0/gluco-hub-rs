// SPDX-License-Identifier: AGPL-3.0-or-later

//! Per-sink watermark routing (V3 — Backfill).
//!
//! Wraps each `Sink` with a `last_pushed_ts` watermark so the fan-out
//! only delivers readings the sink has not seen yet. Solves two problems
//! in one mechanism:
//!
//! 1. **Burst reduction.** The LLU source returns ~24 h of `graphData`
//!    on every poll (288 readings at the 5-min raster). Without
//!    filtering, every poll-cycle re-pushes the entire batch to every
//!    sink — fine for the Nightscout sink (idempotent server-side via
//!    `deviceId`) but a 288-msg/min storm for the MQTT sink.
//!
//! 2. **Recovery after downtime.** When a sink fails, its watermark
//!    stays put. The next poll-cycle's batch still carries the missed
//!    readings (LLU's 24 h window covers most realistic outages), so
//!    the router replays the gap automatically — no per-reading queue,
//!    no on-disk DLQ.
//!
//! Watermarks are in-memory. After a process restart the watermark is
//! `None` and the next cycle re-sends the full 24 h batch to each sink
//! once — matching the prior behaviour. Persisting the watermark across
//! restarts is out of scope here and tracked under V3 DLQ.

use std::sync::Mutex;

use chrono::{DateTime, Utc};
use gluco_hub_core::{CoreError, Reading, Sink};
use tracing::debug;

/// Wraps a `Sink` with the per-sink watermark.
pub struct SinkRouter {
    sink: std::sync::Arc<dyn Sink>,
    /// Highest `timestamp` of any reading successfully pushed to the
    /// wrapped sink. `None` until the first successful push.
    watermark: Mutex<Option<DateTime<Utc>>>,
}

/// What `push_filtered` did this cycle — surfaced so the caller can
/// emit precise metrics.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PushOutcome {
    /// Number of readings in the incoming batch that were skipped
    /// because they were `<= watermark`.
    pub filtered: usize,
    /// Number of readings actually attempted (post-filter).
    pub pushed: usize,
    /// Was this a recovery cycle (i.e. did we send strictly more than
    /// 1 reading after a prior interval)? `pushed - 1` are the
    /// "replayed" readings.
    pub replayed: usize,
}

impl SinkRouter {
    pub fn new(sink: std::sync::Arc<dyn Sink>) -> Self {
        Self {
            sink,
            watermark: Mutex::new(None),
        }
    }

    pub fn name(&self) -> &'static str {
        self.sink.name()
    }

    /// Visible for tests and the HTTP API.
    pub fn watermark(&self) -> Option<DateTime<Utc>> {
        *self.watermark.lock().expect("watermark mutex poisoned")
    }

    /// Filter `batch` down to readings strictly newer than the current
    /// watermark, push them through the wrapped sink, and — only on a
    /// successful push — advance the watermark to the max timestamp in
    /// the attempted slice. On failure the watermark stays put so the
    /// next cycle retries the same window.
    pub async fn push_filtered(&self, batch: &[Reading]) -> (PushOutcome, Result<(), CoreError>) {
        let wm_before = self.watermark();
        let to_push: Vec<Reading> = match wm_before {
            None => batch.to_vec(),
            Some(wm) => batch.iter().filter(|r| r.timestamp > wm).cloned().collect(),
        };
        let outcome = PushOutcome {
            filtered: batch.len().saturating_sub(to_push.len()),
            pushed: to_push.len(),
            // First-ever push (wm = None) is not "replay" — only subsequent
            // multi-reading pushes count, since the steady state is one
            // new reading per cycle.
            replayed: match wm_before {
                None => 0,
                Some(_) => to_push.len().saturating_sub(1),
            },
        };

        if to_push.is_empty() {
            debug!(
                sink = self.sink.name(),
                filtered = outcome.filtered,
                "sink router: nothing new to push"
            );
            return (outcome, Ok(()));
        }

        let result = self.sink.push(&to_push).await;
        if result.is_ok() {
            let new_wm = to_push
                .iter()
                .map(|r| r.timestamp)
                .max()
                .expect("non-empty after early-return guard above");
            let mut guard = self.watermark.lock().expect("watermark mutex poisoned");
            // Only advance — never regress (defensive, in case the source
            // ever returns an out-of-order batch).
            if guard.is_none_or(|cur| new_wm > cur) {
                *guard = Some(new_wm);
            }
        }
        (outcome, result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::TimeZone;
    use gluco_hub_core::{GlucoseMgDl, PatientId, SourceId, Trend};
    use std::sync::Arc;

    fn reading_at(secs: i64) -> Reading {
        Reading {
            patient_id: PatientId::new("p1").unwrap(),
            source_id: SourceId::new("llu").unwrap(),
            timestamp: Utc.timestamp_opt(secs, 0).unwrap(),
            glucose: GlucoseMgDl::new(110.0).unwrap(),
            trend: Trend::Flat,
        }
    }

    /// A test sink that records every push and can be set to fail.
    struct RecordingSink {
        name: &'static str,
        seen: Mutex<Vec<Vec<Reading>>>,
        fail: Mutex<bool>,
    }

    impl RecordingSink {
        fn new(name: &'static str) -> Arc<Self> {
            Arc::new(Self {
                name,
                seen: Mutex::new(Vec::new()),
                fail: Mutex::new(false),
            })
        }

        fn set_fail(&self, fail: bool) {
            *self.fail.lock().unwrap() = fail;
        }

        fn pushes(&self) -> Vec<Vec<Reading>> {
            self.seen.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Sink for RecordingSink {
        fn name(&self) -> &'static str {
            self.name
        }

        async fn push(&self, readings: &[Reading]) -> Result<(), CoreError> {
            if *self.fail.lock().unwrap() {
                return Err(CoreError::Sink {
                    message: "test induced failure".into(),
                });
            }
            self.seen.lock().unwrap().push(readings.to_vec());
            Ok(())
        }
    }

    #[tokio::test]
    async fn first_push_sends_full_batch_and_sets_watermark() {
        let sink = RecordingSink::new("rec");
        let router = SinkRouter::new(sink.clone());

        let batch = vec![reading_at(100), reading_at(200), reading_at(300)];
        let (outcome, result) = router.push_filtered(&batch).await;

        assert!(result.is_ok());
        assert_eq!(outcome.pushed, 3);
        assert_eq!(outcome.filtered, 0);
        assert_eq!(outcome.replayed, 0, "first push is not replay");
        assert_eq!(sink.pushes().len(), 1);
        assert_eq!(sink.pushes()[0].len(), 3);
        assert_eq!(router.watermark().map(|t| t.timestamp()), Some(300));
    }

    #[tokio::test]
    async fn subsequent_push_filters_already_seen_readings() {
        let sink = RecordingSink::new("rec");
        let router = SinkRouter::new(sink.clone());

        let first = vec![reading_at(100), reading_at(200), reading_at(300)];
        let _ = router.push_filtered(&first).await;

        // Same batch + one new reading — only the new one should reach the sink.
        let second = vec![
            reading_at(100),
            reading_at(200),
            reading_at(300),
            reading_at(400),
        ];
        let (outcome, _) = router.push_filtered(&second).await;
        assert_eq!(outcome.pushed, 1);
        assert_eq!(outcome.filtered, 3);
        assert_eq!(
            outcome.replayed, 0,
            "steady-state: one new reading is not replay"
        );

        let pushes = sink.pushes();
        assert_eq!(pushes.len(), 2);
        assert_eq!(pushes[1].len(), 1);
        assert_eq!(pushes[1][0].timestamp.timestamp(), 400);
        assert_eq!(router.watermark().map(|t| t.timestamp()), Some(400));
    }

    #[tokio::test]
    async fn watermark_stays_put_on_failure_and_next_cycle_retries() {
        let sink = RecordingSink::new("rec");
        let router = SinkRouter::new(sink.clone());

        // Initial successful push.
        let _ = router.push_filtered(&[reading_at(100)]).await;
        assert_eq!(router.watermark().map(|t| t.timestamp()), Some(100));

        // Sink fails on the next cycle (e.g. broker offline). Two new readings.
        sink.set_fail(true);
        let cycle2 = vec![reading_at(100), reading_at(200), reading_at(300)];
        let (outcome2, result2) = router.push_filtered(&cycle2).await;
        assert!(result2.is_err());
        assert_eq!(outcome2.pushed, 2, "tried to push the 2 new readings");
        assert_eq!(
            router.watermark().map(|t| t.timestamp()),
            Some(100),
            "watermark must NOT advance on failure"
        );

        // Sink recovers — fourth reading arrives this cycle. Recovery replay:
        // both gap readings (200, 300) plus the new one (400).
        sink.set_fail(false);
        let cycle3 = vec![
            reading_at(100),
            reading_at(200),
            reading_at(300),
            reading_at(400),
        ];
        let (outcome3, result3) = router.push_filtered(&cycle3).await;
        assert!(result3.is_ok());
        assert_eq!(
            outcome3.pushed, 3,
            "200 + 300 + 400 = the 3 readings past the watermark"
        );
        assert_eq!(outcome3.filtered, 1, "only 100 was filtered");
        assert_eq!(outcome3.replayed, 2, "pushed - 1 new = 2 replayed");

        let pushes = sink.pushes();
        assert_eq!(
            pushes.len(),
            2,
            "first success + recovery push (failure left no record)"
        );
        assert_eq!(pushes[1].len(), 3);
        assert_eq!(router.watermark().map(|t| t.timestamp()), Some(400));
    }

    #[tokio::test]
    async fn empty_batch_after_filter_skips_sink_push() {
        let sink = RecordingSink::new("rec");
        let router = SinkRouter::new(sink.clone());

        let _ = router.push_filtered(&[reading_at(100)]).await;
        // Identical batch — entirely filtered.
        let (outcome, result) = router.push_filtered(&[reading_at(100)]).await;
        assert!(result.is_ok());
        assert_eq!(outcome.pushed, 0);
        assert_eq!(outcome.filtered, 1);
        assert_eq!(
            sink.pushes().len(),
            1,
            "second cycle skipped the sink entirely"
        );
    }

    #[tokio::test]
    async fn watermark_advances_only_forwards_even_with_out_of_order_batch() {
        let sink = RecordingSink::new("rec");
        let router = SinkRouter::new(sink.clone());

        let _ = router.push_filtered(&[reading_at(500)]).await;
        // Out-of-order batch with one new (600) and one stale (450) reading.
        // Filter drops the stale; watermark advances to 600 (not 450).
        let (outcome, _) = router
            .push_filtered(&[reading_at(450), reading_at(600)])
            .await;
        assert_eq!(outcome.pushed, 1, "stale 450 filtered, only 600 sent");
        assert_eq!(router.watermark().map(|t| t.timestamp()), Some(600));
    }
}
