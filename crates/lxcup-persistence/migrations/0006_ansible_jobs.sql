CREATE TABLE ansible_jobs (
    id UUID PRIMARY KEY,
    target JSONB NOT NULL,
    idempotency_key TEXT NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN ('queued', 'checking', 'planned', 'applying', 'reconcile_required', 'succeeded', 'failed', 'aborted')
    ),
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    UNIQUE (target, idempotency_key)
);

CREATE INDEX ansible_jobs_target_status_idx ON ansible_jobs (target, status);

CREATE TABLE ansible_job_events (
    job_id UUID NOT NULL REFERENCES ansible_jobs (id) ON DELETE CASCADE,
    sequence BIGINT NOT NULL,
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (job_id, sequence)
);
