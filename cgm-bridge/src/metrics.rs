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
pub const GAUGE_GLUCOSE: &str = "cgm_glucose_mgdl";

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
    *guard = Some(handle.clone());
    Ok(handle)
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
    describe_gauge!(
        GAUGE_GLUCOSE,
        Unit::Count,
        "Most recently observed glucose value in mg/dL"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_recorder_is_idempotent() {
        let h1 = init_recorder().expect("first install");
        let h2 = init_recorder().expect("second call must not error");
        // Both handles render to the same exposition (same global registry).
        assert_eq!(h1.render(), h2.render());
    }
}
