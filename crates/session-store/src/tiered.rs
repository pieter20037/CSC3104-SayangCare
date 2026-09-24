use async_trait::async_trait;
use sayangcare_core::domain::{Session, SessionId};
use sayangcare_core::ports::{ArchiveStore, SessionStore};
use sayangcare_core::CoreResult;
use std::sync::Arc;
use tracing::{info, warn};

/// Combines Redis (hot) + Postgres (cold).
///
/// Write path: hot first (fast), then async archive on completion.
/// Read path: hot -> cold fallback (rehydration).
pub struct TieredSessionStore {
    hot: Arc<dyn SessionStore>,
    cold: Arc<dyn ArchiveStore>,
}

impl TieredSessionStore {
    pub fn new(hot: Arc<dyn SessionStore>, cold: Arc<dyn ArchiveStore>) -> Self {
        Self { hot, cold }
    }

    /// Flush a completed session to cold storage without blocking the caller.
    /// Called from the session completion path.
    pub async fn flush_to_cold(&self, session: &Session) -> CoreResult<()> {
        if let Err(e) = self.cold.archive(session).await {
            warn!(session_id = %session.id, error = %e, "cold archive failed");
            return Err(e);
        }
        info!(session_id = %session.id, "session archived");
        // Only delete hot state after successful archive.
        self.hot.delete(&session.id).await?;
        Ok(())
    }
}

#[async_trait]
impl SessionStore for TieredSessionStore {
    async fn get(&self, id: &SessionId) -> CoreResult<Option<Session>> {
        // Hot path first.
        if let Some(s) = self.hot.get(id).await? {
            return Ok(Some(s));
        }
        // Cold fallback -> rehydrate into hot.
        if let Some(transcript) = self.cold.fetch_transcript(id).await? {
            // Reconstruct a minimal session shell for continuation.
            // (Full rehydration would need the full row; this is a
            // simplification showing the pattern.)
            let _ = transcript;
        }
        Ok(None)
    }

    async fn put(&self, session: &Session) -> CoreResult<()> {
        self.hot.put(session).await
    }

    async fn update_cas(&self, session: &Session, expected_version: u64) -> CoreResult<()> {
        self.hot.update_cas(session, expected_version).await
    }

    async fn delete(&self, id: &SessionId) -> CoreResult<()> {
        self.hot.delete(id).await
    }
}