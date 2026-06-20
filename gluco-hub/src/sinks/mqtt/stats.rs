// SPDX-License-Identifier: AGPL-3.0-or-later

//! Live counter state for the MQTT sink, periodically serialised into
//! the `<prefix>/_stats` payload.
//!
//! All mutation is funnelled through `record_*` methods so the sink and
//! poll-loop never poke struct fields directly. Snapshots are pure-data
//! and can be taken without holding the lock across the publish.

use std::sync::Mutex;
use std::time::Instant;

use chrono::Utc;

use super::wire::StatsSnapshot;

/// Live counters for the MQTT sink. Wrapped in `std::sync::Mutex`
/// because every method does only trivial integer work — no `.await`
/// is held across the lock, so the blocking primitive is correct here.
#[derive(Debug)]
pub struct MqttStatsState {
    started_at: Instant,
    publishes_total: u64,
    publish_errors_total: u64,
    connects_total: u64,
    last_publish_ts_ms: Option<i64>,
    last_connect_ts_ms: Option<i64>,
}

impl MqttStatsState {
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
            publishes_total: 0,
            publish_errors_total: 0,
            connects_total: 0,
            last_publish_ts_ms: None,
            last_connect_ts_ms: None,
        }
    }

    /// Increment by `n` successful glucose publishes and stamp the
    /// most recent timestamp. Called from `MqttSink::push` after the
    /// `publish_with_properties` future resolves Ok for each reading.
    pub fn record_publish(&mut self, n: u64) {
        self.publishes_total = self.publishes_total.saturating_add(n);
        self.last_publish_ts_ms = Some(Utc::now().timestamp_millis());
    }

    pub fn record_publish_error(&mut self) {
        self.publish_errors_total = self.publish_errors_total.saturating_add(1);
    }

    /// Called from the poll loop on every successful ConnAck. The
    /// first call counts as the initial connect; subsequent calls are
    /// reconnects (operators compute reconnects as `connects_total - 1`).
    pub fn record_connect(&mut self) {
        self.connects_total = self.connects_total.saturating_add(1);
        self.last_connect_ts_ms = Some(Utc::now().timestamp_millis());
    }

    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            uptime_secs: self.started_at.elapsed().as_secs(),
            publishes_total: self.publishes_total,
            publish_errors_total: self.publish_errors_total,
            connects_total: self.connects_total,
            last_publish_ts_ms: self.last_publish_ts_ms,
            last_connect_ts_ms: self.last_connect_ts_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_start_at_zero() {
        let s = Mutex::new(MqttStatsState::new());
        let snap = s.lock().unwrap().snapshot();
        assert_eq!(snap.publishes_total, 0);
        assert_eq!(snap.publish_errors_total, 0);
        assert_eq!(snap.connects_total, 0);
        assert!(snap.last_publish_ts_ms.is_none());
        assert!(snap.last_connect_ts_ms.is_none());
    }

    #[test]
    fn record_publish_accumulates_and_stamps() {
        let s = Mutex::new(MqttStatsState::new());
        {
            let mut g = s.lock().unwrap();
            g.record_publish(3);
            g.record_publish(2);
        }
        let snap = s.lock().unwrap().snapshot();
        assert_eq!(snap.publishes_total, 5);
        assert!(snap.last_publish_ts_ms.is_some());
    }

    #[test]
    fn record_publish_error_independent_of_success_counter() {
        let s = Mutex::new(MqttStatsState::new());
        {
            let mut g = s.lock().unwrap();
            g.record_publish_error();
            g.record_publish_error();
        }
        let snap = s.lock().unwrap().snapshot();
        assert_eq!(snap.publish_errors_total, 2);
        assert_eq!(snap.publishes_total, 0);
        // Errors do not stamp last_publish_ts_ms.
        assert!(snap.last_publish_ts_ms.is_none());
    }

    #[test]
    fn record_connect_counts_each_connack() {
        let s = Mutex::new(MqttStatsState::new());
        {
            let mut g = s.lock().unwrap();
            g.record_connect();
            g.record_connect();
            g.record_connect();
        }
        let snap = s.lock().unwrap().snapshot();
        assert_eq!(snap.connects_total, 3);
        assert!(snap.last_connect_ts_ms.is_some());
    }
}
