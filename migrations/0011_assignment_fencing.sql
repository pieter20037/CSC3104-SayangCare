ALTER TABLE volunteer_assignments
    ADD COLUMN assignment_generation BIGINT NOT NULL DEFAULT 1
    CHECK (assignment_generation > 0);

ALTER TABLE sessions ADD COLUMN assignment_generation BIGINT NOT NULL DEFAULT 0
    CHECK (assignment_generation >= 0);

WITH numbered AS (
    SELECT assignment_id,
           row_number() OVER (PARTITION BY session_id ORDER BY assignment_id) * 2 - 1 AS generation
    FROM volunteer_assignments
)
UPDATE volunteer_assignments a
SET assignment_generation = n.generation
FROM numbered n WHERE a.assignment_id = n.assignment_id;

CREATE TABLE case_ownership (
    session_id TEXT PRIMARY KEY,
    volunteer_id TEXT,
    assignment_generation BIGINT NOT NULL DEFAULT 0 CHECK (assignment_generation >= 0),
    coordinator TEXT NOT NULL CHECK (coordinator IN ('postgres', 'raft')),
    pending_operation_id TEXT UNIQUE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO case_ownership(session_id, volunteer_id, assignment_generation, coordinator)
SELECT DISTINCT ON (session_id) session_id,
    CASE WHEN resolved_at IS NULL THEN volunteer_id ELSE NULL END,
    assignment_generation + CASE WHEN resolved_at IS NULL THEN 0 ELSE 1 END,
    'postgres'
FROM volunteer_assignments ORDER BY session_id, assignment_id DESC;

CREATE TABLE assignment_operations (
    request_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES case_ownership(session_id),
    command JSONB NOT NULL,
    outcome JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    projected_at TIMESTAMPTZ
);
CREATE INDEX assignment_operations_pending
    ON assignment_operations(created_at) WHERE completed_at IS NULL;

CREATE TABLE volunteer_case_notes (
    note_id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    request_id TEXT NOT NULL UNIQUE,
    session_id TEXT NOT NULL REFERENCES case_ownership(session_id),
    volunteer_id TEXT NOT NULL,
    assignment_generation BIGINT NOT NULL CHECK (assignment_generation > 0),
    body TEXT NOT NULL CHECK (length(body) BETWEEN 1 AND 4000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

UPDATE session_checkpoints s
SET snapshot = jsonb_set(s.snapshot, '{assignment_generation}', to_jsonb(o.assignment_generation))
FROM case_ownership o WHERE s.id = o.session_id;

UPDATE sessions s SET assignment_generation=o.assignment_generation
FROM case_ownership o WHERE s.id=o.session_id;
