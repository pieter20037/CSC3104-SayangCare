-- Support bounded missing-alert recovery without changing existing alerts.
CREATE INDEX IF NOT EXISTS idx_operator_alerts_session_kind
    ON operator_alerts (session_id, kind);
CREATE INDEX IF NOT EXISTS idx_sessions_escalated_recovery
    ON sessions (id) WHERE escalation_count > 0;
