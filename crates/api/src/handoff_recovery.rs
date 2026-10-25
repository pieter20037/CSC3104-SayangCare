//! Retry handoff side effects independently of the caller response deadline.
use sayangcare_core::domain::{HandoffStatus, Session, SessionState};
use sayangcare_core::{CoreError, CoreResult};
use std::future::Future;
use tokio::time::{timeout, Duration};

pub fn needs_recovery(session: &Session) -> bool {
    session.state == SessionState::Escalated
        && session.handoff_status == HandoffStatus::Pending
        && session.assigned_volunteer_id.is_none()
}

// Failure of either dependency must not skip the other. Bound background work
// too; retry intent remains in the saved session for the next pass.
pub async fn replay(
    queue: impl Future<Output = CoreResult<()>>,
    alert: impl Future<Output = CoreResult<()>>,
) -> (CoreResult<()>, CoreResult<()>) {
    async fn bounded(work: impl Future<Output = CoreResult<()>>) -> CoreResult<()> {
        timeout(Duration::from_secs(2), work)
            .await
            .map_err(|_| CoreError::DeadlineExceeded("handoff_recovery"))?
    }
    tokio::join!(bounded(queue), bounded(alert))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sayangcare_core::domain::CallerId;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test(start_paused = true)]
    async fn timed_out_queue_does_not_skip_alert_and_can_retry() {
        let alerted = AtomicBool::new(false);
        let (queue, alert) = replay(std::future::pending(), async {
            alerted.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await;
        assert!(queue.is_err());
        assert!(alert.is_ok());
        assert!(alerted.load(Ordering::SeqCst));
        let (queue, alert) = replay(async { Ok(()) }, async { Ok(()) }).await;
        assert!(queue.is_ok() && alert.is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn alert_outage_does_not_skip_queue() {
        let (queue, alert) = replay(async { Ok(()) }, std::future::pending()).await;
        assert!(queue.is_ok());
        assert!(alert.is_err());
    }

    #[test]
    fn recovery_excludes_assigned_resolved_and_ended_calls() {
        let mut session = Session::new(CallerId {
            phone_number: "test".into(),
            display_name: None,
        });
        assert!(!needs_recovery(&session));
        session.state = SessionState::Escalated;
        session.handoff_status = HandoffStatus::Pending;
        assert!(needs_recovery(&session));
        session.assigned_volunteer_id = Some("volunteer".into());
        assert!(!needs_recovery(&session));
        session.assigned_volunteer_id = None;
        session.handoff_status = HandoffStatus::Resolved;
        assert!(!needs_recovery(&session));
        session.handoff_status = HandoffStatus::Pending;
        session.state = SessionState::Completed;
        assert!(!needs_recovery(&session));
    }
}
