use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use ::config::{Config as ConfigBuilder, Environment, File, FileFormat};
use serde::Deserialize;
use thiserror::Error;
use validator::Validate;

/// Top-level application configuration.
///
/// Loaded from a TOML file (default `./config.toml`) and overridden by
/// `CGM_BRIDGE_*` environment variables. Secrets are never embedded — TOML
/// references env-var names (e.g. `password_env = "LLU_PASSWORD"`).
#[derive(Debug, Clone, Deserialize, Validate)]
pub struct Config {
    #[validate(nested)]
    pub http: HttpConfig,

    #[validate(nested)]
    pub poller: PollerConfig,
}

#[derive(Debug, Clone, Deserialize, Validate)]
pub struct HttpConfig {
    pub bind: SocketAddr,
}

#[derive(Debug, Clone, Deserialize, Validate)]
pub struct PollerConfig {
    /// Polling interval in seconds. LibreLink Up updates every ~60 s, so
    /// values below 30 are wasteful and may trip rate limits.
    #[validate(range(min = 30, max = 600))]
    pub interval_secs: u64,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("[CFG001] failed to read config: {0}")]
    Read(#[from] ::config::ConfigError),

    #[error("[CFG002] config validation failed: {0}")]
    Validate(#[from] validator::ValidationErrors),
}

const DEFAULT_PATH: &str = "config.toml";
const ENV_PREFIX: &str = "CGM_BRIDGE";

/// Load and validate configuration. If `override_path` is `None`, the
/// loader first tries `./config.toml` and otherwise falls back to a set of
/// built-in defaults suitable for local development.
pub fn load(override_path: Option<&Path>) -> Result<Config, ConfigError> {
    let mut builder = ConfigBuilder::builder()
        .set_default("http.bind", "127.0.0.1:8080")?
        .set_default("poller.interval_secs", 60_i64)?;

    let path = override_path
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PATH));
    if path.exists() {
        builder = builder.add_source(File::from(path).format(FileFormat::Toml));
    }

    builder = builder.add_source(
        Environment::with_prefix(ENV_PREFIX)
            .separator("__")
            .try_parsing(true),
    );

    let cfg: Config = builder.build()?.try_deserialize()?;
    cfg.validate()?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_load_without_file() {
        let cfg = load(Some(Path::new("/nonexistent.toml"))).expect("defaults must load");
        assert_eq!(cfg.http.bind.to_string(), "127.0.0.1:8080");
        assert_eq!(cfg.poller.interval_secs, 60);
    }

    #[test]
    fn rejects_too_fast_polling() {
        // Construct via TOML to exercise validator.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[http]
bind = "127.0.0.1:9000"

[poller]
interval_secs = 5
"#,
        )
        .unwrap();
        let err = load(Some(&path)).expect_err("must reject");
        assert!(matches!(err, ConfigError::Validate(_)));
    }
}
