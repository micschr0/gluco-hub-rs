use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use ::config::{Config as ConfigBuilder, Environment, File, FileFormat};
use serde::Deserialize;
use thiserror::Error;
use validator::{Validate, ValidationError};

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

    #[serde(default)]
    #[validate(nested)]
    pub source: SourceConfig,
}

#[derive(Debug, Clone, Deserialize, Validate)]
pub struct HttpConfig {
    pub bind: SocketAddr,

    /// Optional: name of the env var holding a Bearer token. When set,
    /// `/glucose/*` requires `Authorization: Bearer <token>`. `/healthz`
    /// and `/metrics` always stay public.
    #[serde(default)]
    #[validate(
        length(min = 1, max = 256),
        custom(function = "validate_ascii_env_name")
    )]
    pub bearer_token_env: Option<String>,
}

fn validate_ascii_env_name(value: &str) -> Result<(), ValidationError> {
    if value.is_empty() || !value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(ValidationError::new("env_var_name"));
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Validate)]
pub struct PollerConfig {
    /// Polling interval in seconds. LibreLink Up updates every ~60 s, so
    /// values below 30 are wasteful and may trip rate limits.
    #[validate(range(min = 30, max = 600))]
    pub interval_secs: u64,
}

/// Source-specific configuration. Each variant lives behind a Cargo
/// feature on the binary; deserialisation always parses every block but
/// `build_default_source` only honours the ones whose feature is enabled.
#[derive(Debug, Clone, Default, Deserialize, Validate)]
pub struct SourceConfig {
    #[serde(default)]
    #[validate(nested)]
    pub llu: Option<LluSourceConfig>,
}

/// `[source.llu]` block. The password lives in an environment variable
/// referenced by `password_env`; the TOML never holds the secret itself.
#[derive(Debug, Clone, Deserialize, Validate)]
pub struct LluSourceConfig {
    #[validate(email)]
    pub email: String,

    /// Name of the environment variable holding the LLU password.
    #[validate(length(min = 1, max = 256))]
    pub password_env: String,

    /// LibreLink Up region (`EU`, `US`, `DE`, …). Validated against the
    /// canonical region table — unknown values fail at config load time.
    #[validate(custom(function = "validate_region"))]
    pub region: String,

    /// Optional patient identifier when the LLU account has more than one
    /// linked patient. When absent, the first connection is selected.
    #[validate(length(min = 1, max = 128))]
    pub patient_id: Option<String>,
}

fn validate_region(value: &str) -> Result<(), ValidationError> {
    #[cfg(feature = "source-llu")]
    {
        crate::sources::llu::Region::parse(value)
            .map(|_| ())
            .map_err(|_| ValidationError::new("unknown_llu_region"))
    }
    #[cfg(not(feature = "source-llu"))]
    {
        // Without the feature the field is descriptive-only; accept any
        // 2..=4-letter ASCII string so users can prepare a config ahead
        // of enabling the feature.
        let len = value.len();
        if !(2..=4).contains(&len) || !value.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(ValidationError::new("region_format"));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("[CFG001] failed to read config: {0}")]
    Read(#[from] ::config::ConfigError),

    #[error("[CFG002] config validation failed: {0}")]
    Validate(#[from] validator::ValidationErrors),

    #[error("[CFG003] required secret env var not set: {var}")]
    MissingSecret { var: String },
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

/// Verify that every `*_env` reference resolves to a non-empty environment
/// variable. Called by `check-config` so misconfiguration fails fast — no
/// network round-trips required to learn that `LLU_PASSWORD` was forgotten.
///
/// The env-var *value* is never logged, returned, or stored; only its
/// presence is checked.
pub fn verify_secret_env_vars(cfg: &Config) -> Result<(), ConfigError> {
    let mut required: Vec<&str> = Vec::new();
    if let Some(llu) = cfg.source.llu.as_ref() {
        required.push(&llu.password_env);
    }
    if let Some(name) = cfg.http.bearer_token_env.as_deref() {
        required.push(name);
    }
    for var in required {
        let value = std::env::var(var).map_err(|_| ConfigError::MissingSecret {
            var: var.to_string(),
        })?;
        if value.is_empty() {
            return Err(ConfigError::MissingSecret {
                var: var.to_string(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_load_without_file() {
        let cfg = load(Some(Path::new("/nonexistent.toml"))).expect("defaults must load");
        assert_eq!(cfg.http.bind.to_string(), "127.0.0.1:8080");
        assert_eq!(cfg.poller.interval_secs, 60);
        assert!(cfg.source.llu.is_none());
    }

    #[test]
    fn rejects_too_fast_polling() {
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

    fn write_with_llu(dir: &std::path::Path, region: &str, patient_id: Option<&str>) -> PathBuf {
        let path = dir.join("config.toml");
        let extra = patient_id
            .map(|id| format!("\npatient_id = \"{id}\""))
            .unwrap_or_default();
        std::fs::write(
            &path,
            format!(
                r#"
[http]
bind = "127.0.0.1:9000"

[poller]
interval_secs = 60

[source.llu]
email = "patient@example.com"
password_env = "TEST_LLU_PASSWORD"
region = "{region}"{extra}
"#
            ),
        )
        .unwrap();
        path
    }

    #[test]
    fn parses_llu_section_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_with_llu(dir.path(), "EU", Some("patient-1"));
        let cfg = load(Some(&path)).expect("load");
        let llu = cfg.source.llu.expect("llu present");
        assert_eq!(llu.email, "patient@example.com");
        assert_eq!(llu.region, "EU");
        assert_eq!(llu.patient_id.as_deref(), Some("patient-1"));
    }

    #[test]
    fn rejects_unknown_region() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_with_llu(dir.path(), "MARS", None);
        let err = load(Some(&path)).expect_err("must reject MARS");
        assert!(matches!(err, ConfigError::Validate(_)));
    }

    #[test]
    fn rejects_invalid_email() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[http]
bind = "127.0.0.1:9000"

[poller]
interval_secs = 60

[source.llu]
email = "not-an-email"
password_env = "X"
region = "EU"
"#,
        )
        .unwrap();
        let err = load(Some(&path)).expect_err("must reject");
        assert!(matches!(err, ConfigError::Validate(_)));
    }

    #[test]
    fn verify_secret_env_vars_detects_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_with_llu(dir.path(), "EU", None);
        let cfg = load(Some(&path)).expect("load");
        // Use a guaranteed-unset variable; do not touch process env.
        let mut cfg = cfg;
        cfg.source.llu.as_mut().unwrap().password_env =
            "CGM_BRIDGE_TEST_DEFINITELY_UNSET_VAR".to_string();
        let err = verify_secret_env_vars(&cfg).expect_err("missing");
        match err {
            ConfigError::MissingSecret { var } => {
                assert_eq!(var, "CGM_BRIDGE_TEST_DEFINITELY_UNSET_VAR")
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
