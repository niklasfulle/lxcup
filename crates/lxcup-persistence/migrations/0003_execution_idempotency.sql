CREATE TABLE execution_requests (
    idempotency_key TEXT PRIMARY KEY,
    execution_id UUID NOT NULL UNIQUE REFERENCES executions (id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (length(trim(idempotency_key)) BETWEEN 1 AND 200)
);

CREATE INDEX execution_requests_execution_id_idx ON execution_requests (execution_id);
