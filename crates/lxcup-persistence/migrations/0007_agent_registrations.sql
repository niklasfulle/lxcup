CREATE TABLE agent_registrations (
    id UUID PRIMARY KEY,
    container_id BIGINT NOT NULL REFERENCES containers (id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL,
    endpoint TEXT NOT NULL,
    secret_ref UUID NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('connected', 'degraded', 'unreachable')),
    payload JSONB NOT NULL,
    last_checked_at TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    UNIQUE (container_id)
);

CREATE INDEX agent_registrations_state_idx ON agent_registrations (state);
