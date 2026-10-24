-- Shared durable operator alerts for all API replicas.
CREATE TABLE IF NOT EXISTS operator_alerts (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    session_id TEXT NOT NULL,
    volunteer_id TEXT,
    message TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_operator_alerts_created ON operator_alerts (created_at DESC, id DESC);
