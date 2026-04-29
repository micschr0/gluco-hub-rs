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
            info!(http_addr = %cfg.http.bind, "configuration ok");
            Ok(())
        }
        Command::Run => serve(cfg).await,
    }
}

async fn serve(cfg: config::Config) -> Result<()> {
    let cache = ReadingCache::new();
    let state = api::AppState {
        cache: cache.clone(),
    };

    if let Some(source) = build_default_source()? {
        let interval = Duration::from_secs(cfg.poller.interval_secs);
        let cache_for_poller = cache.clone();
        tokio::spawn(async move {
            poll_loop(source, cache_for_poller, interval).await;
        });
    } else {
        warn!("no source compiled in; HTTP API will serve 503 until one is enabled");
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

/// Build the default source for this binary feature set. V1 ships only
/// LibreLink Up; until that lands, the `mock-source` feature wires in a
/// canned source so the HTTP API can be exercised end-to-end.
fn build_default_source() -> Result<Option<Arc<dyn Source>>> {
    #[cfg(feature = "mock-source")]
    {
        let mock =
            cgm_bridge_core::MockSource::default_fixture().context("build MockSource fixture")?;
        Ok(Some(Arc::new(mock)))
    }
    #[cfg(not(feature = "mock-source"))]
    {
        Ok(None)
    }
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
                cache.update(&batch);
                info!(source_id = %source_id, count, "cache updated");
            }
            Err(e) => {
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
