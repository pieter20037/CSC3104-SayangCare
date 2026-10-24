use serde::{Deserialize, Deserializer};

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub redis: RedisConfig,
    pub postgres: PostgresConfig,
    pub circuit_breaker: CircuitBreakerConfig,
    pub telephony: TelephonyConfig,
    #[serde(default)]
    pub ai: AiConfig,
    #[serde(default)]
    pub queue: QueueConfig,
    #[serde(default)]
    pub performance: PerformanceConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PerformanceConfig {
    pub response_budget_ms: u64,
    pub persistence_reserve_ms: u64,
    pub sentiment_timeout_ms: u64,
    pub reply_timeout_ms: u64,
    pub coordination_timeout_ms: u64,
    pub max_ai_turns: usize,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            response_budget_ms: 5000,
            persistence_reserve_ms: 1000,
            sentiment_timeout_ms: 800,
            reply_timeout_ms: 3000,
            coordination_timeout_ms: 200,
            max_ai_turns: 8,
        }
    }
}

impl PerformanceConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.response_budget_ms > self.persistence_reserve_ms,
            "response budget must exceed persistence reserve"
        );
        anyhow::ensure!(
            self.persistence_reserve_ms > 0
                && self.sentiment_timeout_ms > 0
                && self.reply_timeout_ms > 0
                && self.coordination_timeout_ms > 0
                && self.max_ai_turns > 0,
            "performance limits must be positive"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct QueueConfig {
    /// `redis` (default, single shared Redis sorted set) or `raft` (replicated OpenRaft cluster).
    pub backend: String,
    pub raft_node_id: u64,
    /// Cluster members as `1=http://host-a:8081,2=http://host-b:8081`. Empty = single node.
    pub raft_peers: String,
    /// Directory for this node's persisted Raft state; in-memory when unset.
    pub raft_data_dir: Option<String>,
    /// Shared secret required on `/internal/raft/*` peer requests.
    pub raft_token: Option<String>,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            backend: "redis".to_string(),
            raft_node_id: 1,
            raft_peers: String::new(),
            raft_data_dir: None,
            raft_token: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    pub api_key: Option<String>,
    pub model: String,
    pub base_url: String,
    pub timeout_secs: u64,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            model: "allam-2-7b".to_string(),
            base_url: "https://api.groq.com/openai/v1/chat/completions".to_string(),
            timeout_secs: 12,
        }
    }
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
    #[serde(deserialize_with = "deserialize_sentinel_endpoints")]
    pub sentinel_endpoints: Vec<String>,
    pub master_url: Option<String>,
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
    #[serde(default = "default_breaker_prefix")]
    pub key_prefix: String,
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
    pub twilio_from_number: Option<String>,
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

fn deserialize_sentinel_endpoints<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Endpoints {
        Sequence(Vec<String>),
        String(String),
    }

    match Endpoints::deserialize(deserializer)? {
        Endpoints::Sequence(endpoints) => Ok(endpoints),
        Endpoints::String(value) => {
            let value = value.trim();
            let value = value
                .strip_prefix('[')
                .and_then(|value| value.strip_suffix(']'))
                .unwrap_or(value);

            Ok(value
                .split(',')
                .map(|endpoint| endpoint.trim().trim_matches('"').to_owned())
                .filter(|endpoint| !endpoint.is_empty())
                .collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::deserialize_sentinel_endpoints;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Endpoints {
        #[serde(deserialize_with = "deserialize_sentinel_endpoints")]
        sentinel_endpoints: Vec<String>,
    }

    #[test]
    fn sentinel_endpoints_accept_env_string_and_toml_sequence() {
        let env_value: Endpoints =
            serde_json::from_str(r#"{"sentinel_endpoints":"[127.0.0.1:26379]"}"#).unwrap();
        let sequence: Endpoints =
            serde_json::from_str(r#"{"sentinel_endpoints":["127.0.0.1:26379","127.0.0.2:26379"]}"#)
                .unwrap();

        assert_eq!(env_value.sentinel_endpoints, ["127.0.0.1:26379"]);
        assert_eq!(
            sequence.sentinel_endpoints,
            ["127.0.0.1:26379", "127.0.0.2:26379"]
        );
    }
}

fn default_breaker_prefix() -> String {
    "sayangcare:circuit-breaker".into()
}
