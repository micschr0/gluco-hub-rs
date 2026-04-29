use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing::{error, info};

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
    let router = api::router();
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
