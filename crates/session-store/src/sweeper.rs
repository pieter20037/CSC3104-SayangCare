use crate::{RedisSessionStore, TieredSessionStore};
use chrono::{Duration as ChronoDuration, Utc};
use sayangcare_core::domain::SessionState;
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Archives and removes hot sessions that have had no activity for five minutes.
pub struct OrphanedSessionSweeper {
    hot: Arc<RedisSessionStore>,
    tiered: Arc<TieredSessionStore>,
}

impl OrphanedSessionSweeper {
    pub fn new(hot: Arc<RedisSessionStore>, tiered: Arc<TieredSessionStore>) -> Self {
        Self { hot, tiered }
    }

    /// Process one Redis scan. Failed archival leaves the session in Redis for retry.
    pub async fn sweep_once(&self) -> Result<usize, sayangcare_core::CoreError> {
        const INACTIVITY: ChronoDuration = ChronoDuration::minutes(5);
        let cutoff = Utc::now() - INACTIVITY;
        let sessions = self.hot.scan_sessions().await?;
        let mut archived = 0;

        for session in sessions {
            // A terminal session remaining in Redis needs its failed archive retried.
            // Never reclassify Completed or Failed sessions as Abandoned.
            let result = match session.state {
                SessionState::Completed | SessionState::Failed | SessionState::Abandoned => {
                    self.tiered.recover_terminal(&session.id).await
                }
                _ if session.updated_at < cutoff => {
                    self.tiered.abandon_if_inactive(&session.id, cutoff).await
                }
                _ => continue,
            };

            match result {
                Ok(true) => archived += 1,
                Ok(false) => {}
                Err(error) => {
                    warn!(session_id = %session.id, error = %error, "session recovery/sweep failed; retained for retry");
                }
            }
        }

        if archived > 0 {
            info!(archived, "sessions archived by sweeper/recovery");
        }
        Ok(archived)
    }

    pub fn spawn(self: Arc<Self>, cancel: CancellationToken) {
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(30));
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        if let Err(error) = self.sweep_once().await {
                            warn!(error = %error, "orphaned session sweep scan failed");
                        }
                    }
                    _ = cancel.cancelled() => {
                        info!("orphaned session sweeper shutting down");
                        break;
                    }
                }
            }
        });
    }
}
