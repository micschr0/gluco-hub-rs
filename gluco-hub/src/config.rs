// SPDX-License-Identifier: AGPL-3.0-or-later

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use ::config::{Config as ConfigBuilder, Environment, File, FileFormat};
use serde::Deserialize;
use thiserror::Error;
use validator::{Validate, ValidationError};

/// Top-level application configuration.
///
/// Loaded from a TOML file (default `./config.toml`) and overridden by
/// `GLUCO_HUB_*` environment variables. Secrets are never embedded — TOML
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

    #[serde(default)]
    #[validate(nested)]
    pub sink: SinkConfig,
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

/// Sink-specific configuration. Mirrors `SourceConfig`: each variant
/// lives behind a Cargo feature on the binary; the loader parses every
/// block and the binary's wiring honours only those whose feature is on.
#[derive(Debug, Clone, Default, Deserialize, Validate)]
pub struct SinkConfig {
    #[serde(default)]
    #[validate(nested)]
    pub nightscout: Option<NightscoutSinkConfig>,

    /// V2 MQTT sink. Parsed unconditionally; honoured only when the
    /// `sink-mqtt` feature is enabled (see `build_sinks` in `main.rs`).
    #[serde(default)]
    #[validate(nested)]
    pub mqtt: Option<MqttSinkConfig>,
}

/// `[sink.nightscout]` block. The API secret lives in an environment
/// variable referenced by `api_secret_env`; the TOML never holds it.
#[derive(Debug, Clone, Deserialize, Validate)]
pub struct NightscoutSinkConfig {
    #[validate(length(min = 5, max = 512), custom(function = "validate_http_url"))]
    pub base_url: String,

    /// Name of the environment variable holding the Nightscout API
    /// secret (raw, NOT pre-hashed).
    #[validate(
        length(min = 1, max = 256),
        custom(function = "validate_ascii_env_name")
    )]
    pub api_secret_env: String,

    /// Identifies this service in the NS UI's source column. Defaults
    /// to `"gluco-hub"`. Mirrors the reference port's
    /// `NIGHTSCOUT_DEVICE_NAME`.
    #[serde(default)]
    #[validate(length(min = 1, max = 128))]
    pub device: Option<String>,

    /// App name attached to every uploaded entry. Defaults to
    /// `"gluco-hub"`. Mirrors the reference port's `app` config value.
    #[serde(default)]
    #[validate(length(min = 1, max = 128))]
    pub app: Option<String>,
}

fn validate_http_url(value: &str) -> Result<(), ValidationError> {
    if !(value.starts_with("http://") || value.starts_with("https://")) {
        return Err(ValidationError::new("url_scheme"));
    }
    Ok(())
}

/// QoS level for MQTT publishes. Deserialised from a TOML integer
/// (`qos = 1`) and validated against the MQTT 5 fixed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(try_from = "u8")]
#[allow(clippy::enum_variant_names)] // matches MQTT spec terminology
pub enum MqttQos {
    AtMostOnce,
    #[default]
    AtLeastOnce,
    ExactlyOnce,
}

impl TryFrom<u8> for MqttQos {
    type Error = String;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::AtMostOnce),
            1 => Ok(Self::AtLeastOnce),
            2 => Ok(Self::ExactlyOnce),
            other => Err(format!("qos must be 0, 1, or 2; got {other}")),
        }
    }
}

/// `[sink.mqtt]` block. The password lives in an env var referenced
/// by `password_env`; TOML never holds the secret itself.
#[derive(Debug, Clone, Deserialize, Validate)]
#[cfg_attr(not(feature = "sink-mqtt"), allow(dead_code))]
pub struct MqttSinkConfig {
    /// Broker hostname or IP — no scheme prefix.
    #[validate(length(min = 1, max = 253))]
    pub broker_host: String,

    /// Broker port (1883 = plain MQTT, 8883 = MQTT-over-TLS by IANA).
    #[validate(range(min = 1, max = 65535))]
    pub broker_port: u16,

    /// MQTT client-id. MQTT 5 allows up to 65535 chars but most brokers
    /// cap at 23 for backwards-compat; we follow the conservative limit.
    #[validate(
        length(min = 1, max = 23),
        custom(function = "validate_mqtt_client_id")
    )]
    pub client_id: String,

    /// Optional MQTT username.
    #[serde(default)]
    #[validate(length(min = 1, max = 256))]
    pub username: Option<String>,

    /// Name of the env var holding the MQTT password (never the value).
    #[serde(default)]
    #[validate(
        length(min = 1, max = 256),
        custom(function = "validate_ascii_env_name")
    )]
    pub password_env: Option<String>,

    /// Topic prefix. Readings publish to `<prefix>/glucose`, health to
    /// `<prefix>/_health`. Typically `gluco-hub/<client_id>`.
    #[validate(length(min = 1, max = 200), custom(function = "validate_topic_prefix"))]
    pub topic_prefix: String,

    /// QoS for glucose publishes. 0/1/2 — defaults to 1.
    #[serde(default)]
    pub qos: MqttQos,

    /// Keep-alive interval in seconds. 30 is sensible for mobile/LTE.
    #[serde(default = "default_mqtt_keep_alive")]
    #[validate(range(min = 5, max = 300))]
    pub keep_alive_secs: u64,

    /// MQTT v5 session-expiry-interval in seconds. 0 = clean-start
    /// every connect (recommended for a stateless publisher).
    #[serde(default)]
    pub session_expiry_secs: u32,

    /// Enable TLS (rustls). Default `true`; flip to `false` only for
    /// local plaintext brokers in dev.
    #[serde(default = "default_true")]
    pub tls: bool,

    /// Whether to embed `patient_id` in the JSON payload. Defaults to
    /// `true` — flip to `false` for shared brokers where the bridge
    /// should not leak the patient identifier.
    #[serde(default = "default_true")]
    pub include_patient_id: bool,

    /// Interval, in seconds, at which the sink publishes a retained
    /// `<prefix>/_stats` snapshot. Defaults to 60. Lower bound 5
    /// (avoid hammering the broker), upper bound 3600 (an hour-old
    /// stats payload still meaningfully describes a long-lived sink).
    #[serde(default = "default_mqtt_stats_interval")]
    #[validate(range(min = 5, max = 3600))]
    pub stats_interval_secs: u64,
}

fn default_mqtt_keep_alive() -> u64 {
    30
}

fn default_mqtt_stats_interval() -> u64 {
    60
}

fn default_true() -> bool {
    true
}

fn validate_mqtt_client_id(value: &str) -> Result<(), ValidationError> {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(())
    } else {
        Err(ValidationError::new("mqtt_client_id_chars"))
    }
}

fn validate_topic_prefix(value: &str) -> Result<(), ValidationError> {
    // MQTT topics must not contain wildcards (#, +) and must not start
    // or end with '/'. Embedded '/' segments are fine.
    if value.contains('#') || value.contains('+') || value.starts_with('/') || value.ends_with('/')
    {
        return Err(ValidationError::new("topic_prefix_chars"));
    }
    Ok(())
}

/// `[source.llu]` block. The password is sourced from one of:
///   * `password_env` — name of an environment variable holding the secret.
///   * `password_file` — path to a 0600 file whose contents are the secret
///     (a single trailing CR/LF is stripped). Suits Docker/Podman secrets,
///     systemd `LoadCredential=`, and Kubernetes secret volumes.
///
/// Exactly one of the two MUST be set; the TOML never holds the secret.
#[derive(Debug, Clone, Deserialize, Validate)]
#[validate(schema(function = "validate_llu_secret_source"))]
pub struct LluSourceConfig {
    #[validate(email)]
    pub email: String,

    /// Name of the environment variable holding the LLU password. Mutually
    /// exclusive with `password_file`.
    #[serde(default)]
    #[validate(
        length(min = 1, max = 256),
        custom(function = "validate_ascii_env_name")
    )]
    pub password_env: Option<String>,

    /// Path to a file whose contents are the LLU password. Mutually
    /// exclusive with `password_env`. The file's content is read at
    /// startup and stripped of a single trailing `\n` or `\r\n`.
    #[serde(default)]
    pub password_file: Option<PathBuf>,

    /// LibreLink Up region (`EU`, `US`, `DE`, …). Validated against the
    /// canonical region table — unknown values fail at config load time.
    #[validate(custom(function = "validate_region"))]
    pub region: String,

    /// Optional patient identifier when the LLU account has more than one
    /// linked patient. When absent, the first connection is selected.
    #[validate(length(min = 1, max = 128))]
    pub patient_id: Option<String>,

    /// Optional LibreLink Up app version sent in the `version` header.
    /// Defaults to the binary's pinned `DEFAULT_LLU_VERSION` when unset.
    /// Override here, or at runtime via the env var
    /// `GLUCO_HUB__SOURCE__LLU__VERSION`, when LibreView rejects the
    /// pinned default — no recompile required.
    #[serde(default)]
    #[validate(length(min = 1, max = 32), custom(function = "validate_llu_version"))]
    pub version: Option<String>,

    /// IANA timezone of the LLU patient. LLU's `Timestamp` field carries
    /// the patient's local wall-clock time; without this hint the bridge
    /// cannot convert it to UTC and readings appear shifted by the local
    /// offset. Defaults to `UTC` when unset, which matches the historical
    /// (incorrect) parse behaviour and so preserves whatever a UTC-only
    /// deployment was already producing. Set to e.g. `Europe/Berlin`,
    /// `America/New_York`, etc.
    #[serde(default)]
    #[validate(length(min = 1, max = 64), custom(function = "validate_iana_tz"))]
    pub timezone: Option<String>,
}

/// Struct-level validator: exactly one of `password_env` / `password_file`
/// must be set. Returning a `ValidationError` here surfaces under the
/// existing `[CFG002]` config-validation error path.
fn validate_llu_secret_source(cfg: &LluSourceConfig) -> Result<(), ValidationError> {
    match (cfg.password_env.as_deref(), cfg.password_file.as_deref()) {
        (Some(_), None) | (None, Some(_)) => Ok(()),
        (Some(_), Some(_)) => Err(ValidationError::new("secret_source")
            .with_message("set exactly one of password_env / password_file, not both".into())),
        (None, None) => Err(ValidationError::new("secret_source")
            .with_message("either password_env or password_file is required".into())),
    }
}

/// Conservative version-string validator: every char must be ASCII
/// graphic or a literal space. Mirrors `reqwest::header::HeaderValue`'s
/// acceptance set so misconfiguration fails at config load, not deep
/// inside the HTTP layer at the first poll. Deliberately not semver-
/// strict — LibreView ships values like `4.16.0-rc1` from time to time.
fn validate_llu_version(value: &str) -> Result<(), ValidationError> {
    if value.is_empty() || !value.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
        return Err(ValidationError::new("llu_version_format"));
    }
    Ok(())
}

/// Reject anything chrono-tz cannot resolve to a real IANA zone. Gated
/// behind the `source-llu` feature only because that's where the value
/// is consumed; without the feature we still accept the syntactic form
/// so an operator can pre-fill the config.
fn validate_iana_tz(value: &str) -> Result<(), ValidationError> {
    #[cfg(feature = "source-llu")]
    {
        value
            .parse::<chrono_tz::Tz>()
            .map(|_| ())
            .map_err(|_| ValidationError::new("unknown_iana_timezone"))
    }
    #[cfg(not(feature = "source-llu"))]
    {
        if value.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
            Ok(())
        } else {
            Err(ValidationError::new("iana_timezone_format"))
        }
    }
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

    // Display intentionally omits the io::Error — `{:#}` chain-walking
    // (in main.rs and `anyhow::Error::source()`) appends it exactly once.
    #[error("[CFG004] failed to read secret file {}", path.display())]
    SecretFileRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("[CFG005] secret file is empty: {}", path.display())]
    SecretFileEmpty { path: PathBuf },
}

const DEFAULT_PATH: &str = "config.toml";
const ENV_PREFIX: &str = "GLUCO_HUB";

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
/// Resolve a single secret env var by name. Returns the raw value on
/// success; surfaces missing or empty values as `MissingSecret` so the
/// `[CFG003]` prefix is the same regardless of caller. The value
/// itself is NEVER logged or attached to the error.
pub fn resolve_secret_env(var_name: &str) -> Result<String, ConfigError> {
    let value = std::env::var(var_name).map_err(|_| ConfigError::MissingSecret {
        var: var_name.to_string(),
    })?;
    if value.is_empty() {
        return Err(ConfigError::MissingSecret {
            var: var_name.to_string(),
        });
    }
    Ok(value)
}

/// Read a secret from a file. Strips a single trailing CR/LF — many tools
/// (`echo`, editors) append a newline that is not part of the secret.
/// Returns `SecretFileEmpty` for an empty or all-whitespace-newline file.
/// The value is NEVER logged.
pub fn resolve_secret_file(path: &Path) -> Result<String, ConfigError> {
    let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::SecretFileRead {
        path: path.to_path_buf(),
        source,
    })?;
    let value = raw.trim_end_matches(['\r', '\n']).to_string();
    if value.is_empty() {
        return Err(ConfigError::SecretFileEmpty {
            path: path.to_path_buf(),
        });
    }
    Ok(value)
}

/// Verify that every referenced secret (env var or file) is reachable.
/// Called by `check-config` and at startup so misconfiguration fails fast —
/// no network round-trips required to learn that `LLU_PASSWORD` was forgotten
/// or that the password file path is wrong.
///
/// Secret values are never logged, returned, or stored; only their presence
/// is checked.
pub fn verify_secrets(cfg: &Config) -> Result<(), ConfigError> {
    if let Some(llu) = cfg.source.llu.as_ref() {
        match (llu.password_env.as_deref(), llu.password_file.as_deref()) {
            (Some(env), None) => {
                resolve_secret_env(env)?;
            }
            (None, Some(path)) => {
                resolve_secret_file(path)?;
            }
            // Validator enforces exactly one is set; both other cases
            // are rejected at config load.
            _ => {}
        }
    }
    if let Some(ns) = cfg.sink.nightscout.as_ref() {
        resolve_secret_env(&ns.api_secret_env)?;
    }
    if let Some(mqtt) = cfg.sink.mqtt.as_ref()
        && let Some(env_name) = mqtt.password_env.as_deref()
    {
        resolve_secret_env(env_name)?;
    }
    if let Some(name) = cfg.http.bearer_token_env.as_deref() {
        resolve_secret_env(name)?;
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

    /// `validate_region` is only strict when the `source-llu` feature is
    /// enabled (it consults the real `Region::parse` lookup). Without
    /// the feature the loose 2..=4-letter ASCII fallback accepts
    /// "MARS" — that's by design (operators preparing a config ahead
    /// of enabling the feature) but means the strict-rejection test
    /// only makes sense in the `source-llu` build.
    #[cfg(feature = "source-llu")]
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
    fn verify_secrets_detects_missing_env() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_with_llu(dir.path(), "EU", None);
        let cfg = load(Some(&path)).expect("load");
        // Use a guaranteed-unset variable; do not touch process env.
        let mut cfg = cfg;
        cfg.source.llu.as_mut().unwrap().password_env =
            Some("GLUCO_HUB_TEST_DEFINITELY_UNSET_VAR".to_string());
        let err = verify_secrets(&cfg).expect_err("missing");
        match err {
            ConfigError::MissingSecret { var } => {
                assert_eq!(var, "GLUCO_HUB_TEST_DEFINITELY_UNSET_VAR")
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn resolve_secret_file_reads_and_trims_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pw");
        std::fs::write(&path, "hunter2\n").unwrap();
        assert_eq!(resolve_secret_file(&path).unwrap(), "hunter2");

        let path_crlf = dir.path().join("pw_crlf");
        std::fs::write(&path_crlf, "hunter2\r\n").unwrap();
        assert_eq!(resolve_secret_file(&path_crlf).unwrap(), "hunter2");

        let path_no_eol = dir.path().join("pw_no_eol");
        std::fs::write(&path_no_eol, "hunter2").unwrap();
        assert_eq!(resolve_secret_file(&path_no_eol).unwrap(), "hunter2");
    }

    #[test]
    fn resolve_secret_file_rejects_empty() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty");
        std::fs::write(&empty, "").unwrap();
        assert!(matches!(
            resolve_secret_file(&empty),
            Err(ConfigError::SecretFileEmpty { .. })
        ));

        let only_newline = dir.path().join("only_newline");
        std::fs::write(&only_newline, "\n").unwrap();
        assert!(matches!(
            resolve_secret_file(&only_newline),
            Err(ConfigError::SecretFileEmpty { .. })
        ));
    }

    #[test]
    fn resolve_secret_file_reports_path_on_io_error() {
        let missing = std::path::Path::new("/nonexistent/dir/secret-file");
        match resolve_secret_file(missing) {
            Err(ConfigError::SecretFileRead { path, .. }) => assert_eq!(path, missing),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn validation_rejects_both_password_env_and_file() {
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
email = "patient@example.com"
password_env = "TEST_LLU_PASSWORD"
password_file = "/tmp/whatever"
region = "EU"
"#,
        )
        .unwrap();
        let err = load(Some(&path)).expect_err("must reject");
        assert!(matches!(err, ConfigError::Validate(_)));
    }

    #[test]
    fn validation_rejects_neither_password_env_nor_file() {
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
email = "patient@example.com"
region = "EU"
"#,
        )
        .unwrap();
        let err = load(Some(&path)).expect_err("must reject");
        assert!(matches!(err, ConfigError::Validate(_)));
    }

    #[test]
    fn loads_with_password_file() {
        let dir = tempfile::tempdir().unwrap();
        let pw_path = dir.path().join("llu_pw");
        std::fs::write(&pw_path, "secret").unwrap();
        let cfg_path = dir.path().join("config.toml");
        std::fs::write(
            &cfg_path,
            format!(
                r#"
[http]
bind = "127.0.0.1:9000"

[poller]
interval_secs = 60

[source.llu]
email = "patient@example.com"
password_file = "{}"
region = "EU"
"#,
                pw_path.display()
            ),
        )
        .unwrap();
        let cfg = load(Some(&cfg_path)).expect("load");
        let llu = cfg.source.llu.as_ref().expect("llu present");
        assert!(llu.password_env.is_none());
        assert_eq!(llu.password_file.as_deref(), Some(pw_path.as_path()));
        verify_secrets(&cfg).expect("secret resolves");
    }
}
