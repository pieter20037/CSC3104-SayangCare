use async_trait::async_trait;
use sayangcare_core::domain::{Session, SessionId, SessionState};
use sayangcare_core::ports::{ArchiveStore, SessionStore};
use sayangcare_core::{CoreError, CoreResult};
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

    /// CAS-commits a terminal transition before archive/delete, so a concurrent turn cannot be lost.
    pub async fn finish(&self, id: &SessionId, terminal_state: SessionState) -> CoreResult<bool> {
        if !matches!(
            terminal_state,
            SessionState::Completed | SessionState::Failed | SessionState::Escalated | SessionState::Abandoned
        ) {
            return Err(CoreError::Internal(anyhow::anyhow!(
                "finish requires a terminal state"
            )));
        }

        let Some(mut session) = self.get(id).await? else {
            return Ok(false);
        };

        if !matches!(
            session.state,
            SessionState::Completed | SessionState::Failed | SessionState::Escalated | SessionState::Abandoned
        ) {
            let expected_version = session.version;
            session.transition(terminal_state)?;
            self.hot.update_cas(&session, expected_version).await?;
        }

        self.flush_to_cold(&session).await?;
        Ok(true)
    }

    /// Mark and archive a session only if its latest activity is older than the cutoff.
    /// The version CAS prevents a concurrent webhook/turn from being lost to the sweeper.
    pub async fn abandon_if_inactive(
        &self,
        id: &SessionId,
        cutoff: chrono::DateTime<chrono::Utc>,
    ) -> CoreResult<bool> {
        let Some(mut session) = self.hot.get(id).await? else {
            return Ok(false);
        };
        if session.state != SessionState::Abandoned && session.updated_at >= cutoff {
            return Ok(false);
        }
        if matches!(session.state, SessionState::Completed | SessionState::Failed) {
            return Ok(false);
        }

        if session.state != SessionState::Abandoned {
            let expected_version = session.version;
            session.transition(SessionState::Abandoned)?;
            self.hot.update_cas(&session, expected_version).await?;
        }

        self.flush_to_cold(&session).await?;
        Ok(true)
    }
}

#[async_trait]
impl SessionStore for TieredSessionStore {
    async fn get(&self, id: &SessionId) -> CoreResult<Option<Session>> {
        // Hot path first.
        if let Some(s) = self.hot.get(id).await? {
            return Ok(Some(s));
        }
        // Cold fallback restores the full record; terminal calls remain cold-only.
        if let Some(session) = self.cold.fetch_session(id).await? {
            if matches!(
                session.state,
                SessionState::Completed | SessionState::Failed | SessionState::Abandoned
            ) {
                return Ok(Some(session));
            }

            if self.hot.create_if_absent(&session).await? {
                return Ok(Some(session));
            }
            return self.hot.get(id).await;
        }
        Ok(None)
    }

    async fn create_if_absent(&self, session: &Session) -> CoreResult<bool> {
        self.hot.create_if_absent(session).await
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

#[cfg(test)]
mod tests {
    use super::TieredSessionStore;
    use async_trait::async_trait;
    use chrono::Utc;
    use sayangcare_core::domain::{CallerId, Session, SessionId, SessionState, Speaker, Turn};
    use sayangcare_core::ports::{ArchiveStore, SessionStore};
    use sayangcare_core::{CoreError, CoreResult};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[derive(Default)]
    struct MemoryHot {
        sessions: Mutex<HashMap<SessionId, Session>>,
        reject_next_cas: AtomicBool,
    }

    #[async_trait]
    impl SessionStore for MemoryHot {
        async fn get(&self, id: &SessionId) -> CoreResult<Option<Session>> {
            Ok(self.sessions.lock().await.get(id).cloned())
        }

        async fn create_if_absent(&self, session: &Session) -> CoreResult<bool> {
            let mut sessions = self.sessions.lock().await;
            if sessions.contains_key(&session.id) {
                return Ok(false);
            }
            sessions.insert(session.id.clone(), session.clone());
            Ok(true)
        }

        async fn put(&self, session: &Session) -> CoreResult<()> {
            self.sessions
                .lock()
                .await
                .insert(session.id.clone(), session.clone());
            Ok(())
        }

        async fn update_cas(&self, session: &Session, expected_version: u64) -> CoreResult<()> {
            if self.reject_next_cas.swap(false, Ordering::SeqCst) {
                return Err(CoreError::VersionConflict {
                    session_id: session.id.0.clone(),
                    expected_version,
                });
            }

            let mut sessions = self.sessions.lock().await;
            let current = sessions
                .get(&session.id)
                .ok_or_else(|| CoreError::SessionNotFound(session.id.0.clone()))?;
            if current.version != expected_version {
                return Err(CoreError::VersionConflict {
                    session_id: session.id.0.clone(),
                    expected_version,
                });
            }
            sessions.insert(session.id.clone(), session.clone());
            Ok(())
        }

        async fn delete(&self, id: &SessionId) -> CoreResult<()> {
            self.sessions.lock().await.remove(id);
            Ok(())
        }
    }

    #[derive(Default)]
    struct MemoryArchive {
        sessions: Mutex<HashMap<SessionId, Session>>,
    }

    #[async_trait]
    impl ArchiveStore for MemoryArchive {
        async fn archive(&self, session: &Session) -> CoreResult<()> {
            self.sessions
                .lock()
                .await
                .insert(session.id.clone(), session.clone());
            Ok(())
        }

        async fn fetch_session(&self, id: &SessionId) -> CoreResult<Option<Session>> {
            Ok(self.sessions.lock().await.get(id).cloned())
        }
    }

    fn session(id: &str) -> Session {
        Session::with_id(
            SessionId(id.to_string()),
            CallerId {
                phone_number: "+6500000000".to_string(),
                display_name: Some("Caller".to_string()),
            },
        )
    }

    #[tokio::test]
    async fn cold_read_rehydrates_full_session_into_hot_store() {
        let hot = Arc::new(MemoryHot::default());
        let cold = Arc::new(MemoryArchive::default());
        let tiered = TieredSessionStore::new(hot.clone(), cold.clone());
        let mut archived = session("CA-rehydrate");
        archived.transition(SessionState::Active).unwrap();
        archived
            .record_turns([Turn {
                speaker: Speaker::Caller,
                text: "Hello".to_string(),
                timestamp: Utc::now(),
            }])
            .unwrap();
        cold.archive(&archived).await.unwrap();

        let restored = tiered
            .get(&archived.id)
            .await
            .unwrap()
            .expect("archived session should be returned");
        let hot_copy = hot
            .get(&archived.id)
            .await
            .unwrap()
            .expect("restored session should be cached");

        assert_eq!(restored.version, archived.version);
        assert_eq!(restored.caller.display_name.as_deref(), Some("Caller"));
        assert_eq!(restored.transcript.turns[0].text, "Hello");
        assert_eq!(hot_copy.version, archived.version);
    }

    #[tokio::test]
    async fn finish_commits_archive_before_removing_hot_session() {
        let hot = Arc::new(MemoryHot::default());
        let cold = Arc::new(MemoryArchive::default());
        let tiered = TieredSessionStore::new(hot.clone(), cold.clone());
        let mut active = session("CA-finish");
        active.transition(SessionState::Active).unwrap();
        hot.put(&active).await.unwrap();

        assert!(tiered
            .finish(&active.id, SessionState::Completed)
            .await
            .unwrap());

        assert!(hot.get(&active.id).await.unwrap().is_none());
        assert_eq!(
            cold.fetch_session(&active.id).await.unwrap().unwrap().state,
            SessionState::Completed
        );
    }

    #[tokio::test]
    async fn finish_preserves_escalated_state_for_high_risk_calls() {
        let hot = Arc::new(MemoryHot::default());
        let cold = Arc::new(MemoryArchive::default());
        let tiered = TieredSessionStore::new(hot.clone(), cold.clone());
        let mut escalated = session("CA-escalated");
        escalated.transition(SessionState::Active).unwrap();
        escalated.transition(SessionState::Escalated).unwrap();
        hot.put(&escalated).await.unwrap();

        assert!(tiered
            .finish(&escalated.id, SessionState::Escalated)
            .await
            .unwrap());

        assert!(hot.get(&escalated.id).await.unwrap().is_none());
        assert_eq!(
            cold.fetch_session(&escalated.id)
                .await
                .unwrap()
                .unwrap()
                .state,
            SessionState::Escalated
        );
    }

    #[tokio::test]
    async fn finish_cas_conflict_does_not_archive_or_delete_session() {
        let hot = Arc::new(MemoryHot::default());
        let cold = Arc::new(MemoryArchive::default());
        let tiered = TieredSessionStore::new(hot.clone(), cold.clone());
        let mut active = session("CA-conflict");
        active.transition(SessionState::Active).unwrap();
        hot.put(&active).await.unwrap();
        hot.reject_next_cas.store(true, Ordering::SeqCst);

        let result = tiered.finish(&active.id, SessionState::Completed).await;

        assert!(matches!(result, Err(CoreError::VersionConflict { .. })));
        assert!(cold.fetch_session(&active.id).await.unwrap().is_none());
        assert_eq!(
            hot.get(&active.id).await.unwrap().unwrap().state,
            SessionState::Active
        );
    }
}
