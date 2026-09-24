use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub redis: RedisConfig,
    pub postgres: PostgresConfig,
    pub circuit_breaker: CircuitBreakerConfig,
    pub telephony: TelephonyConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub workers: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
    /// Comma-separated sentinel endpoints: "host1:26379,host2:26379"
    pub sentinel_endpoints: Vec<String>,
    pub master_name: String,
    pub password: Option<String>,
    pub pool_size: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostgresConfig {
    pub url: String,
    pub max_connections: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Base error rate threshold; adapted by sentiment severity.
    pub base_error_rate: f64,
    /// Base latency threshold in ms; adapted by sentiment severity.
    pub base_latency_ms: u64,
    /// Minimum requests before evaluating.
    pub min_requests: u64,
    /// Half-open probe interval.
    pub half_open_after_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelephonyConfig {
    pub twilio_account_sid: String,
    pub twilio_auth_token: String,
    pub public_base_url: String,
}

impl AppConfig {
    pub fn load() -> anyhow::Result<Self> {
        let cfg = config::Config::builder()
            .add_source(config::File::with_name("config/default").required(false))
            .add_source(config::Environment::with_prefix("SAYANGCARE").separator("__"))
            .build()?;
        Ok(cfg.try_deserialize()?)
    }
}