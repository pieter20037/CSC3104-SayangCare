use crate::breaker::{AdaptiveThresholdPolicy, BreakerConfig};
use crate::shared::SharedCircuitBreaker;
use redis::aio::ConnectionManager;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Named breaker registry, keyed by dependency name (e.g. "llm-openai").
pub struct BreakerRegistry {
    breakers: RwLock<HashMap<String, Arc<SharedCircuitBreaker>>>,
    policy: Arc<dyn AdaptiveThresholdPolicy>,
    default_config: BreakerConfig,
    client: ConnectionManager,
    prefix: String,
    probe_lease_ms: u64,
}

impl BreakerRegistry {
    pub fn new(
        policy: Arc<dyn AdaptiveThresholdPolicy>,
        default_config: BreakerConfig,
        client: ConnectionManager,
        prefix: impl Into<String>,
        probe_lease_ms: u64,
    ) -> Self {
        Self {
            breakers: RwLock::new(HashMap::new()),
            policy,
            default_config,
            client,
            prefix: prefix.into(),
            probe_lease_ms,
        }
    }

    pub async fn get(&self, name: &str) -> Arc<SharedCircuitBreaker> {
        if let Some(b) = self.breakers.read().await.get(name) {
            return b.clone();
        }
        let mut w = self.breakers.write().await;
        w.entry(name.to_string())
            .or_insert_with(|| {
                Arc::new(SharedCircuitBreaker::new(
                    name,
                    &self.prefix,
                    self.client.clone(),
                    self.default_config.clone(),
                    self.policy.clone(),
                    self.probe_lease_ms,
                ))
            })
            .clone()
    }

}
