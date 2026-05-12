// SPDX-License-Identifier: AGPL-3.0-or-later

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use gluco_hub_core::{Reading, ReadingCache, Sink, Source};
use tracing::{error, info, warn};

mod api;
mod config;
mod metrics;
mod sink_router;
mod sinks;
mod sources;

#[cfg(all(test, feature = "source-llu", feature = "sink-nightscout"))]
mod e2e_tests;

#[derive(Debug, Parser)]
#[command(
    name = "gluco-hub",
    about = "LibreLink Up → HTTP/Nightscout/MQTT bridge"
)]
struct Cli {
    /// Path to the configuration file. Defaults to `config.toml` in the CWD.
    #[arg(long, short = 'c', global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the polling loop and HTTP API.
    Run,
    /// Validate the configuration and exit.
    CheckConfig,
    /// One-shot LLU connectivity probe: log in, list connections,
    /// fetch one graph, print a JSON summary to stdout. No HTTP server,
    /// no Nightscout push, no cache writes. Use BEFORE wiring the sink
    /// to confirm the operator's LLU credentials + region + version
    /// actually work against the live API.
    #[cfg(feature = "source-llu")]
    Dryrun,
    /// One-shot Nightscout connectivity probe: read-only
    /// `GET /api/v3/entries?count=1`. Confirms the api-secret hashes
    /// correctly and the NS host is reachable, WITHOUT writing any
    /// entry. Counterpart of `dryrun` for the sink side.
    #[cfg(feature = "sink-nightscout")]
    NsDryrun,
}

fn main() -> ExitCode {
    print_disclaimer_banner();
    init_tracing();

    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            // `{:#}` walks the anyhow source chain so the inner
            // [CFGxxx]/[LLUxxx] code is visible in the JSON error field —
            // not just the outer `.context(...)` label.
            error!(error = format!("{e:#}"), "fatal");
            ExitCode::FAILURE
        }
    }
}

/// First-match prefix-to-exit-code lookup. The string match survives
/// `anyhow::Context` wrapping so the exit-code contract is stable
/// across refactors that change error chain formatting.
#[cfg(any(feature = "source-llu", feature = "sink-nightscout", test))]
fn classify_by_prefix(err_text: &str, table: &[(&str, u8)]) -> ExitCode {
    for (prefix, code) in table {
        if err_text.contains(prefix) {
            return ExitCode::from(*code);
        }
    }
    ExitCode::FAILURE
}

/// `scripts/llu-dryrun.sh` exit-code contract. Order matters: the
/// first matching prefix wins.
#[cfg(any(feature = "source-llu", test))]
const DRYRUN_EXIT_TABLE: &[(&str, u8)] = &[
    ("[LLU003]", 3), // invalid credentials
    ("[LLU002]", 4), // status / version mismatch
    ("[LLU004]", 4), // protocol
    ("[LLU006]", 4), // unknown region
    ("[LLU007]", 4), // bad timestamp
    ("[LLU001]", 5), // transport
    ("[LLU005]", 5), // redirect loop
    ("[CFG", 2),     // config / env (any CFG0xx)
];

/// `scripts/ns-dryrun.sh` exit-code contract.
#[cfg(any(feature = "sink-nightscout", test))]
const NSDRYRUN_EXIT_TABLE: &[(&str, u8)] = &[
    ("[NS005]", 2), // invalid base URL → config bucket
    ("[CFG", 2),    // config / env
    ("[NS001]", 3), // transport
    ("[NS002]", 4), // 401 / 403 auth
    ("[NS003]", 5), // unexpected status
    ("[NS004]", 5), // retryable
];

/// Surface the not-for-medical-use posture once on every binary start,
/// on every subcommand. Printed to stderr BEFORE tracing initialisation
/// so it appears even when the operator pipes stdout JSON to a log
/// aggregator and uses stderr for human-readable startup output.
fn print_disclaimer_banner() {
    eprintln!("===========================================================");
    eprintln!(
        "gluco-hub-rs v{} — NOT FOR MEDICAL USE",
        env!("CARGO_PKG_VERSION")
    );
    eprintln!("Research and self-hosting tool. No warranty.");
    eprintln!("Not for therapy, dosing, or diagnosis.");
    eprintln!("See SCOPE.md, DISCLAIMER.md, LICENSE.");
    eprintln!("===========================================================");
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let builder = tracing_subscriber::fmt().with_env_filter(filter);

    if std::env::var_os("GLUCO_HUB_LOG_PRETTY").is_some() {
        builder.init();
    } else {
        builder.json().init();
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn run(cli: Cli) -> Result<ExitCode> {
    let cfg = config::load(cli.config.as_deref()).context("failed to load configuration")?;

    match cli.command {
        Command::CheckConfig => {
            config::verify_features(&cfg).context("verify enabled features match config")?;
            config::verify_secrets(&cfg).context("verify configured secrets")?;
            info!(
                http_addr = %cfg.http.bind,
                llu_configured = cfg.source.llu.is_some(),
                "configuration ok"
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Run => {
            serve(cfg).await?;
            Ok(ExitCode::SUCCESS)
        }
        #[cfg(feature = "source-llu")]
        Command::Dryrun => match dryrun(&cfg).await {
            Ok(()) => Ok(ExitCode::SUCCESS),
            Err(e) => {
                let txt = format!("{e:#}");
                error!(error = %txt, "llu dryrun failed");
                Ok(classify_by_prefix(&txt, DRYRUN_EXIT_TABLE))
            }
        },
        #[cfg(feature = "sink-nightscout")]
        Command::NsDryrun => match nsdryrun(&cfg).await {
            Ok(()) => Ok(ExitCode::SUCCESS),
            Err(e) => {
                let txt = format!("{e:#}");
                error!(error = %txt, "ns dryrun failed");
                Ok(classify_by_prefix(&txt, NSDRYRUN_EXIT_TABLE))
            }
        },
    }
}

async fn serve(cfg: config::Config) -> Result<()> {
    // Fail fast if a TOML block references a Sink/Source whose Cargo
    // feature is not compiled in — otherwise the operator gets silent
    // data loss instead of a clear startup error.
    config::verify_features(&cfg).context("verify enabled features match config")?;
    // Fail fast if any referenced secret env var is missing — better to
    // crash on startup than to start serving 401s after the bearer token
    // ended up empty.
    config::verify_secrets(&cfg).context("verify configured secrets")?;

    let metrics_handle = metrics::init_recorder().context("init metrics recorder")?;
    let cache = ReadingCache::new();
    let bearer_token = resolve_bearer_token(&cfg);
    info!(auth_enabled = bearer_token.is_some(), "http auth state");
    let state = api::AppState {
        cache: cache.clone(),
        metrics_handle,
        bearer_token,
    };

    let sinks = build_sinks(&cfg)?;
    info!(sink_count = sinks.len(), "sinks configured");
    if sinks.is_empty() {
        warn!(
            "no sink configured; readings will populate the in-memory cache and \
             HTTP API only — nothing is pushed downstream"
        );
    }

    if let Some(source) = build_default_source(&cfg)? {
        let interval = Duration::from_secs(cfg.poller.interval_secs);
        let cache_for_poller = cache.clone();
        let sinks_for_poller = sinks.clone();
        tokio::spawn(async move {
            poll_loop(source, cache_for_poller, sinks_for_poller, interval).await;
        });
    } else {
        warn!("no source configured; HTTP API will serve 503 until one is enabled");
    }

    let router = api::router(state);
    let listener = tokio::net::TcpListener::bind(cfg.http.bind)
        .await
        .with_context(|| format!("bind {}", cfg.http.bind))?;

    info!(addr = %cfg.http.bind, "http server listening");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("http server")?;
    Ok(())
}

fn resolve_bearer_token(cfg: &config::Config) -> Option<secrecy::SecretString> {
    cfg.http.bearer_token.clone()
}

/// Build the source to drive the poller. Priority order:
/// 1. `source-llu` feature + `[source.llu]` block configured → LLU.
/// 2. `mock-source` feature → MockSource fixture.
/// 3. Otherwise, no source (HTTP API stays up; data endpoints serve 503).
fn build_default_source(cfg: &config::Config) -> Result<Option<Arc<dyn Source>>> {
    // Mark `cfg` used in every feature combination, including the
    // no-default-features build where neither branch below compiles.
    let _ = cfg;

    #[cfg_attr(
        not(any(feature = "source-llu", feature = "mock-source")),
        allow(unused_mut)
    )]
    let mut source: Option<Arc<dyn Source>> = None;

    #[cfg(feature = "source-llu")]
    if let Some(llu) = cfg.source.llu.as_ref() {
        source = Some(build_llu_source(llu).context("build LLU source")?);
    }

    #[cfg(feature = "mock-source")]
    if source.is_none() {
        let mock =
            gluco_hub_core::MockSource::default_fixture().context("build MockSource fixture")?;
        source = Some(Arc::new(mock));
    }

    Ok(source)
}

/// Resolve `[source.llu]` into a wired `LluAuthClient` + `LluCredentials`
/// plus the version string actually sent in the `version` header. Shared
/// between the long-running `serve` path and the one-shot `dryrun` probe
/// so they cannot disagree on header values or env-var resolution.
#[cfg(feature = "source-llu")]
fn build_llu_client_and_creds(
    llu: &config::LluSourceConfig,
) -> Result<(
    sources::llu::auth::LluAuthClient,
    sources::llu::auth::LluCredentials,
    String,
)> {
    use secrecy::SecretString;
    use sources::llu::Region;
    use sources::llu::auth::{LluAuthClient, LluCredentials};
    use sources::llu::headers::DEFAULT_LLU_VERSION;

    let region = Region::parse(&llu.region).context("parse LLU region")?;
    let password: SecretString = match (llu.password.as_ref(), llu.password_file.as_deref()) {
        (Some(secret), None) => secret.clone(),
        (None, Some(path)) => SecretString::from(
            config::resolve_secret_file(path).map_err(|e| anyhow::anyhow!("{e}"))?,
        ),
        _ => anyhow::bail!("[CFG002] exactly one of password or password_file must be set"),
    };
    let resolved_version = llu
        .version
        .as_deref()
        .unwrap_or(DEFAULT_LLU_VERSION)
        .to_string();
    let client = LluAuthClient::new()
        .context("build LLU HTTP client")?
        .with_version(&resolved_version);
    let creds = LluCredentials {
        email: llu.email.clone(),
        password,
        region,
    };
    Ok((client, creds, resolved_version))
}

#[cfg(feature = "source-llu")]
fn resolve_source_tz(llu: &config::LluSourceConfig) -> Result<chrono_tz::Tz> {
    llu.timezone
        .as_deref()
        .unwrap_or("UTC")
        .parse()
        .with_context(|| {
            format!(
                "invalid IANA timezone in [source.llu] timezone: {:?}",
                llu.timezone
            )
        })
}

#[cfg(feature = "source-llu")]
fn build_llu_source(llu: &config::LluSourceConfig) -> Result<Arc<dyn Source>> {
    use chrono_tz::Tz;
    use gluco_hub_core::{PatientId, SourceId};
    use sources::llu::source::{ConnectionSelection, LluSource};

    let (client, creds, resolved_version) = build_llu_client_and_creds(llu)?;
    let selection = match llu.patient_id.as_deref() {
        Some(id) => ConnectionSelection::ByPatientId(
            PatientId::new(id).context("invalid patient_id in [source.llu]")?,
        ),
        None => ConnectionSelection::First,
    };
    // Validator already accepted this string as a real IANA zone; the call
    // below is a defensive double-check rather than a real fallible step.
    // Default `UTC` keeps behaviour stable for deployments that never set it.
    let source_tz: Tz = resolve_source_tz(llu)?;
    let id = SourceId::new("llu").context("build SourceId")?;
    info!(
        region = ?creds.region,
        llu_version = %resolved_version,
        source_tz = %source_tz,
        "llu source configured"
    );
    Ok(Arc::new(LluSource::new(
        id, client, creds, selection, source_tz,
    )))
}

/// One-shot LLU probe. Logs in, lists connections, fetches one graph,
/// prints a single-line JSON summary to stdout. Errors propagate with
/// their `[LLU0xx]` / `[CFG0xx]` prefix so `classify_by_prefix` can
/// map them to operator-facing exit codes.
#[cfg(feature = "source-llu")]
async fn dryrun(cfg: &config::Config) -> Result<()> {
    use gluco_hub_core::PatientId;

    let llu = cfg.source.llu.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "[CFG005] dryrun requires [source.llu] config or \
            GLUCO_HUB__SOURCE__LLU__* env overrides"
        )
    })?;
    let (client, creds, resolved_version) = build_llu_client_and_creds(llu)?;
    let region = creds.region;

    info!(region = ?region, llu_version = %resolved_version, "llu dryrun: logging in");
    let tokens = client.login(&creds).await?;
    info!("llu dryrun: login ok");

    let connections = client.connections(&tokens, region).await?;
    if connections.is_empty() {
        anyhow::bail!("[LLU009] no connections returned for this account");
    }

    let selected = match llu.patient_id.as_deref() {
        Some(pid) => connections
            .iter()
            .find(|c| c.patient_id == pid)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "[LLU009] configured patient_id {pid} not present in connections list"
                )
            })?,
        None => &connections[0],
    };
    let pid = PatientId::new(selected.patient_id.clone()).context("invalid patient_id from LLU")?;
    let graph = client.graph(&tokens, region, &pid).await?;

    let source_tz = resolve_source_tz(llu)?;
    let latest = sources::llu::mapping::newest_measurement(&graph.data.graph_data, source_tz).map(
        |(t, m)| {
            serde_json::json!({
                "mgdl": m.value_in_mg_per_dl,
                "trend_arrow": m.trend_arrow,
                "timestamp_iso": t.to_rfc3339(),
            })
        },
    );

    let summary = serde_json::json!({
        "llu_version": resolved_version,
        "region": format!("{region:?}"),
        "patients": connections
            .iter()
            .map(|c| c.patient_id.as_str())
            .collect::<Vec<_>>(),
        "selected_patient_id": selected.patient_id,
        "graph_count": graph.data.graph_data.len(),
        "latest": latest,
    });
    println!("{}", serde_json::to_string(&summary).expect("json"));
    Ok(())
}

/// One-shot NS read-only probe. Runs `GET /api/v3/entries?count=1`
/// against the configured NS instance, prints a JSON summary on
/// stdout. Never writes — by design.
#[cfg(feature = "sink-nightscout")]
async fn nsdryrun(cfg: &config::Config) -> Result<()> {
    use sinks::nightscout::NightscoutClient;

    let ns = cfg.sink.nightscout.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "[CFG005] ns-dryrun requires [sink.nightscout] config or \
            GLUCO_HUB__SINK__NIGHTSCOUT__* env overrides"
        )
    })?;
    let client = NightscoutClient::new(ns.base_url.clone(), ns.api_secret.clone())
        .context("build nightscout client")?;

    info!(base_url = %ns.base_url, "ns dryrun: probing /api/v3/entries");
    let last_ms = client.fetch_last_entry_date().await?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let age_secs = last_ms.map(|d| (now_ms - d) / 1_000);

    let summary = serde_json::json!({
        "base_url": ns.base_url,
        "last_entry_date_ms": last_ms,
        "last_entry_age_secs": age_secs,
    });
    println!("{}", serde_json::to_string(&summary).expect("json"));
    Ok(())
}

async fn poll_loop(
    source: Arc<dyn Source>,
    cache: ReadingCache,
    sinks: Vec<Arc<sink_router::SinkRouter>>,
    interval: Duration,
) {
    let source_id = source.id().as_str().to_string();
    let sink_timeout = sink_push_timeout(interval);
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let fetch_result = tokio::time::timeout(sink_timeout, source.fetch_latest()).await;
        match fetch_result {
            Err(_elapsed) => {
                ::metrics::counter!(
                    metrics::COUNTER_FETCH_ERRORS,
                    "error_code" => "TIMEOUT",
                )
                .increment(1);
                warn!(
                    source_id = %source_id,
                    timeout_secs = sink_timeout.as_secs(),
                    "source fetch timed out",
                );
                continue;
            }
            Ok(Err(e)) => {
                ::metrics::counter!(
                    metrics::COUNTER_FETCH_ERRORS,
                    "error_code" => e.error_code(),
                )
                .increment(1);
                error!(
                    error_code = e.error_code(),
                    source_id = %source_id,
                    error = %e,
                    "source fetch failed",
                );
                continue;
            }
            Ok(Ok(batch)) => {
                let count = batch.len();
                let newest = batch.iter().max_by_key(|r| r.timestamp).cloned();
                cache.update(&batch);
                ::metrics::counter!(
                    metrics::COUNTER_FETCH_SUCCESS,
                    "source_id" => source_id.clone(),
                )
                .increment(1);
                ::metrics::counter!(metrics::COUNTER_CACHE_UPDATES).increment(1);
                let newest_log = newest.as_ref().map(|r| {
                    (
                        r.glucose.get(),
                        r.timestamp.to_rfc3339(),
                        (chrono::Utc::now() - r.timestamp).num_seconds(),
                    )
                });
                if let Some(latest) = newest {
                    ::metrics::gauge!(
                        metrics::GAUGE_GLUCOSE,
                        "patient_id" => latest.patient_id.as_str().to_string(),
                        "source_id" => latest.source_id.as_str().to_string(),
                    )
                    .set(latest.glucose.get());
                }
                match newest_log {
                    Some((mgdl, ts, age_secs)) => info!(
                        source_id = %source_id,
                        count,
                        newest_mgdl = mgdl,
                        newest_ts = %ts,
                        newest_age_secs = age_secs,
                        "cache updated",
                    ),
                    None => info!(source_id = %source_id, count, "cache updated (empty batch)"),
                }

                if !sinks.is_empty() {
                    fan_out_to_sinks(&sinks, &batch, sink_timeout).await;
                }
            }
        }
    }
}

/// Per-sink push timeout: never longer than the poll interval, capped
/// at 20 s so a stuck sink can't starve the next tick.
fn sink_push_timeout(interval: Duration) -> Duration {
    interval.min(Duration::from_secs(20))
}

/// Push `batch` to every sink concurrently, with a per-sink timeout.
/// Routing is delegated to `SinkRouter::push_filtered` so each sink only
/// sees readings strictly newer than its watermark — see
/// `sink_router::SinkRouter` for the backfill / recovery semantics.
/// Errors are logged + counted; a failing sink never propagates and never
/// blocks the next poll tick.
async fn fan_out_to_sinks(
    sinks: &[Arc<sink_router::SinkRouter>],
    batch: &[Reading],
    timeout: Duration,
) {
    use futures::future::join_all;

    let pushes = sinks.iter().map(|router| {
        let router = Arc::clone(router);
        let batch_owned: Vec<Reading> = batch.to_vec();
        async move {
            let name = router.name();
            match tokio::time::timeout(timeout, router.push_filtered(&batch_owned)).await {
                Ok((outcome, Ok(()))) => {
                    if outcome.filtered > 0 {
                        ::metrics::counter!(
                            metrics::COUNTER_SINK_FILTERED,
                            "sink" => name,
                        )
                        .increment(outcome.filtered as u64);
                    }
                    if outcome.replayed > 0 {
                        ::metrics::counter!(
                            metrics::COUNTER_SINK_REPLAYED,
                            "sink" => name,
                        )
                        .increment(outcome.replayed as u64);
                    }
                    if outcome.pushed > 0 {
                        ::metrics::counter!(
                            metrics::COUNTER_SINK_SUCCESS,
                            "sink" => name,
                        )
                        .increment(1);
                    }
                }
                Ok((outcome, Err(e))) => {
                    if outcome.filtered > 0 {
                        ::metrics::counter!(
                            metrics::COUNTER_SINK_FILTERED,
                            "sink" => name,
                        )
                        .increment(outcome.filtered as u64);
                    }
                    let code = extract_error_code(&format!("{e}"));
                    ::metrics::counter!(
                        metrics::COUNTER_SINK_ERRORS,
                        "sink" => name,
                        "error_code" => code.clone(),
                    )
                    .increment(1);
                    warn!(
                        sink = name,
                        error_code = %code,
                        error = %e,
                        "sink push failed",
                    );
                }
                Err(_elapsed) => {
                    ::metrics::counter!(
                        metrics::COUNTER_SINK_ERRORS,
                        "sink" => name,
                        "error_code" => "TIMEOUT",
                    )
                    .increment(1);
                    warn!(
                        sink = name,
                        timeout_secs = timeout.as_secs(),
                        "sink push timed out"
                    );
                }
            }
        }
    });
    join_all(pushes).await;
}

/// Extract a `[CODE]` prefix from an error's `Display` representation.
/// Returns `"UNKNOWN"` when the message is not in our `[XXNNN] ...` shape
/// — that's defensive: a future variant without a code shouldn't crash.
fn extract_error_code(message: &str) -> String {
    if let Some(rest) = message.strip_prefix('[')
        && let Some(end) = rest.find(']')
    {
        return rest[..end].to_string();
    }
    "UNKNOWN".to_string()
}

/// Build the configured sinks, each wrapped in a `SinkRouter` for
/// per-sink watermark filtering (V3 — Backfill). Order is config-driven;
/// future sinks (webhook, …) slot in as additional entries.
fn build_sinks(cfg: &config::Config) -> Result<Vec<Arc<sink_router::SinkRouter>>> {
    let _ = cfg;
    #[cfg_attr(
        not(any(feature = "sink-nightscout", feature = "sink-mqtt")),
        allow(unused_mut)
    )]
    let mut sinks: Vec<Arc<dyn Sink>> = Vec::new();

    #[cfg(feature = "sink-nightscout")]
    if let Some(ns) = cfg.sink.nightscout.as_ref() {
        sinks.push(build_nightscout_sink(ns).context("build Nightscout sink")?);
    }

    #[cfg(feature = "sink-mqtt")]
    if let Some(mqtt) = cfg.sink.mqtt.as_ref() {
        sinks.push(build_mqtt_sink(mqtt).context("build MQTT sink")?);
    }

    Ok(sinks
        .into_iter()
        .map(|s| Arc::new(sink_router::SinkRouter::new(s)))
        .collect())
}

#[cfg(feature = "sink-mqtt")]
fn build_mqtt_sink(cfg: &config::MqttSinkConfig) -> Result<Arc<dyn Sink>> {
    use sinks::mqtt::MqttSink;

    let password = cfg.password.clone();

    info!(
        broker = %cfg.broker_host,
        port = cfg.broker_port,
        client_id = %cfg.client_id,
        tls = cfg.tls,
        topic_prefix = %cfg.topic_prefix,
        "mqtt sink configured"
    );

    let sink = MqttSink::new(cfg, password).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(Arc::new(sink))
}

#[cfg(feature = "sink-nightscout")]
fn build_nightscout_sink(ns: &config::NightscoutSinkConfig) -> Result<Arc<dyn Sink>> {
    use sinks::nightscout::{NightscoutClient, NightscoutSink};

    const DEFAULT_DEVICE: &str = "gluco-hub";
    const DEFAULT_APP: &str = "gluco-hub";

    let device = ns.device.as_deref().unwrap_or(DEFAULT_DEVICE);
    let app = ns.app.as_deref().unwrap_or(DEFAULT_APP);
    let client = NightscoutClient::new(ns.base_url.clone(), ns.api_secret.clone())
        .context("build Nightscout client")?
        .with_device(device)
        .with_app(app);
    info!(base_url = %ns.base_url, device, app, "nightscout sink configured");
    Ok(Arc::new(NightscoutSink::new(client)))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    info!("shutdown signal received");
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use gluco_hub_core::{CoreError, GlucoseMgDl, PatientId, Reading, SourceId, Trend};
    use std::sync::Mutex as StdMutex;

    /// Test sink that records every push and can be set to fail.
    struct RecorderSink {
        name: &'static str,
        calls: Arc<StdMutex<usize>>,
        fail_with: Option<&'static str>,
        delay: Option<Duration>,
    }

    #[async_trait::async_trait]
    impl Sink for RecorderSink {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn push(&self, _: &[Reading]) -> Result<(), CoreError> {
            if let Some(d) = self.delay {
                tokio::time::sleep(d).await;
            }
            *self.calls.lock().expect("calls mutex") += 1;
            if let Some(code) = self.fail_with {
                return Err(CoreError::Sink {
                    message: format!("[{}] simulated failure", code),
                });
            }
            Ok(())
        }
    }

    fn one_reading() -> Reading {
        Reading {
            patient_id: PatientId::new("p1").unwrap(),
            source_id: SourceId::new("llu").unwrap(),
            timestamp: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            glucose: GlucoseMgDl::new(120.0).unwrap(),
            trend: Trend::Flat,
        }
    }

    #[test]
    fn extract_error_code_handles_known_and_unknown_shapes() {
        assert_eq!(extract_error_code("[NS004] retry me"), "NS004");
        assert_eq!(extract_error_code("CORE004 something"), "UNKNOWN");
        assert_eq!(extract_error_code("[unterminated"), "UNKNOWN");
        assert_eq!(extract_error_code(""), "UNKNOWN");
    }

    fn wrap_router(sink: Arc<dyn Sink>) -> Arc<sink_router::SinkRouter> {
        Arc::new(sink_router::SinkRouter::new(sink))
    }

    #[tokio::test]
    async fn fan_out_calls_every_sink_on_success() {
        let calls_a = Arc::new(StdMutex::new(0));
        let calls_b = Arc::new(StdMutex::new(0));
        let sinks: Vec<Arc<sink_router::SinkRouter>> = vec![
            wrap_router(Arc::new(RecorderSink {
                name: "a",
                calls: calls_a.clone(),
                fail_with: None,
                delay: None,
            })),
            wrap_router(Arc::new(RecorderSink {
                name: "b",
                calls: calls_b.clone(),
                fail_with: None,
                delay: None,
            })),
        ];
        fan_out_to_sinks(&sinks, &[one_reading()], Duration::from_secs(5)).await;
        assert_eq!(*calls_a.lock().unwrap(), 1);
        assert_eq!(*calls_b.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn fan_out_isolates_sink_failures() {
        let calls_ok = Arc::new(StdMutex::new(0));
        let calls_bad = Arc::new(StdMutex::new(0));
        let sinks: Vec<Arc<sink_router::SinkRouter>> = vec![
            wrap_router(Arc::new(RecorderSink {
                name: "good",
                calls: calls_ok.clone(),
                fail_with: None,
                delay: None,
            })),
            wrap_router(Arc::new(RecorderSink {
                name: "bad",
                calls: calls_bad.clone(),
                fail_with: Some("NS004"),
                delay: None,
            })),
        ];
        // The function returns `()`; failures are absorbed.
        fan_out_to_sinks(&sinks, &[one_reading()], Duration::from_secs(5)).await;
        assert_eq!(*calls_ok.lock().unwrap(), 1);
        assert_eq!(*calls_bad.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn fan_out_times_out_slow_sink_without_blocking_others() {
        let calls_fast = Arc::new(StdMutex::new(0));
        let calls_slow = Arc::new(StdMutex::new(0));
        let sinks: Vec<Arc<sink_router::SinkRouter>> = vec![
            wrap_router(Arc::new(RecorderSink {
                name: "fast",
                calls: calls_fast.clone(),
                fail_with: None,
                delay: None,
            })),
            wrap_router(Arc::new(RecorderSink {
                name: "slow",
                calls: calls_slow.clone(),
                fail_with: None,
                delay: Some(Duration::from_secs(5)),
            })),
        ];
        let started = std::time::Instant::now();
        fan_out_to_sinks(&sinks, &[one_reading()], Duration::from_millis(50)).await;
        // Total wall time bounded by the timeout, not by the slow sink.
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(*calls_fast.lock().unwrap(), 1);
        // The slow sink never finished its body before the timeout fired,
        // so its `calls` counter was not incremented.
        assert_eq!(*calls_slow.lock().unwrap(), 0);
    }

    #[test]
    fn sink_push_timeout_caps_at_20_secs() {
        assert_eq!(
            sink_push_timeout(Duration::from_secs(60)),
            Duration::from_secs(20)
        );
        assert_eq!(
            sink_push_timeout(Duration::from_secs(5)),
            Duration::from_secs(5)
        );
    }

    /// `scripts/llu-dryrun.sh` documents these exit codes — keep them
    /// pinned by test so a future refactor doesn't silently shift the
    /// operator-visible contract.
    #[test]
    fn dryrun_exit_classification() {
        // The classification operates on `format!("{e:#}")` style
        // anyhow chains, so embed the prefix in any larger message.
        let mk = |s: &str| classify_by_prefix(&format!("oops: {s} extra"), DRYRUN_EXIT_TABLE);
        // anyhow's ExitCode does not impl PartialEq; stringify for compare.
        let to_n = |c: ExitCode| format!("{c:?}");

        assert_eq!(to_n(mk("[LLU003]")), to_n(ExitCode::from(3)));
        assert_eq!(to_n(mk("[LLU002]")), to_n(ExitCode::from(4)));
        assert_eq!(to_n(mk("[LLU004]")), to_n(ExitCode::from(4)));
        assert_eq!(to_n(mk("[LLU001]")), to_n(ExitCode::from(5)));
        assert_eq!(to_n(mk("[CFG003]")), to_n(ExitCode::from(2)));
        assert_eq!(to_n(mk("[CFG004]")), to_n(ExitCode::from(2)));
        // Unknown / unclassified → generic failure.
        assert_eq!(to_n(mk("totally unrelated panic")), to_n(ExitCode::FAILURE));
    }

    /// `scripts/ns-dryrun.sh` exit-code contract. Pinned mirror of
    /// `dryrun_exit_classification` for the sink side.
    #[test]
    fn ns_dryrun_exit_classification() {
        let mk = |s: &str| classify_by_prefix(&format!("oops: {s} extra"), NSDRYRUN_EXIT_TABLE);
        let to_n = |c: ExitCode| format!("{c:?}");

        assert_eq!(to_n(mk("[CFG003]")), to_n(ExitCode::from(2)));
        assert_eq!(to_n(mk("[NS005]")), to_n(ExitCode::from(2)));
        assert_eq!(to_n(mk("[NS001]")), to_n(ExitCode::from(3)));
        assert_eq!(to_n(mk("[NS002]")), to_n(ExitCode::from(4)));
        assert_eq!(to_n(mk("[NS003]")), to_n(ExitCode::from(5)));
        assert_eq!(to_n(mk("[NS004]")), to_n(ExitCode::from(5)));
        assert_eq!(to_n(mk("totally unrelated panic")), to_n(ExitCode::FAILURE));
    }
}
