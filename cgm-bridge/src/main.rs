use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use cgm_bridge_core::{ReadingCache, Source};
use clap::{Parser, Subcommand};
use tracing::{error, info, warn};

mod api;
mod config;
mod metrics;
mod sources;

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

    if let Some(source) = build_default_source(&cfg)? {
        let interval = Duration::from_secs(cfg.poller.interval_secs);
        let cache_for_poller = cache.clone();
        tokio::spawn(async move {
            poll_loop(source, cache_for_poller, interval).await;
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
    let client = LluAuthClient::new().context("build LLU HTTP client")?;
    let selection = match llu.patient_id.as_deref() {
        Some(id) => ConnectionSelection::ByPatientId(
            PatientId::new(id).context("invalid patient_id in [source.llu]")?,
        ),
        None => ConnectionSelection::First,
    };
    let id = SourceId::new("llu").context("build SourceId")?;
    info!(region = ?region, "llu source configured");
    Ok(Arc::new(LluSource::new(id, client, creds, selection)))
}

async fn poll_loop(source: Arc<dyn Source>, cache: ReadingCache, interval: Duration) {
    let source_id = source.id().as_str().to_string();
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
