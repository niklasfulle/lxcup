CREATE TABLE schedules (
    id TEXT PRIMARY KEY,
    operation TEXT NOT NULL,
    timezone TEXT NOT NULL,
    target_ids UUID[] NOT NULL,
    every_minutes INTEGER NOT NULL CHECK (every_minutes > 0),
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    threshold JSONB,
    last_run_at TIMESTAMPTZ,
    next_run_at TIMESTAMPTZ NOT NULL,
    last_error TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX schedules_due_idx ON schedules (enabled, next_run_at);
