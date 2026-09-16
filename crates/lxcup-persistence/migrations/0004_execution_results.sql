CREATE TABLE execution_results (
    execution_id UUID PRIMARY KEY REFERENCES executions (id) ON DELETE RESTRICT,
    exit_code INTEGER NOT NULL,
    stdout TEXT NOT NULL DEFAULT '',
    stderr TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
