ALTER TABLE docker_workloads
    ADD COLUMN ports JSONB NOT NULL DEFAULT '[]',
    ADD COLUMN started_at TEXT NULL,
    ADD COLUMN labels JSONB NOT NULL DEFAULT '[]',
    ADD COLUMN presence TEXT NOT NULL DEFAULT 'present' CHECK (presence IN ('present', 'missing')),
    ADD COLUMN last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW();
