// SPDX-License-Identifier: AGPL-3.0-or-later

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use ::config::{Config as ConfigBuilder, Environment, File, FileFormat};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use thiserror::Error;
use validator::{Validate, ValidationError};

/// Top-level application configuration.
///
/// Loaded from a TOML file (default `./config.toml`) and overridden by
/// `GLUCO_HUB__<SECTION>__<KEY>` environment variables. Secrets should be
/// supplied via environment variables (e.g. `GLUCO_HUB__SOURCE__LLU__PASSWORD`)
/// or via `password_file`. Never embed secrets directly in the TOML file.
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

    /// On-disk state used by the dead-letter queue. Defaults to
    /// `./state` (relative to CWD). In containers, mount a persistent
    /// volume here.
    #[serde(default)]
    #[validate(nested)]
    pub state: StateConfig,

    /// Dead-letter queue knobs — see `gluco-hub/src/dlq.rs` for
    /// behaviour. Defaults are sensible for a home-grade CGM bridge.
    #[serde(default)]
    #[validate(nested)]
    pub dlq: DlqConfig,
}

#[derive(Debug, Clone, Deserialize, Validate)]
pub struct StateConfig {
    /// Directory that holds gluco-hub's persistent state files (DLQ
    /// JSONL today; watermark snapshots later). Created on startup if
    /// missing.
    #[serde(default = "default_state_dir")]
    pub dir: std::path::PathBuf,
}

impl Default for StateConfig {
    fn default() -> Self {
        Self {
            dir: default_state_dir(),
        }
    }
}

fn default_state_dir() -> std::path::PathBuf {
    std::path::PathBuf::from("./state")
}

#[derive(Debug, Clone, Deserialize, Validate)]
pub struct DlqConfig {
    /// Master toggle. When `false`, sink failures behave as before
    /// V3-DLQ (lost on restart, replayed only within LLU's 24 h
    /// `graphData` window).
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Per-sink hard cap on queued readings. When exceeded, the oldest
    /// readings are evicted (logged + `cgm_dlq_evicted_total`). 10000
    /// covers ~35 days at the 5-min LLU raster.
    #[serde(default = "default_dlq_max_entries")]
    #[validate(range(min = 100, max = 1_000_000))]
    pub max_entries: usize,
}

impl Default for DlqConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_entries: default_dlq_max_entries(),
        }
    }
}

fn default_dlq_max_entries() -> usize {
    10_000
}

#[derive(Debug, Clone, Deserialize, Validate)]
pub struct HttpConfig {
    /// Master toggle for the embedded axum HTTP server (the cache API,
    /// `/healthz`, and `/metrics`). When `false`, the binary still
    /// polls the source and pushes to sinks — only the local HTTP
    /// listener is suppressed. Defaults to `true` (backwards-compatible).
    /// Useful for MQTT-only deployments (e.g. Home Assistant) where
    /// liveness is observed via the heartbeat file under `state.dir`.
    #[serde(default = "default_true")]
    pub enabled: bool,

    pub bind: SocketAddr,

    /// Optional Bearer token. When set, `/glucose/*` requires
    /// `Authorization: Bearer <token>`. Supply via
    /// `GLUCO_HUB__HTTP__BEARER_TOKEN`. `/healthz` and `/metrics` stay public.
    /// Ignored (with a startup warning) when `enabled = false`.
    #[serde(default)]
    pub bearer_token: Option<SecretString>,
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
    /// Legacy single-source block. Works identically to v4.x.
    #[serde(default)]
    #[validate(nested)]
    pub llu: Option<LluSourceConfig>,

    /// Named multi-source blocks: each key becomes the source name
    /// (used in MQTT topic prefixes, client IDs, log context).
    /// When `sources` is non-empty, `llu` is ignored.
    #[serde(default)]
    #[validate(nested)]
    pub sources: HashMap<String, LluSourceConfig>,

    /// V6 NS-Socket source. Parsed unconditionally; honoured only when the
    /// `source-ns-socket` feature is enabled (see `build_default_source` in
    /// `main.rs`). Standalone alternative to LLU.
    #[serde(default)]
    #[validate(nested)]
    pub ns_socket: Option<NsSocketSourceConfig>,
}

/// How the NS-Socket source authenticates to Nightscout's Socket.IO
/// `authorize` handshake. Fixed set — never a magic string.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NsSocketAuthMode {
    /// Access token (e.g. `myreader-0123456789abcdef`) sent as the
    /// `authorize` payload's `token` field. The default — preferred on
    /// modern Nightscout deployments.
    #[default]
    Token,
    /// API secret (raw, NOT pre-hashed) — the source hashes it to SHA-1 and
    /// sends it as the `authorize` payload's `secret` field.
    ApiSecret,
}

/// `[source.ns_socket]` block (Roadmap V6). Supply the credential via the
/// environment: `GLUCO_HUB__SOURCE__NS_SOCKET__TOKEN` for `auth = "token"`
/// (the default) or `GLUCO_HUB__SOURCE__NS_SOCKET__API_SECRET` for
/// `auth = "api_secret"`. Never embed either in TOML.
#[cfg_attr(not(feature = "source-ns-socket"), allow(dead_code))]
#[derive(Debug, Clone, Deserialize, Validate)]
pub struct NsSocketSourceConfig {
    /// Base URL of the Nightscout site, e.g. `https://ns.example.com`.
    #[validate(length(min = 5, max = 512), custom(function = "validate_http_url"))]
    pub base_url: String,

    /// Credential type. Defaults to `token`.
    #[serde(default)]
    pub auth: NsSocketAuthMode,

    /// Nightscout access token. Required when `auth = "token"`. Supply via
    /// `GLUCO_HUB__SOURCE__NS_SOCKET__TOKEN`.
    #[serde(default)]
    pub token: Option<SecretString>,

    /// Nightscout API secret (raw, NOT pre-hashed). Required when
    /// `auth = "api_secret"`. Supply via
    /// `GLUCO_HUB__SOURCE__NS_SOCKET__API_SECRET`.
    #[serde(default)]
    pub api_secret: Option<SecretString>,

    /// History window, in hours, requested in the `authorize` handshake.
    /// Defaults to 48 (Nightscout's own default). Bounds the initial replay.
    #[serde(default = "default_ns_socket_history_hours")]
    #[validate(range(min = 1, max = 168))]
    pub history_hours: u32,
}

fn default_ns_socket_history_hours() -> u32 {
    48
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

/// `[sink.nightscout]` block. Supply the API secret via the environment
/// variable `GLUCO_HUB__SINK__NIGHTSCOUT__API_SECRET`; do not embed it in TOML.
#[cfg_attr(not(feature = "sink-nightscout"), allow(dead_code))]
#[derive(Debug, Clone, Deserialize, Validate)]
pub struct NightscoutSinkConfig {
    #[validate(length(min = 5, max = 512), custom(function = "validate_http_url"))]
    pub base_url: String,

    /// Nightscout API secret (raw, NOT pre-hashed). Supply via
    /// `GLUCO_HUB__SINK__NIGHTSCOUT__API_SECRET`.
    pub api_secret: SecretString,

    /// Identifies this service in the NS UI's source column. Defaults
    /// to `"gluco-hub"`.
    #[serde(default)]
    #[validate(length(min = 1, max = 128))]
    pub device: Option<String>,

    /// App name attached to every uploaded entry. Defaults to
    /// `"gluco-hub"`.
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

/// Selects which glucose unit the Home Assistant MQTT discovery sensor
/// reports as its state value. The wire payload always carries both
/// `mgdl` and `mmol` fields (see [`crate::sinks::mqtt::wire::GlucosePayload`]);
/// this option only switches the `unit_of_measurement` string and the
/// `value_template` HA reads. Default `mgdl` preserves the V2 / V3
/// behaviour and matches US clinical convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MqttGlucoseUnit {
    /// Report `mg/dL`. Default — preserves existing behaviour.
    #[default]
    MgDl,
    /// Report `mmol/L`. Sensible for EU / UK / most-of-world deployments.
    Mmol,
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

/// `[sink.mqtt]` block. Supply the password via
/// `GLUCO_HUB__SINK__MQTT__PASSWORD`; do not embed it in TOML.
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

    /// Optional MQTT password. Supply via `GLUCO_HUB__SINK__MQTT__PASSWORD`.
    #[serde(default)]
    pub password: Option<SecretString>,

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

    /// Enable Home Assistant MQTT auto-discovery. When `true`, the sink
    /// publishes a retained config message on
    /// `<discovery_prefix>/sensor/<unique_id>/config` after each ConnAck
    /// so HA picks the glucose sensor up automatically. Opt-in (default
    /// `false`) to avoid surprising operators on shared brokers.
    #[serde(default)]
    pub discovery_enabled: bool,

    /// Topic prefix that Home Assistant subscribes to for discovery.
    /// HA's own default is `homeassistant`; only change this if the HA
    /// instance has been reconfigured to a non-default prefix.
    #[serde(default = "default_discovery_prefix")]
    #[validate(length(min = 1, max = 200), custom(function = "validate_topic_prefix"))]
    pub discovery_prefix: String,

    /// Friendly device name shown in the HA UI. Defaults to
    /// `Gluco Hub (<client_id>)` when absent.
    #[serde(default)]
    #[validate(length(min = 1, max = 128))]
    pub device_name: Option<String>,

    /// Unit reported by the HA discovery sensor entity. The wire
    /// payload always carries both `mgdl` and `mmol`; this switches the
    /// `unit_of_measurement` + `value_template` HA reads. Default
    /// `mgdl` preserves V2 / V3 behaviour.
    #[serde(default)]
    pub discovery_unit: MqttGlucoseUnit,

    /// Path to a PEM-encoded client certificate for mTLS.
    /// When set together with `client_key_file`, the MQTT sink
    /// presents a client certificate during the TLS handshake.
    /// Leave unset for standard server-only TLS.
    #[serde(default)]
    #[validate(length(min = 1, max = 512), custom(function = "validate_file_path"))]
    pub client_cert_file: Option<String>,

    /// Path to a PEM-encoded client private key for mTLS.
    /// Must be paired with `client_cert_file`.
    #[serde(default)]
    #[validate(length(min = 1, max = 512), custom(function = "validate_file_path"))]
    pub client_key_file: Option<String>,

    /// Optional Tailscale MagicDNS hostname of the MQTT broker. When set,
    /// gluco-hub resolves this hostname to a tailnet IP via the local
    /// `tailscaled` daemon's API at startup and uses the resolved IP as
    /// `broker_host`. The `tailscaled` daemon must be running on the
    /// same host (or as a sidecar container). Falls back to `broker_host`
    /// if tailscaled is unreachable or the hostname is not found.
    #[serde(default)]
    #[validate(length(min = 1, max = 253))]
    pub tailscale_hostname: Option<String>,

    /// When true, MQTT topic_prefix and client_id are suffixed with
    /// the source name. When false (default, backward compat), they
    /// are used exactly as configured.
    #[serde(default)]
    pub per_source: bool,
}

fn default_mqtt_keep_alive() -> u64 {
    30
}

fn default_mqtt_stats_interval() -> u64 {
    60
}

fn default_discovery_prefix() -> String {
    "homeassistant".to_string()
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

/// Reject file paths that contain null bytes or are only whitespace.
fn validate_file_path(value: &str) -> Result<(), ValidationError> {
    if value.contains('\0') {
        return Err(ValidationError::new("file_path_null"));
    }
    if value.trim().is_empty() {
        return Err(ValidationError::new("file_path_empty"));
    }
    Ok(())
}

/// `[source.llu]` block. The password is sourced from one of:
///   * `password` — supplied via `GLUCO_HUB__SOURCE__LLU__PASSWORD` (never in TOML).
///   * `password_file` — path to a 0600 file whose contents are the secret
///     (a single trailing CR/LF is stripped). Suits Docker/Podman secrets,
///     systemd `LoadCredential=`, and Kubernetes secret volumes.
///
/// Exactly one of the two MUST be set.
#[derive(Debug, Clone, Deserialize, Validate)]
#[validate(schema(function = "validate_llu_secret_source"))]
pub struct LluSourceConfig {
    #[validate(email)]
    pub email: String,

    /// LLU password. Supply via `GLUCO_HUB__SOURCE__LLU__PASSWORD`.
    /// Mutually exclusive with `password_file`.
    #[serde(default)]
    pub password: Option<SecretString>,

    /// Path to a file whose contents are the LLU password. Mutually
    /// exclusive with `password`. The file's content is read at startup
    /// and stripped of a single trailing `\n` or `\r\n`.
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

/// Struct-level validator: exactly one of `password` / `password_file`
/// must be set. Returning a `ValidationError` here surfaces under the
/// existing `[CFG002]` config-validation error path.
fn validate_llu_secret_source(cfg: &LluSourceConfig) -> Result<(), ValidationError> {
    match (cfg.password.as_ref(), cfg.password_file.as_deref()) {
        (Some(_), None) | (None, Some(_)) => Ok(()),
        (Some(_), Some(_)) => Err(ValidationError::new("secret_source")
            .with_message("set exactly one of password / password_file, not both".into())),
        (None, None) => Err(ValidationError::new("secret_source")
            .with_message("either password (via env) or password_file is required".into())),
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

    // Display intentionally omits the io::Error — `{:#}` chain-walking
    // (in main.rs and `anyhow::Error::source()`) appends it exactly once.
    #[error("[CFG003] failed to read secret file {}", path.display())]
    SecretFileRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("[CFG004] secret file is empty: {}", path.display())]
    SecretFileEmpty { path: PathBuf },

    /// Built only when at least one optional Source/Sink feature is OFF.
    /// With `--all-features` the variant is never constructed; suppress
    /// the resulting dead-code lint there.
    #[cfg_attr(
        all(
            feature = "source-llu",
            feature = "source-ns-socket",
            feature = "sink-nightscout",
            feature = "sink-mqtt"
        ),
        allow(dead_code)
    )]
    #[error("[CFG006] {section} configured but binary built without `{feature}` feature")]
    FeatureMismatch {
        section: &'static str,
        feature: &'static str,
    },

    #[error("[CFG007] {field} is empty (likely an unset env var)")]
    EmptySecret { field: String },
}

const DEFAULT_PATH: &str = "config.toml";
const ENV_PREFIX: &str = "GLUCO_HUB";

/// Load and validate configuration. If `override_path` is `None`, the
/// loader first tries `./config.toml` and otherwise falls back to a set of
/// built-in defaults suitable for local development.
pub fn load(override_path: Option<&Path>) -> Result<Config, ConfigError> {
    let mut builder = ConfigBuilder::builder()
        .set_default("http.enabled", true)?
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

/// Verify that file-based secrets are readable AND that ENV-injected
/// secrets are non-empty (an unset `GLUCO_HUB__…` var deserialises into
/// `SecretString("")`, which the `config` crate cannot reject by length).
/// Called by `check-config` and at startup so misconfiguration fails fast
/// with a clear error code instead of a deep transport-layer 401.
pub fn verify_secrets(cfg: &Config) -> Result<(), ConfigError> {
    if let Some(llu) = cfg.source.llu.as_ref() {
        match (llu.password.as_ref(), llu.password_file.as_deref()) {
            (None, Some(path)) => {
                resolve_secret_file(path)?;
            }
            (Some(secret), None) if secret.expose_secret().is_empty() => {
                return Err(ConfigError::EmptySecret {
                    field: "[source.llu] password".to_string(),
                });
            }
            _ => {}
        }
    }

    for (name, llu) in &cfg.source.sources {
        match (llu.password.as_ref(), llu.password_file.as_deref()) {
            (None, Some(path)) => {
                resolve_secret_file(path)?;
            }
            (Some(secret), None) if secret.expose_secret().is_empty() => {
                return Err(ConfigError::EmptySecret {
                    field: format!("[source.sources.{name}] password"),
                });
            }
            _ => {}
        }
    }

    if let Some(ns) = cfg.source.ns_socket.as_ref() {
        // The credential required depends on the selected auth mode. An
        // empty or absent value is rejected the same way.
        let (value, field): (Option<&SecretString>, &'static str) = match ns.auth {
            NsSocketAuthMode::Token => (ns.token.as_ref(), "[source.ns_socket] token"),
            NsSocketAuthMode::ApiSecret => {
                (ns.api_secret.as_ref(), "[source.ns_socket] api_secret")
            }
        };
        if value.is_none_or(|s| s.expose_secret().is_empty()) {
            return Err(ConfigError::EmptySecret {
                field: field.to_string(),
            });
        }
    }

    if let Some(token) = cfg.http.bearer_token.as_ref()
        && token.expose_secret().is_empty()
    {
        return Err(ConfigError::EmptySecret {
            field: "[http] bearer_token".to_string(),
        });
    }

    if let Some(ns) = cfg.sink.nightscout.as_ref()
        && ns.api_secret.expose_secret().is_empty()
    {
        return Err(ConfigError::EmptySecret {
            field: "[sink.nightscout] api_secret".to_string(),
        });
    }

    if let Some(mqtt) = cfg.sink.mqtt.as_ref()
        && let Some(pw) = mqtt.password.as_ref()
        && pw.expose_secret().is_empty()
    {
        return Err(ConfigError::EmptySecret {
            field: "[sink.mqtt] password".to_string(),
        });
    }

    Ok(())
}

/// Reject TOML blocks that reference Sources/Sinks whose Cargo feature
/// is not compiled into this binary. Without this check the operator
/// gets silent data loss: `[sink.mqtt]` deserialises fine, validates
/// fine, then is ignored at wiring time because `build_sinks` is
/// `#[cfg(feature = "sink-mqtt")]`-gated. Fail loudly instead.
pub fn verify_features(cfg: &Config) -> Result<(), ConfigError> {
    let _ = cfg;

    #[cfg(not(feature = "source-llu"))]
    if cfg.source.llu.is_some() {
        return Err(ConfigError::FeatureMismatch {
            section: "[source.llu]",
            feature: "source-llu",
        });
    }
    #[cfg(not(feature = "source-llu"))]
    if !cfg.source.sources.is_empty() {
        return Err(ConfigError::FeatureMismatch {
            section: "[source.sources]",
            feature: "source-llu",
        });
    }
    #[cfg(not(feature = "source-ns-socket"))]
    if cfg.source.ns_socket.is_some() {
        return Err(ConfigError::FeatureMismatch {
            section: "[source.ns_socket]",
            feature: "source-ns-socket",
        });
    }
    #[cfg(not(feature = "sink-nightscout"))]
    if cfg.sink.nightscout.is_some() {
        return Err(ConfigError::FeatureMismatch {
            section: "[sink.nightscout]",
            feature: "sink-nightscout",
        });
    }
    #[cfg(not(feature = "sink-mqtt"))]
    if cfg.sink.mqtt.is_some() {
        return Err(ConfigError::FeatureMismatch {
            section: "[sink.mqtt]",
            feature: "sink-mqtt",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_load_without_file() {
        let cfg = load(Some(Path::new("/nonexistent.toml"))).expect("defaults must load");
        assert!(cfg.http.enabled, "http.enabled defaults to true");
        assert_eq!(cfg.http.bind.to_string(), "127.0.0.1:8080");
        assert_eq!(cfg.poller.interval_secs, 60);
        assert!(cfg.source.llu.is_none());
    }

    #[test]
    fn parses_ns_socket_section_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[http]
bind = "127.0.0.1:9000"

[poller]
interval_secs = 60

[source.ns_socket]
base_url = "https://ns.example.com"
"#,
        )
        .unwrap();
        let cfg = load(Some(&path)).expect("load");
        let ns = cfg.source.ns_socket.expect("ns_socket present");
        assert_eq!(ns.base_url, "https://ns.example.com");
        // auth defaults to token; history defaults to 48.
        assert_eq!(ns.auth, NsSocketAuthMode::Token);
        assert_eq!(ns.history_hours, 48);
    }

    #[test]
    fn rejects_ns_socket_non_http_base_url() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[http]
bind = "127.0.0.1:9000"

[poller]
interval_secs = 60

[source.ns_socket]
base_url = "wss://ns.example.com"
"#,
        )
        .unwrap();
        let err = load(Some(&path)).expect_err("must reject non-http scheme");
        assert!(matches!(err, ConfigError::Validate(_)));
    }

    #[test]
    fn verify_secrets_rejects_missing_ns_socket_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[http]
bind = "127.0.0.1:9000"

[poller]
interval_secs = 60

[source.ns_socket]
base_url = "https://ns.example.com"
auth = "token"
"#,
        )
        .unwrap();
        let cfg = load(Some(&path)).expect("load");
        let err = verify_secrets(&cfg).expect_err("must reject missing token");
        assert!(
            matches!(&err, ConfigError::EmptySecret { field } if field.contains("token")),
            "unexpected: {err:?}"
        );
    }

    #[test]
    fn verify_secrets_rejects_empty_ns_socket_api_secret() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[http]
bind = "127.0.0.1:9000"

[poller]
interval_secs = 60

[source.ns_socket]
base_url = "https://ns.example.com"
auth = "api_secret"
"#,
        )
        .unwrap();
        let mut cfg = load(Some(&path)).expect("load");
        cfg.source.ns_socket.as_mut().unwrap().api_secret = Some(SecretString::from(String::new()));
        let err = verify_secrets(&cfg).expect_err("must reject empty api_secret");
        assert!(
            matches!(&err, ConfigError::EmptySecret { field } if field.contains("api_secret")),
            "unexpected: {err:?}"
        );
    }

    /// MQTT-only deployments (e.g. the HA add-on) disable the listener
    /// to avoid running an unused TCP server. `http.enabled = false` in
    /// the TOML must round-trip without touching other config knobs.
    #[test]
    fn http_enabled_round_trips_from_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
            [http]
            enabled = false
            bind = "127.0.0.1:8080"

            [poller]
            interval_secs = 60
            "#,
        )
        .unwrap();
        let cfg = load(Some(&path)).expect("must load");
        assert!(!cfg.http.enabled);
        // bind is still parsed even when disabled — the schema stays
        // uniform regardless of the toggle.
        assert_eq!(cfg.http.bind.to_string(), "127.0.0.1:8080");
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
password = "test_secret"
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
password = "test_secret"
region = "EU"
"#,
        )
        .unwrap();
        let err = load(Some(&path)).expect_err("must reject");
        assert!(matches!(err, ConfigError::Validate(_)));
    }

    #[test]
    fn verify_secrets_detects_unreadable_password_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_with_llu(dir.path(), "EU", None);
        let cfg = load(Some(&path)).expect("load");
        let mut cfg = cfg;
        cfg.source.llu.as_mut().unwrap().password = None;
        cfg.source.llu.as_mut().unwrap().password_file =
            Some(std::path::PathBuf::from("/nonexistent/dir/secret-file"));
        let err = verify_secrets(&cfg).expect_err("missing file");
        assert!(matches!(err, ConfigError::SecretFileRead { .. }));
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
    fn verify_secrets_rejects_empty_llu_password() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_with_llu(dir.path(), "EU", None);
        let mut cfg = load(Some(&path)).expect("load");
        cfg.source.llu.as_mut().unwrap().password = Some(SecretString::from(String::new()));
        let err = verify_secrets(&cfg).expect_err("must reject empty password");
        assert!(
            matches!(err, ConfigError::EmptySecret { ref field } if field.contains("password")),
            "unexpected: {err:?}"
        );
    }

    #[cfg(feature = "sink-nightscout")]
    #[test]
    fn verify_secrets_rejects_empty_ns_api_secret() {
        let cfg = Config {
            http: HttpConfig {
                enabled: true,
                bind: "127.0.0.1:0".parse().unwrap(),
                bearer_token: None,
            },
            poller: PollerConfig { interval_secs: 60 },
            source: SourceConfig::default(),
            sink: SinkConfig {
                nightscout: Some(NightscoutSinkConfig {
                    base_url: "https://ns.example".into(),
                    api_secret: SecretString::from(String::new()),
                    device: None,
                    app: None,
                }),
                mqtt: None,
            },
            state: StateConfig::default(),
            dlq: DlqConfig::default(),
        };
        let err = verify_secrets(&cfg).expect_err("must reject empty api_secret");
        assert!(
            matches!(err, ConfigError::EmptySecret { ref field } if field.contains("api_secret")),
            "unexpected: {err:?}"
        );
    }
    #[test]

    fn verify_secrets_rejects_empty_bearer_token() {
        let cfg = Config {
            http: HttpConfig {
                enabled: true,
                bind: "127.0.0.1:0".parse().unwrap(),
                bearer_token: Some(SecretString::from(String::new())),
            },
            poller: PollerConfig { interval_secs: 60 },
            source: SourceConfig::default(),
            sink: SinkConfig::default(),
            state: StateConfig::default(),
            dlq: DlqConfig::default(),
        };
        let err = verify_secrets(&cfg).expect_err("must reject empty bearer_token");
        assert!(
            matches!(err, ConfigError::EmptySecret { ref field } if field.contains("bearer_token")),
            "unexpected: {err:?}"
        );
    }

    /// `verify_features` only triggers in builds that lack the feature
    /// referenced by the TOML block. With `default = ["source-llu",
    /// "sink-nightscout"]` and `--all-features` the function is a no-op,
    /// so the meaningful assertion is "all-features build accepts every
    /// configured block". Negative tests live in the `--no-default-features`
    /// CI lane (see `.github/workflows/ci.yml`).
    #[test]
    fn verify_features_accepts_when_all_features_match() {
        let cfg = Config {
            http: HttpConfig {
                enabled: true,
                bind: "127.0.0.1:0".parse().unwrap(),
                bearer_token: None,
            },
            poller: PollerConfig { interval_secs: 60 },
            source: SourceConfig::default(),
            sink: SinkConfig::default(),
            state: StateConfig::default(),
            dlq: DlqConfig::default(),
        };
        verify_features(&cfg).expect("empty config always accepted");
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
    fn validation_rejects_both_password_and_file() {
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
password = "test_secret"
password_file = "/tmp/whatever"
region = "EU"
"#,
        )
        .unwrap();
        let err = load(Some(&path)).expect_err("must reject");
        assert!(matches!(err, ConfigError::Validate(_)));
    }

    #[test]
    fn validation_rejects_neither_password_nor_file() {
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
        assert!(llu.password.is_none());
        assert_eq!(llu.password_file.as_deref(), Some(pw_path.as_path()));
        verify_secrets(&cfg).expect("secret resolves");
    }

    /// Multi-source `[source.sources]` block: each named entry becomes
    /// its own LLU source. Legacy `[source.llu]` is NOT present.
    #[test]
    fn parses_multi_source_block() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[http]
bind = "127.0.0.1:9000"

[poller]
interval_secs = 60

[source.sources.alice]
email = "alice@example.com"
password = "secret1"
region = "EU"
patient_id = "patient-alice"

[source.sources.bob]
email = "bob@example.com"
password = "secret2"
region = "US"
"#,
        )
        .unwrap();
        let cfg = load(Some(&path)).expect("load");
        assert!(cfg.source.llu.is_none(), "llu should be absent");
        assert_eq!(cfg.source.sources.len(), 2);

        let alice = cfg.source.sources.get("alice").expect("alice present");
        assert_eq!(alice.email, "alice@example.com");
        assert_eq!(alice.region, "EU");
        assert_eq!(alice.patient_id.as_deref(), Some("patient-alice"));

        let bob = cfg.source.sources.get("bob").expect("bob present");
        assert_eq!(bob.email, "bob@example.com");
        assert_eq!(bob.region, "US");
        assert!(bob.patient_id.is_none());
    }

    /// When both `[source.llu]` and `[source.sources]` are configured,
    /// both are parsed and present in the struct. The priority (llu
    /// wins) is enforced at wiring time in `build_default_source`.
    #[test]
    fn parses_both_llu_and_sources_blocks() {
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
email = "legacy@example.com"
password = "legacy_secret"
region = "EU"

[source.sources.alice]
email = "alice@example.com"
password = "alice_secret"
region = "EU"
"#,
        )
        .unwrap();
        let cfg = load(Some(&path)).expect("load");
        assert!(cfg.source.llu.is_some(), "llu should be present");
        assert_eq!(cfg.source.sources.len(), 1);
        assert!(cfg.source.sources.contains_key("alice"));
    }
}
