use anyhow::Context;
use axum_server::tls_rustls::RustlsConfig;
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use serde::de::Error;
use serde::{Deserialize, Deserializer};
use std::sync::Arc;
use std::time::Duration;
use strum_macros::AsRefStr;

#[derive(Debug, Clone, Deserialize)]
pub struct EnvironmentConfig {
    #[serde(default)]
    pub cors_permissive: bool,
    pub provider: ProviderConfig,

    #[serde(default)]
    pub release_zip_password: Option<Arc<str>>,

    #[serde(
        deserialize_with = "deserialize_duration",
        default = "default_refresh_interval"
    )]
    pub poll_interval: Duration,

    #[serde(default)]
    pub otel_enabled: bool,

    #[serde(default)]
    pub http_ssl: Option<HttpSslConfig>,

    #[serde(default)]
    pub tsgo: TsgoConfig,

    #[serde(default)]
    pub connectors: ConnectorsConfig,

    #[serde(default)]
    pub decision_log: DecisionLogConfig,
}

fn default_refresh_interval() -> Duration {
    Duration::from_millis(5_000)
}

/// Type resolution for function nodes and policies, which runs the TypeScript
/// compiler as a WASI module under wasmtime.
#[derive(Debug, Clone, Deserialize)]
pub struct TsgoConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default = "default_tsgo_memory_bytes")]
    pub memory_bytes: usize,

    #[serde(
        deserialize_with = "deserialize_millis",
        default = "default_tsgo_timeout"
    )]
    pub timeout: Duration,

    /// Resolved function types held per process, keyed by source and input
    /// type. Cleared wholesale when full — the working set is one release's
    /// functions, so this is a ceiling, not an eviction policy.
    #[serde(default = "default_tsgo_cache_capacity")]
    pub cache_capacity: usize,
}

fn default_true() -> bool {
    true
}

fn default_tsgo_memory_bytes() -> usize {
    2 << 30
}

fn default_tsgo_timeout() -> Duration {
    Duration::from_secs(30)
}

fn default_tsgo_cache_capacity() -> usize {
    4_096
}

impl Default for TsgoConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            memory_bytes: default_tsgo_memory_bytes(),
            timeout: default_tsgo_timeout(),
            cache_capacity: default_tsgo_cache_capacity(),
        }
    }
}

/// What connector nodes may do when a decision calls an outside service. A
/// node may ask for less time or fewer retries, never more than the maximums.
#[derive(Debug, Clone, Deserialize)]
pub struct ConnectorsConfig {
    #[serde(
        deserialize_with = "deserialize_millis",
        default = "default_connector_timeout"
    )]
    pub timeout: Duration,

    #[serde(
        deserialize_with = "deserialize_millis",
        default = "default_connector_max_timeout"
    )]
    pub max_timeout: Duration,

    #[serde(default = "default_connector_retries")]
    pub retries: u32,

    #[serde(default = "default_connector_max_retries")]
    pub max_retries: u32,

    /// Consecutive failed calls to one URL that pause calls to it…
    #[serde(default = "default_connector_breaker_failures")]
    pub breaker_failures: u32,

    /// …for this long.
    #[serde(
        deserialize_with = "deserialize_millis",
        default = "default_connector_breaker_cooldown"
    )]
    pub breaker_cooldown: Duration,
}

fn default_connector_timeout() -> Duration {
    Duration::from_millis(3_000)
}

fn default_connector_max_timeout() -> Duration {
    Duration::from_millis(10_000)
}

fn default_connector_retries() -> u32 {
    1
}

fn default_connector_max_retries() -> u32 {
    3
}

fn default_connector_breaker_failures() -> u32 {
    5
}

fn default_connector_breaker_cooldown() -> Duration {
    Duration::from_millis(30_000)
}

impl Default for ConnectorsConfig {
    fn default() -> Self {
        Self {
            timeout: default_connector_timeout(),
            max_timeout: default_connector_max_timeout(),
            retries: default_connector_retries(),
            max_retries: default_connector_max_retries(),
            breaker_failures: default_connector_breaker_failures(),
            breaker_cooldown: default_connector_breaker_cooldown(),
        }
    }
}

/// Where the Runtime sends a record of every evaluation (Donka Studio's
/// decision log). Off unless both `url` and `token` are set.
#[derive(Clone, Deserialize)]
pub struct DecisionLogConfig {
    /// Studio's feed endpoint, e.g. `https://studio.example/api/v1/decision-log/records`.
    #[serde(default)]
    pub url: Option<String>,

    /// The decision-log token Studio issued for this Runtime's environment.
    #[serde(default)]
    pub token: Option<String>,

    /// Records sent together, at most.
    #[serde(default = "default_decision_log_batch_size")]
    pub batch_size: usize,

    /// Bytes of records sent together, at most (a record larger than this goes alone).
    #[serde(default = "default_decision_log_batch_bytes")]
    pub batch_bytes: usize,

    /// How long a record waits for others before its batch is sent.
    #[serde(
        deserialize_with = "deserialize_millis",
        default = "default_decision_log_flush_interval"
    )]
    pub flush_interval: Duration,

    /// Records held while Studio cannot be reached; beyond this, new records are dropped
    /// (and counted in the logs) rather than slowing evaluations down.
    #[serde(default = "default_decision_log_queue_capacity")]
    pub queue_capacity: usize,

    /// Time allowed for one send.
    #[serde(
        deserialize_with = "deserialize_millis",
        default = "default_decision_log_timeout"
    )]
    pub timeout: Duration,

    /// Time allowed, when the Runtime stops, to send what is still queued.
    #[serde(
        deserialize_with = "deserialize_millis",
        default = "default_decision_log_shutdown_timeout"
    )]
    pub shutdown_timeout: Duration,
}

// The token never reaches a log line.
impl std::fmt::Debug for DecisionLogConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecisionLogConfig")
            .field("url", &self.url)
            .field("token", &self.token.as_ref().map(|_| "<set>"))
            .field("batch_size", &self.batch_size)
            .field("batch_bytes", &self.batch_bytes)
            .field("flush_interval", &self.flush_interval)
            .field("queue_capacity", &self.queue_capacity)
            .field("timeout", &self.timeout)
            .field("shutdown_timeout", &self.shutdown_timeout)
            .finish()
    }
}

fn default_decision_log_batch_size() -> usize {
    100
}

fn default_decision_log_batch_bytes() -> usize {
    4 << 20
}

fn default_decision_log_flush_interval() -> Duration {
    Duration::from_millis(1_000)
}

fn default_decision_log_queue_capacity() -> usize {
    10_000
}

fn default_decision_log_timeout() -> Duration {
    Duration::from_millis(10_000)
}

fn default_decision_log_shutdown_timeout() -> Duration {
    Duration::from_millis(10_000)
}

impl Default for DecisionLogConfig {
    fn default() -> Self {
        Self {
            url: None,
            token: None,
            batch_size: default_decision_log_batch_size(),
            batch_bytes: default_decision_log_batch_bytes(),
            flush_interval: default_decision_log_flush_interval(),
            queue_capacity: default_decision_log_queue_capacity(),
            timeout: default_decision_log_timeout(),
            shutdown_timeout: default_decision_log_shutdown_timeout(),
        }
    }
}

impl Default for EnvironmentConfig {
    fn default() -> Self {
        Self {
            cors_permissive: true,
            release_zip_password: None,
            provider: ProviderConfig::default(),
            poll_interval: Duration::from_millis(5_000),
            otel_enabled: false,
            http_ssl: None,
            tsgo: TsgoConfig::default(),
            connectors: ConnectorsConfig::default(),
            decision_log: DecisionLogConfig::default(),
        }
    }
}

pub fn deserialize_millis<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Duration::from_millis(<u64>::deserialize(deserializer)?))
}

pub fn deserialize_duration<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    let millis = <u64>::deserialize(deserializer)?;
    if millis < 1_000 {
        return Err(Error::custom(format!(
            "poll interval must be at least 1000 milliseconds ({millis} given)"
        )));
    }

    Ok(Duration::from_millis(millis))
}

#[derive(Debug, Clone, Deserialize, AsRefStr)]
#[serde(tag = "type")]
pub enum ProviderConfig {
    Zip(ZipProviderConfig),
    Filesystem(FilesystemProviderConfig),
    S3(S3ProviderConfig),
    AzureStorage(AzureStorageProviderConfig),
    GCS(GcsProviderConfig),
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self::Zip(ZipProviderConfig::default())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ZipProviderConfig {
    #[serde(default = "default_root")]
    pub root_dir: String,
}

impl Default for ZipProviderConfig {
    fn default() -> Self {
        Self {
            root_dir: default_root(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FilesystemProviderConfig {
    #[serde(default = "default_root")]
    pub root_dir: String,
}

impl Default for FilesystemProviderConfig {
    fn default() -> Self {
        Self {
            root_dir: default_root(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct S3ProviderConfig {
    pub bucket: String,
    #[serde(default)]
    pub force_path_style: bool,
    pub endpoint: Option<String>,
    pub prefix: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AzureStorageProviderConfig {
    pub connection_string: Option<String>,
    pub account_name: Option<String>,
    pub container: String,
    pub prefix: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GcsProviderConfig {
    pub base64_contents: Option<String>,
    pub bucket: String,
    pub prefix: Option<String>,
}

pub fn default_root() -> String {
    "data".to_string()
}

#[derive(Debug, Clone, Default)]
pub struct GlobalAgentConfig {
    pub release_zip_password: Option<Arc<str>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HttpSslConfig {
    pub key: String,
    pub cert: String,
}

impl HttpSslConfig {
    pub async fn to_rustls_config(&self) -> anyhow::Result<RustlsConfig> {
        let cert = BASE64_STANDARD
            .decode(&self.cert)
            .context("Failed to decode SSL certificate")?;
        let key = BASE64_STANDARD
            .decode(&self.key)
            .context("Failed to decode SSL key")?;

        Ok(RustlsConfig::from_pem(cert, key).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::{Config, Environment};

    /// The README documents these names, so they are worth pinning: the
    /// `config` crate maps `TSGO__TIMEOUT` onto `tsgo.timeout` through the
    /// `__` separator, and a missing section still has to default cleanly.
    fn parse(vars: &[(&str, &str)]) -> EnvironmentConfig {
        let source = Environment::default()
            .separator("__")
            .try_parsing(true)
            .source(Some(
                vars.iter()
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect(),
            ));

        Config::builder()
            .add_source(source)
            .set_default("provider.type", "Zip")
            .unwrap()
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap()
    }

    #[test]
    fn tsgo_defaults_when_unset() {
        let config = parse(&[]);

        assert!(config.tsgo.enabled);
        assert_eq!(config.tsgo.memory_bytes, 2 << 30);
        assert_eq!(config.tsgo.timeout, Duration::from_secs(30));
        assert_eq!(config.tsgo.cache_capacity, 4_096);
    }

    #[test]
    fn tsgo_reads_the_documented_env_vars() {
        let config = parse(&[
            ("TSGO__ENABLED", "false"),
            ("TSGO__MEMORY_BYTES", "1073741824"),
            ("TSGO__TIMEOUT", "5000"),
            ("TSGO__CACHE_CAPACITY", "16"),
        ]);

        assert!(!config.tsgo.enabled);
        assert_eq!(config.tsgo.memory_bytes, 1024 * 1024 * 1024);
        assert_eq!(config.tsgo.timeout, Duration::from_millis(5_000));
        assert_eq!(config.tsgo.cache_capacity, 16);
    }

    #[test]
    fn a_partial_tsgo_section_keeps_the_other_defaults() {
        let config = parse(&[("TSGO__ENABLED", "false")]);

        assert!(!config.tsgo.enabled);
        assert_eq!(config.tsgo.timeout, Duration::from_secs(30));
    }

    #[test]
    fn decision_log_is_off_by_default() {
        let config = parse(&[]);

        assert!(config.decision_log.url.is_none());
        assert!(config.decision_log.token.is_none());
        assert_eq!(config.decision_log.batch_size, 100);
    }

    #[test]
    fn decision_log_reads_the_documented_env_vars() {
        let config = parse(&[
            (
                "DECISION_LOG__URL",
                "https://studio.example/api/v1/decision-log/records",
            ),
            ("DECISION_LOG__TOKEN", "dnk_log_secret"),
            ("DECISION_LOG__BATCH_SIZE", "20"),
            ("DECISION_LOG__FLUSH_INTERVAL", "250"),
            ("DECISION_LOG__QUEUE_CAPACITY", "500"),
        ]);

        let log = config.decision_log;
        assert_eq!(
            log.url.as_deref(),
            Some("https://studio.example/api/v1/decision-log/records")
        );
        assert_eq!(log.token.as_deref(), Some("dnk_log_secret"));
        assert_eq!(log.batch_size, 20);
        assert_eq!(log.flush_interval, Duration::from_millis(250));
        assert_eq!(log.queue_capacity, 500);
        assert!(!format!("{log:?}").contains("dnk_log_secret"));
    }
}
