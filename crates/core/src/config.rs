use serde::{Deserialize, Deserializer};

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
    #[serde(deserialize_with = "deserialize_sentinel_endpoints")]
    pub sentinel_endpoints: Vec<String>,
    pub master_url: Option<String>,
    pub queue_url: Option<String>,
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
        let sequence: Endpoints = serde_json::from_str(
            r#"{"sentinel_endpoints":["127.0.0.1:26379","127.0.0.2:26379"]}"#,
        )
        .unwrap();

        assert_eq!(env_value.sentinel_endpoints, ["127.0.0.1:26379"]);
        assert_eq!(
            sequence.sentinel_endpoints,
            ["127.0.0.1:26379", "127.0.0.2:26379"]
        );
    }
}