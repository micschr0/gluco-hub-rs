// SPDX-License-Identifier: AGPL-3.0-or-later

use chrono::{DateTime, Utc};

/// Snapshot of the poll-loop's most recent timing state.
///
/// Written by the poll task via `tokio::sync::watch::Sender<PollStatus>`
/// after every iteration (attempt and success/failure). Read by the
/// `GET /api/v1/status` handler via a cloned `Receiver`.
///
/// Two separate timestamps let caregivers distinguish:
/// - **poll failures** (attempt advances but success does not), vs.
/// - **sensor transmission gaps** (neither advances).
#[derive(Debug, Clone, Default)]
pub struct PollStatus {
    /// Set to `Utc::now()` at the start of every `source.fetch_latest()` call,
    /// including iterations that end in a timeout or error.
    pub last_poll_attempt_at: Option<DateTime<Utc>>,

    /// Set to `Utc::now()` only when `source.fetch_latest()` returns a
    /// non-empty `Ok(batch)` — i.e. the upstream API was reached and
    /// returned at least one measurement.
    pub last_successful_reading_at: Option<DateTime<Utc>>,

    /// Seconds until the next scheduled poll tick fires (best-effort
    /// estimate written after each tick).
    pub next_poll_in_secs: u64,

    /// Configured poll interval in seconds. Constant after startup;
    /// included in the response so clients need not read config separately.
    pub poll_interval_secs: u64,
}
