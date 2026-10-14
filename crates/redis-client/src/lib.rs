use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use redis::sentinel::{Sentinel, SentinelNodeConnectionInfo};
use redis::{Client, ConnectionAddr, RedisConnectionInfo, RedisResult};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const SENTINEL_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct RedisConnectionProvider {
    source: Arc<ConnectionSource>,
}

enum ConnectionSource {
    Direct(ConnectionManager),
    Sentinel {
        endpoints: Vec<String>,
        master_name: String,
        password: Option<String>,
        cache: Mutex<SentinelCache>,
    },
}

#[derive(Default)]
struct SentinelCache {
    manager: Option<ConnectionManager>,
    master_addr: Option<ConnectionAddr>,
    refreshed_at: Option<Instant>,
}

impl RedisConnectionProvider {
    pub async fn from_config(
        master_url: Option<&str>,
        sentinel_endpoints: &[String],
        master_name: &str,
        password: Option<&str>,
    ) -> RedisResult<Self> {
        if let Some(master_url) = master_url {
            let client = Client::open(master_url)?;
            let manager = connection_manager(client).await?;
            return Ok(Self::from_manager(manager));
        }

        let provider = Self {
            source: Arc::new(ConnectionSource::Sentinel {
                endpoints: sentinel_endpoints.to_vec(),
                master_name: master_name.to_owned(),
                password: password.map(str::to_owned),
                cache: Mutex::new(SentinelCache::default()),
            }),
        };
        provider.connection().await?;
        Ok(provider)
    }

    pub fn from_manager(manager: ConnectionManager) -> Self {
        Self {
            source: Arc::new(ConnectionSource::Direct(manager)),
        }
    }

    pub async fn connection(&self) -> RedisResult<ConnectionManager> {
        match self.source.as_ref() {
            ConnectionSource::Direct(manager) => Ok(manager.clone()),
            ConnectionSource::Sentinel {
                endpoints,
                master_name,
                password,
                cache,
            } => {
                let mut cache = cache.lock().await;
                if cache
                    .refreshed_at
                    .is_some_and(|last| last.elapsed() < SENTINEL_REFRESH_INTERVAL)
                {
                    if let Some(manager) = &cache.manager {
                        return Ok(manager.clone());
                    }
                }

                let sentinel_urls = endpoints
                    .iter()
                    .map(|endpoint| format!("redis://{endpoint}/"))
                    .collect();
                let node_info = SentinelNodeConnectionInfo {
                    redis_connection_info: password.as_ref().map(|password| RedisConnectionInfo {
                        password: Some(password.clone()),
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                let mut sentinel = Sentinel::build(sentinel_urls)?;
                let client = sentinel
                    .async_master_for(master_name, Some(&node_info))
                    .await?;
                let master_addr = client.get_connection_info().addr.clone();
                if cache
                    .master_addr
                    .as_ref()
                    .is_some_and(|cached_addr| cached_addr == &master_addr)
                {
                    cache.refreshed_at = Some(Instant::now());
                    if let Some(manager) = &cache.manager {
                        return Ok(manager.clone());
                    }
                }
                let manager = connection_manager(client).await?;
                cache.manager = Some(manager.clone());
                cache.master_addr = Some(master_addr);
                cache.refreshed_at = Some(Instant::now());
                Ok(manager)
            }
        }
    }
}

async fn connection_manager(client: Client) -> RedisResult<ConnectionManager> {
    ConnectionManager::new_with_config(
        client,
        ConnectionManagerConfig::new()
            .set_connection_timeout(Duration::from_secs(2))
            .set_response_timeout(Duration::from_millis(500)),
    )
    .await
}
