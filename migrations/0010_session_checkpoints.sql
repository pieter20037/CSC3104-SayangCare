-- Authoritative commit point for acknowledged active-session mutations.
CREATE TABLE session_checkpoints (
    id TEXT PRIMARY KEY,
    version BIGINT NOT NULL CHECK (version > 0),
    snapshot JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    archived BOOLEAN NOT NULL DEFAULT false
);

CREATE INDEX idx_session_checkpoints_recovery ON session_checkpoints (updated_at)
    WHERE NOT archived;
