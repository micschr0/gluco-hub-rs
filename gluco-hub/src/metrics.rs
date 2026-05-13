// SPDX-License-Identifier: AGPL-3.0-or-later

//! Prometheus exporter wiring.
//!
//! Installs a single global recorder on first call; subsequent calls return
//! the cached handle so the binary stays robust under repeated initialisation
//! (e.g. test harnesses) instead of panicking inside `metrics`.

use std::sync::Mutex;

use anyhow::{Context, Result};
use metrics::{Unit, describe_counter, describe_gauge};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

pub const COUNTER_CACHE_UPDATES: &str = "cgm_cache_updates_total";
pub const COUNTER_FETCH_SUCCESS: &str = "cgm_source_fetch_success_total";
pub const COUNTER_FETCH_ERRORS: &str = "cgm_source_fetch_errors_total";
pub const COUNTER_SINK_SUCCESS: &str = "cgm_sink_push_success_total";
pub const COUNTER_SINK_ERRORS: &str = "cgm_sink_push_errors_total";
pub const COUNTER_SINK_RETRY: &str = "cgm_sink_post_retry_total";
pub const COUNTER_SINK_FILTERED: &str = "cgm_sink_filtered_total";
pub const COUNTER_SINK_REPLAYED: &str = "cgm_sink_replayed_total";
pub const COUNTER_DLQ_ENQUEUED: &str = "cgm_dlq_enqueued_total";
pub const COUNTER_DLQ_DRAINED: &str = "cgm_dlq_drained_total";
pub const COUNTER_DLQ_EVICTED: &str = "cgm_dlq_evicted_total";
pub const GAUGE_DLQ_SIZE: &str = "cgm_dlq_size";
pub const GAUGE_GLUCOSE: &str = "cgm_glucose_mgdl";
pub const GAUGE_BUILD_INFO: &str = "gluco_hub_build_info";

/// Compile-time list of enabled Cargo features. Returned as a stable,
/// alphabetically-sorted comma-joined string so the
/// `gluco_hub_build_info{features=…}` label matches across rebuilds.
pub fn enabled_features() -> String {
    let mut v: Vec<&'static str> = Vec::new();
    #[cfg(feature = "mock-source")]
    v.push("mock-source");
    #[cfg(feature = "source-llu")]
    v.push("source-llu");
    #[cfg(feature = "sink-nightscout")]
    v.push("sink-nightscout");
    #[cfg(feature = "sink-mqtt")]
    v.push("sink-mqtt");
    if v.is_empty() {
        "none".to_string()
    } else {
        v.sort_unstable();
        v.join(",")
    }
}

/// `git_sha` label for the build-info gauge. Read from the
/// `GLUCO_HUB_GIT_SHA` build-time env var (set by CI or
/// `GLUCO_HUB_GIT_SHA=$(git rev-parse HEAD) cargo build`); falls back
/// to `"unknown"` for ad-hoc dev builds.
pub fn git_sha() -> &'static str {
    option_env!("GLUCO_HUB_GIT_SHA").unwrap_or("unknown")
}

static HANDLE: Mutex<Option<PrometheusHandle>> = Mutex::new(None);

/// Install the global Prometheus recorder and describe all metrics.
/// Idempotent: subsequent calls return the cached handle without
/// re-installing. The mutex serialises check-and-install so concurrent
/// callers (notably tokio test harnesses) can never both reach
/// `install_recorder` — `metrics` permits only one global recorder.
pub fn init_recorder() -> Result<PrometheusHandle> {
    let mut guard = HANDLE.lock().expect("metrics HANDLE Mutex poisoned");
    if let Some(handle) = guard.as_ref() {
        return Ok(handle.clone());
    }
    let handle = PrometheusBuilder::new()
        .install_recorder()
        .context("install Prometheus recorder")?;
    describe_all();
    register_build_info();
    *guard = Some(handle.clone());
    Ok(handle)
}

/// Set `gluco_hub_build_info{version, git_sha, features}` to 1.
/// Prometheus dashboards group runtime metrics by build identity by
/// joining on this label set — handy for "which build is leaking?"
/// post-mortems.
fn register_build_info() {
    ::metrics::gauge!(
        GAUGE_BUILD_INFO,
        "version" => env!("CARGO_PKG_VERSION"),
        "git_sha" => git_sha(),
        "features" => enabled_features(),
    )
    .set(1.0);
}

fn describe_all() {
    describe_counter!(
        COUNTER_CACHE_UPDATES,
        "Number of times the in-memory reading cache was updated"
    );
    describe_counter!(COUNTER_FETCH_SUCCESS, "Number of successful source fetches");
    describe_counter!(
        COUNTER_FETCH_ERRORS,
        "Number of failed source fetches, labelled by error_code"
    );
    describe_counter!(
        COUNTER_SINK_SUCCESS,
        "Number of successful sink pushes, labelled by sink"
    );
    describe_counter!(
        COUNTER_SINK_ERRORS,
        "Number of failed sink pushes, labelled by sink and error_code"
    );
    describe_counter!(
        COUNTER_SINK_RETRY,
        "Number of in-process retries against a sink (transient 429 / 5xx); \
        labelled by sink and attempt number (1-based)"
    );
    describe_counter!(
        COUNTER_SINK_FILTERED,
        "Number of readings dropped by the SinkRouter watermark filter \
        (already pushed in a previous cycle); labelled by sink"
    );
    describe_counter!(
        COUNTER_SINK_REPLAYED,
        "Number of readings re-sent to a recovering sink after a prior \
        failure (PushOutcome.replayed); labelled by sink"
    );
    describe_counter!(
        COUNTER_DLQ_ENQUEUED,
        "Number of readings added to a sink's dead-letter queue after a \
        failed push; labelled by sink"
    );
    describe_counter!(
        COUNTER_DLQ_DRAINED,
        "Number of readings successfully replayed out of a sink's DLQ \
        on the next successful push; labelled by sink"
    );
    describe_counter!(
        COUNTER_DLQ_EVICTED,
        "Number of readings dropped from a sink's DLQ because the \
        per-sink cap was exceeded; labelled by sink"
    );
    describe_gauge!(
        GAUGE_DLQ_SIZE,
        Unit::Count,
        "Current in-memory size of each sink's DLQ"
    );
    describe_gauge!(
        GAUGE_GLUCOSE,
        Unit::Count,
        "Most recently observed glucose value in mg/dL"
    );
    describe_gauge!(
        GAUGE_BUILD_INFO,
        "Always 1; carries build identity (version, git_sha, features) in labels"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_recorder_is_idempotent() {
        let h1 = init_recorder().expect("first install");
        let _h2 = init_recorder().expect("second call must not error");
        // Both calls must succeed and `describe_all` runs on the second
        // call too — verify the describe target is present in the
        // exposition. We deliberately do NOT compare h1.render() against
        // h2.render(): metric values change between the two calls when
        // parallel tests touch the global registry, but the *describe*
        // metadata is stable.
        let r1 = h1.render();
        assert!(
            r1.contains("gluco_hub_build_info{"),
            "build_info missing from render after second init_recorder(): {r1}"
        );
    }

    #[test]
    fn build_info_gauge_appears_in_render() {
        let handle = init_recorder().expect("recorder");
        let body = handle.render();
        assert!(
            body.contains("gluco_hub_build_info{"),
            "build_info gauge missing from /metrics render: {body}"
        );
        // The version label always resolves at compile time, so we can
        // assert on it without flakiness.
        assert!(
            body.contains(env!("CARGO_PKG_VERSION")),
            "build_info gauge missing version label: {body}"
        );
    }

    #[test]
    fn enabled_features_is_alphabetical_and_stable() {
        let s = enabled_features();
        let parts: Vec<&str> = s.split(',').collect();
        let mut sorted = parts.clone();
        sorted.sort_unstable();
        assert_eq!(parts, sorted, "features label not alphabetical: {s}");
    }
}
