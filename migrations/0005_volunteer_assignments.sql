CREATE TABLE IF NOT EXISTS volunteer_assignments (
    session_id TEXT PRIMARY KEY REFERENCES sessions (id) ON DELETE CASCADE,
    volunteer_id TEXT NOT NULL,
    risk_level SMALLINT NOT NULL CHECK (risk_level BETWEEN 1 AND 5),
    status TEXT NOT NULL CHECK (status IN ('assigned', 'transferred', 'resolved')),
    assigned_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_volunteer_assignments_active
    ON volunteer_assignments (volunteer_id)
    WHERE status IN ('assigned', 'transferred');