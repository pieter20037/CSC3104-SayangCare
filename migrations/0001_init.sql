-- Sessions archive (cold tier)
CREATE TABLE IF NOT EXISTS sessions (
    id              TEXT PRIMARY KEY,
    caller_phone    TEXT NOT NULL,
    state           TEXT NOT NULL,
    transcript      JSONB NOT NULL DEFAULT '{"turns":[]}'::jsonb,
    risk            JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at      TIMESTAMPTZ NOT NULL,
    updated_at      TIMESTAMPTZ NOT NULL,
    version         BIGINT NOT NULL DEFAULT 1
);

CREATE INDEX idx_sessions_caller ON sessions (caller_phone);
CREATE INDEX idx_sessions_created ON sessions (created_at DESC);
CREATE INDEX idx_sessions_state ON sessions (state);

-- Volunteer registry
CREATE TABLE IF NOT EXISTS volunteers (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    display_name    TEXT NOT NULL,
    phone_number    TEXT NOT NULL UNIQUE,
    on_shift        BOOLEAN NOT NULL DEFAULT false,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Volunteer-patient bindings (durable, auditable)
CREATE TABLE IF NOT EXISTS escalations (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id      TEXT NOT NULL REFERENCES sessions (id),
    volunteer_id    UUID NOT NULL REFERENCES volunteers (id),
    risk_level      SMALLINT NOT NULL,
    assigned_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at     TIMESTAMPTZ,
    UNIQUE (session_id)  -- prevent double-booking a session
);

CREATE INDEX idx_escalations_open ON escalations (volunteer_id)
    WHERE resolved_at IS NULL;