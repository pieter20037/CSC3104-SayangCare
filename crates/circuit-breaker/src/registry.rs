use crate::breaker::{AdaptiveThresholdPolicy, BreakerConfig, CircuitBreaker};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Named breaker registry, keyed by dependency name (e.g. "llm-openai").
pub struct BreakerRegistry {
    breakers: RwLock<HashMap<String, Arc<CircuitBreaker>>>,
    policy: Arc<dyn AdaptiveThresholdPolicy>,
    default_config: BreakerConfig,
}

impl BreakerRegistry {
    pub fn new(policy: Arc<dyn AdaptiveThresholdPolicy>, default_config: BreakerConfig) -> Self {
        Self {
            breakers: RwLock::new(HashMap::new()),
            policy,
            default_config,
        }
    }

    pub async fn get(&self, name: &str) -> Arc<CircuitBreaker> {
        if let Some(b) = self.breakers.read().await.get(name) {
            return b.clone();
        }
        let mut w = self.breakers.write().await;
        w.entry(name.to_string())
            .or_insert_with(|| {
                Arc::new(CircuitBreaker::new(
                    self.default_config.clone(),
                    self.policy.clone(),
                ))
            })
            .clone()
    }

    /// Background tick - call from a tokio interval task.
    pub async fn tick_all(&self) {
        for b in self.breakers.read().await.values() {
            b.tick().await;
        }
    }
}
