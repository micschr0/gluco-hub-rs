use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use cgm_bridge_core::{Reading, ReadingCache, Sink, Source};
use clap::{Parser, Subcommand};
use tracing::{error, info, warn};

mod api;
mod config;
mod metrics;
mod sinks;
mod sources;

#[cfg(all(test, feature = "source-llu", feature = "sink-nightscout"))]
mod e2e_tests;

#[derive(Debug, Parser)]
#[command(name = "cgm-bridge", about = "LibreLink Up → HTTP/Nightscout bridge")]
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
}

fn main() -> ExitCode {
    init_tracing();

    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!(error = %e, "fatal");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let builder = tracing_subscriber::fmt().with_env_filter(filter);

    if std::env::var_os("CGM_BRIDGE_LOG_PRETTY").is_some() {
        builder.init();
    } else {
        builder.json().init();
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn run(cli: Cli) -> Result<()> {
    let cfg = config::load(cli.config.as_deref()).context("failed to load configuration")?;

    match cli.command {
        Command::CheckConfig => {
            config::verify_secret_env_vars(&cfg).context("verify secret env vars")?;
            info!(
                http_addr = %cfg.http.bind,
                llu_configured = cfg.source.llu.is_some(),
                "configuration ok"
            );
            Ok(())
        }
        Command::Run => serve(cfg).await,
    }
}

async fn serve(cfg: config::Config) -> Result<()> {
    // Fail fast if any referenced secret env var is missing — better to
    // crash on startup than to start serving 401s after the bearer token
    // ended up empty.
    config::verify_secret_env_vars(&cfg).context("verify secret env vars")?;

    let metrics_handle = metrics::init_recorder().context("init metrics recorder")?;
    let cache = ReadingCache::new();
    let bearer_token = resolve_bearer_token(&cfg)?;
    info!(auth_enabled = bearer_token.is_some(), "http auth state");
    let state = api::AppState {
        cache: cache.clone(),
        metrics_handle,
        bearer_token,
    };

    let sinks = build_sinks(&cfg)?;
    info!(sink_count = sinks.len(), "sinks configured");

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

/// Resolve the optional Bearer token from the env var named in
/// `[http] bearer_token_env`. Returns `Ok(None)` when the operator has
/// not opted in to auth. Errors when the env var is referenced but
/// unset/empty (already pre-checked by `verify_secret_env_vars`, but
/// kept defensive in case of a race where the var is unset between
/// startup and here).
fn resolve_bearer_token(cfg: &config::Config) -> Result<Option<secrecy::SecretString>> {
    let Some(name) = cfg.http.bearer_token_env.as_deref() else {
        return Ok(None);
    };
    let value = std::env::var(name)
        .map_err(|_| anyhow::anyhow!("[CFG003] bearer_token_env not set: {name}"))?;
    if value.is_empty() {
        anyhow::bail!("[CFG003] bearer_token_env is empty: {name}");
    }
    Ok(Some(secrecy::SecretString::from(value)))
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
            cgm_bridge_core::MockSource::default_fixture().context("build MockSource fixture")?;
        source = Some(Arc::new(mock));
    }

    Ok(source)
}

#[cfg(feature = "source-llu")]
fn build_llu_source(llu: &config::LluSourceConfig) -> Result<Arc<dyn Source>> {
    use cgm_bridge_core::{PatientId, SourceId};
    use secrecy::SecretString;
    use sources::llu::Region;
    use sources::llu::auth::{LluAuthClient, LluCredentials};
    use sources::llu::headers::DEFAULT_LLU_VERSION;
    use sources::llu::source::{ConnectionSelection, LluSource};

    let region = Region::parse(&llu.region).context("parse LLU region")?;
    let password = std::env::var(&llu.password_env).map_err(|_| {
        anyhow::anyhow!(
            "[CFG003] required secret env var not set: {}",
            llu.password_env
        )
    })?;
    if password.is_empty() {
        anyhow::bail!(
            "[CFG003] required secret env var is empty: {}",
            llu.password_env
        );
    }

    let creds = LluCredentials {
        email: llu.email.clone(),
        password: SecretString::from(password),
        region,
    };
    let resolved_version = llu
        .version
        .as_deref()
        .unwrap_or(DEFAULT_LLU_VERSION)
        .to_string();
    let client = LluAuthClient::new()
        .context("build LLU HTTP client")?
        .with_version(&resolved_version);
    let selection = match llu.patient_id.as_deref() {
        Some(id) => ConnectionSelection::ByPatientId(
            PatientId::new(id).context("invalid patient_id in [source.llu]")?,
        ),
        None => ConnectionSelection::First,
    };
    let id = SourceId::new("llu").context("build SourceId")?;
    // Logged at INFO so post-mortems can confirm exactly which `version`
    // header the bridge sent — recovers from a 4xx without spelunking
    // through the running config.
    info!(
        region = ?region,
        llu_version = %resolved_version,
        "llu source configured"
    );
    Ok(Arc::new(LluSource::new(id, client, creds, selection)))
}

async fn poll_loop(
    source: Arc<dyn Source>,
    cache: ReadingCache,
    sinks: Vec<Arc<dyn Sink>>,
    interval: Duration,
) {
    let source_id = source.id().as_str().to_string();
    let sink_timeout = sink_push_timeout(interval);
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        match source.fetch_latest().await {
            Ok(batch) => {
                let count = batch.len();
                let newest = batch.iter().max_by_key(|r| r.timestamp).cloned();
                cache.update(&batch);
                ::metrics::counter!(
                    metrics::COUNTER_FETCH_SUCCESS,
                    "source_id" => source_id.clone(),
                )
                .increment(1);
                ::metrics::counter!(metrics::COUNTER_CACHE_UPDATES).increment(1);
                if let Some(latest) = newest {
                    ::metrics::gauge!(
                        metrics::GAUGE_GLUCOSE,
                        "patient_id" => latest.patient_id.as_str().to_string(),
                        "source_id" => latest.source_id.as_str().to_string(),
                    )
                    .set(latest.glucose.get());
                }
                info!(source_id = %source_id, count, "cache updated");

                if !sinks.is_empty() {
                    fan_out_to_sinks(&sinks, &batch, sink_timeout).await;
                }
            }
            Err(e) => {
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
/// Errors are logged + counted; a failing sink never propagates and never
/// blocks the next poll tick.
async fn fan_out_to_sinks(sinks: &[Arc<dyn Sink>], batch: &[Reading], timeout: Duration) {
    use futures::future::join_all;

    let pushes = sinks.iter().map(|sink| {
        let sink = Arc::clone(sink);
        let batch_owned: Vec<Reading> = batch.to_vec();
        async move {
            let name = sink.name();
            match tokio::time::timeout(timeout, sink.push(&batch_owned)).await {
                Ok(Ok(())) => {
                    ::metrics::counter!(
                        metrics::COUNTER_SINK_SUCCESS,
                        "sink" => name,
                    )
                    .increment(1);
                }
                Ok(Err(e)) => {
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

/// Build the configured sinks. Order is config-driven; future sinks
/// (MQTT, webhook) slot in as additional entries.
fn build_sinks(cfg: &config::Config) -> Result<Vec<Arc<dyn Sink>>> {
    let _ = cfg;
    #[cfg_attr(not(feature = "sink-nightscout"), allow(unused_mut))]
    let mut sinks: Vec<Arc<dyn Sink>> = Vec::new();

    #[cfg(feature = "sink-nightscout")]
    if let Some(ns) = cfg.sink.nightscout.as_ref() {
        sinks.push(build_nightscout_sink(ns).context("build Nightscout sink")?);
    }

    Ok(sinks)
}

#[cfg(feature = "sink-nightscout")]
fn build_nightscout_sink(ns: &config::NightscoutSinkConfig) -> Result<Arc<dyn Sink>> {
    use secrecy::SecretString;
    use sinks::nightscout::{NightscoutClient, NightscoutSink};

    const DEFAULT_DEVICE: &str = "cgm-bridge";
    const DEFAULT_APP: &str = "cgm-bridge";

    let secret = std::env::var(&ns.api_secret_env).map_err(|_| {
        anyhow::anyhow!(
            "[CFG003] required secret env var not set: {}",
            ns.api_secret_env
        )
    })?;
    if secret.is_empty() {
        anyhow::bail!(
            "[CFG003] required secret env var is empty: {}",
            ns.api_secret_env
        );
    }
    let device = ns.device.as_deref().unwrap_or(DEFAULT_DEVICE);
    let app = ns.app.as_deref().unwrap_or(DEFAULT_APP);
    let client = NightscoutClient::new(ns.base_url.clone(), SecretString::from(secret))
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
    use cgm_bridge_core::{CoreError, GlucoseMgDl, PatientId, Reading, SourceId, Trend};
    use chrono::{TimeZone, Utc};
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

    #[tokio::test]
    async fn fan_out_calls_every_sink_on_success() {
        let calls_a = Arc::new(StdMutex::new(0));
        let calls_b = Arc::new(StdMutex::new(0));
        let sinks: Vec<Arc<dyn Sink>> = vec![
            Arc::new(RecorderSink {
                name: "a",
                calls: calls_a.clone(),
                fail_with: None,
                delay: None,
            }),
            Arc::new(RecorderSink {
                name: "b",
                calls: calls_b.clone(),
                fail_with: None,
                delay: None,
            }),
        ];
        fan_out_to_sinks(&sinks, &[one_reading()], Duration::from_secs(5)).await;
        assert_eq!(*calls_a.lock().unwrap(), 1);
        assert_eq!(*calls_b.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn fan_out_isolates_sink_failures() {
        let calls_ok = Arc::new(StdMutex::new(0));
        let calls_bad = Arc::new(StdMutex::new(0));
        let sinks: Vec<Arc<dyn Sink>> = vec![
            Arc::new(RecorderSink {
                name: "good",
                calls: calls_ok.clone(),
                fail_with: None,
                delay: None,
            }),
            Arc::new(RecorderSink {
                name: "bad",
                calls: calls_bad.clone(),
                fail_with: Some("NS004"),
                delay: None,
            }),
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
        let sinks: Vec<Arc<dyn Sink>> = vec![
            Arc::new(RecorderSink {
                name: "fast",
                calls: calls_fast.clone(),
                fail_with: None,
                delay: None,
            }),
            Arc::new(RecorderSink {
                name: "slow",
                calls: calls_slow.clone(),
                fail_with: None,
                delay: Some(Duration::from_secs(5)),
            }),
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
}
